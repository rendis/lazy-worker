//! The socket that the helper connects to, and the one place where a remote request meets the dispatcher.
//!
//! The helper speaks for a peer, but the peer is only a claim. Every request looks the peer up
//! again, so a removed or downgraded device loses access on its next request, on the same open
//! connection. The socket answers only our own user.

use super::{
    identity::DeviceId,
    log::{Entry, Log},
    our_uid,
    pake::SESSION_TIMEOUT,
    pairing::Pairing,
    peer_uid,
    peers::Peers,
    wire::{error_line, read_line, LineError, MAX_REQUEST_LINE},
};
use crate::dispatch::{Origin, Permissions, Request};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufReader, Cursor, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const MAX_CONNECTIONS: usize = 16;
const IDLE: Duration = Duration::from_secs(10 * 60);
const HELLO_WAIT: Duration = Duration::from_secs(10);
const MAX_RESPONSE: usize = 1024 * 1024;
/// `sun_path` holds 104 bytes with its end mark on macOS.
const SOCKET_PATH_MAX: usize = 103;

/// Runs one request for an origin. The app runs the same dispatcher as the webview does.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, origin: Origin, request: Request) -> Result<Value, &'static str>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    hello: HelloBody,
}

/// A paired device names itself with `peer`. A device that is pairing says `pair: true`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloBody {
    peer: Option<String>,
    pair: Option<bool>,
}

/// The registered name of a pairing connection. It is not a device id, so it never matches a peer.
const PAIRING_CONNECTION: &str = "pairing";

/// A stream that ends its reads at a fixed time, whatever the other side does.
struct Deadlined {
    stream: UnixStream,
    deadline: Instant,
}

impl Read for Deadlined {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buffer)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    request: Value,
}

#[derive(Default)]
struct Connections {
    next: u64,
    open: HashMap<u64, (String, UnixStream)>,
}

struct Shared {
    peers: Arc<Peers>,
    log: Arc<Log>,
    pairing: Arc<Pairing>,
    handler: Box<dyn Handler>,
    connections: Mutex<Connections>,
    live: AtomicUsize,
    stopped: AtomicBool,
}

pub struct Server {
    shared: Arc<Shared>,
    path: PathBuf,
}

/// Frees the slot of a connection when its thread ends, however it ends.
struct Slot {
    shared: Arc<Shared>,
    id: Option<u64>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.shared.lock().open.remove(&id);
        }
        self.shared.live.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Connections> {
        self.connections
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// The `command` tag of a request. Nothing else of the request is ever recorded.
fn command_name(request: &Value) -> String {
    request
        .get("command")
        .and_then(Value::as_str)
        .filter(|name| name.len() <= 48 && name.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .unwrap_or("unknown")
        .to_owned()
}

impl Server {
    /// Binds the socket and starts the accept thread. A path that is too long, or a leftover
    /// that is not ours, stops it, and the app then reports why.
    pub fn start(
        path: PathBuf,
        peers: Arc<Peers>,
        log: Arc<Log>,
        pairing: Arc<Pairing>,
        handler: impl Handler,
    ) -> Result<Self, &'static str> {
        if path.as_os_str().len() > SOCKET_PATH_MAX {
            return Err("socketPathTooLong");
        }
        let directory = path.parent().ok_or("remoteIo")?;
        let meta = fs::symlink_metadata(directory).map_err(|_| "remoteIo")?;
        if !meta.is_dir() || meta.uid() != our_uid() || meta.mode() & 0o077 != 0 {
            return Err("remoteDirInvalid");
        }
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_socket() && meta.uid() == our_uid() => {
                fs::remove_file(&path).map_err(|_| "remoteIo")?;
            }
            Ok(_) => return Err("socketPathBusy"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("remoteIo"),
        }
        let listener = UnixListener::bind(&path).map_err(|_| "socketBindFailed")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|_| "remoteIo")?;
        let shared = Arc::new(Shared {
            peers,
            log,
            pairing,
            handler: Box::new(handler),
            connections: Mutex::new(Connections::default()),
            live: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
        });
        let accepting = shared.clone();
        thread::Builder::new()
            .name("remote-accept".into())
            .spawn(move || accept(listener, accepting))
            .map_err(|_| "remoteIo")?;
        Ok(Self { shared, path })
    }

    /// Ends every open connection of one peer at once.
    pub fn close_peer(&self, id: &str) {
        for (peer, stream) in self.shared.lock().open.values() {
            if peer == id {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    pub fn close_all(&self) {
        for (_, stream) in self.shared.lock().open.values() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// The ids of the peers that have a connection now.
    pub fn connected(&self) -> HashSet<String> {
        self.shared
            .lock()
            .open
            .values()
            .map(|(peer, _)| peer.clone())
            .collect()
    }

    /// Ends the connections and removes the socket.
    pub fn stop(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        self.close_all();
        let _ = fs::remove_file(&self.path);
        // A connection wakes the accept thread, so it sees the flag and ends.
        let _ = UnixStream::connect(&self.path);
    }
}

fn accept(listener: UnixListener, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.stopped.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        if shared.live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            shared.live.fetch_sub(1, Ordering::SeqCst);
            let mut stream = stream;
            let _ = stream.write_all(error_line("busy").as_bytes());
            continue;
        }
        let slot = Slot {
            shared: shared.clone(),
            id: None,
        };
        let spawned = thread::Builder::new()
            .name("remote-connection".into())
            .spawn(move || connection(stream, slot));
        if spawned.is_err() {
            // The closure, and with it the slot, is dropped, which frees the count.
        }
    }
}

/// Lists the connection, so that cutting a device or stopping the server ends it.
fn register(shared: &Shared, slot: &mut Slot, name: &str, stream: UnixStream) {
    let mut open = shared.lock();
    open.next += 1;
    let key = open.next;
    open.open.insert(key, (name.to_owned(), stream));
    slot.id = Some(key);
}

/// Runs one pairing session. It has `SESSION_TIMEOUT` in all, from the first line on.
fn serve_pairing(reader: BufReader<UnixStream>, mut writer: UnixStream, shared: &Shared) {
    // The start message may have come in the same read as the hello, so keep what is buffered.
    let buffered = reader.buffer().to_vec();
    let stream = reader.into_inner();
    let deadline = Instant::now() + SESSION_TIMEOUT;
    let _ = writer.set_write_timeout(Some(SESSION_TIMEOUT));
    let mut input = BufReader::new(Cursor::new(buffered).chain(Deadlined { stream, deadline }));
    shared.pairing.serve(&mut input, &mut writer);
}

fn connection(stream: UnixStream, mut slot: Slot) {
    // Only our own user may talk to the app. The socket mode already says so, and this checks it.
    if peer_uid(&stream) != Some(our_uid()) {
        return;
    }
    let shared = slot.shared.clone();
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let Ok(registered) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let _ = reader.get_ref().set_read_timeout(Some(HELLO_WAIT));
    let reply = |writer: &mut UnixStream, code: &str| {
        let _ = writer.write_all(error_line(code).as_bytes());
    };

    let hello = match read_line(&mut reader, MAX_REQUEST_LINE) {
        Ok(Some(line)) => line,
        Err(LineError::TooLong) => return reply(&mut writer, "lineTooLong"),
        _ => return,
    };
    let Ok(Hello {
        hello: HelloBody { peer, pair },
    }) = serde_json::from_slice(&hello)
    else {
        return reply(&mut writer, "badHello");
    };
    let id = match (peer, pair) {
        (Some(peer), None) => match DeviceId::parse(&peer) {
            Ok(id) if shared.peers.get(id.as_str()).is_some() => id.as_str().to_owned(),
            _ => return reply(&mut writer, "notPaired"),
        },
        (None, Some(true)) => {
            register(&shared, &mut slot, PAIRING_CONNECTION, registered);
            return serve_pairing(reader, writer, &shared);
        }
        _ => return reply(&mut writer, "badHello"),
    };
    register(&shared, &mut slot, &id, registered);
    let _ = reader.get_ref().set_read_timeout(Some(IDLE));

    loop {
        let line = match read_line(&mut reader, MAX_REQUEST_LINE) {
            Ok(Some(line)) => line,
            Err(LineError::TooLong) => return reply(&mut writer, "lineTooLong"),
            // The end of the input, an idle timeout, or a connection that was shut down.
            _ => return,
        };
        let envelope = serde_json::from_slice::<Envelope>(&line).map(|envelope| envelope.request);
        let name = envelope
            .as_ref()
            .map_or_else(|_| "unknown".to_owned(), command_name);
        // The peer is read now, not when the connection opened.
        let Some(peer) = shared.peers.get(&id) else {
            let _ = shared
                .log
                .append(&Entry::new(Some(&id), None, &name, "notPaired"));
            return reply(&mut writer, "notPaired");
        };
        let mut read = false;
        let result = match envelope.map(serde_json::from_value::<Request>) {
            Ok(Ok(request)) => {
                read = request.is_read();
                let origin = Origin::Remote(Permissions {
                    control: peer.permissions.control,
                    actions: peer.permissions.actions,
                });
                shared.handler.handle(origin, request)
            }
            _ => Err("badRequest"),
        };
        shared.peers.touch(&id);
        let outcome = result.as_ref().map(|_| "ok").unwrap_or_else(|code| code);
        // A read that worked is not worth a line. Anything else is.
        if !(read && result.is_ok()) {
            let _ = shared
                .log
                .append(&Entry::new(Some(&id), Some(&peer.name), &name, outcome));
        }
        let response = match result {
            Ok(value) => serde_json::to_string(&serde_json::json!({ "ok": value }))
                .ok()
                .filter(|text| text.len() < MAX_RESPONSE)
                .unwrap_or_else(|| error_line("responseTooLarge").trim_end().to_owned()),
            Err(code) => error_line(code).trim_end().to_owned(),
        };
        if writer
            .write_all(format!("{response}\n").as_bytes())
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dispatch::authorize,
        preferences::Preferences,
        remote::{
            helper,
            peers::testing::{peer, ID_A, ID_B},
            testing::Sink,
        },
    };
    use std::{io::BufRead, sync::atomic::AtomicU32};
    use crate::{
        devices::Kind,
        remote::{
            authorized_keys::AuthorizedKeys,
            pake::{testing::HOST_KEY, Own},
            peers::{testing::KEY, PeerPermissions},
        },
    };

    /// A handler that runs the dispatcher's own permission check, then answers without an app.
    struct Fake;

    impl Handler for Fake {
        fn handle(&self, origin: Origin, request: Request) -> Result<Value, &'static str> {
            authorize(origin, &request, &Preferences::default())?;
            Ok(serde_json::json!("done"))
        }
    }

    struct Setup {
        dir: PathBuf,
        socket: PathBuf,
        peers: Arc<Peers>,
        log: Arc<Log>,
        pairing: Arc<Pairing>,
        server: Server,
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            self.server.stop();
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// The directory is short and in `/tmp`, because a socket path has a hard limit.
    fn setup(_label: &str) -> Setup {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = PathBuf::from(format!(
            "/tmp/lw-t-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let peers = Arc::new(Peers::open(dir.join("peers.json")).unwrap());
        let log = Arc::new(Log::open(dir.join("log.jsonl")).unwrap());
        let socket = dir.join("app.sock");
        let host_key_file = dir.join("host.pub");
        fs::write(&host_key_file, format!("{HOST_KEY}\n")).unwrap();
        let pairing = Arc::new(Pairing::for_test(
            peers.clone(),
            log.clone(),
            AuthorizedKeys::new(dir.clone(), PathBuf::from("/Applications/L.app/lw")),
            Arc::new(|| {
                Ok(Own {
                    id: DeviceId::parse(ID_B).unwrap(),
                    name: "Studio".into(),
                    kind: Kind::Desktop,
                    public_key: KEY.into(),
                })
            }),
            host_key_file,
            Duration::from_secs(600),
        ));
        let server = Server::start(
            socket.clone(),
            peers.clone(),
            log.clone(),
            pairing.clone(),
            Fake,
        )
        .unwrap();
        Setup {
            dir,
            socket,
            peers,
            log,
            pairing,
            server,
        }
    }

    impl Setup {
        fn connect(&self, peer: &str) -> (UnixStream, BufReader<UnixStream>) {
            let stream = UnixStream::connect(&self.socket).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream.try_clone().unwrap();
            writeln!(writer, "{}", serde_json::json!({"hello": {"peer": peer}})).unwrap();
            (stream, reader)
        }

        fn pair(&self, id: &str, control: bool, actions: bool) {
            self.peers.insert(peer(id, "Studio")).unwrap();
            self.peers
                .set_permissions(id, control, false, actions)
                .unwrap();
        }
    }

    fn ask(stream: &mut UnixStream, reader: &mut BufReader<UnixStream>, request: Value) -> Value {
        writeln!(stream, "{}", serde_json::json!({ "request": request })).unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or(Value::Null)
    }

    fn at_end(reader: &mut BufReader<UnixStream>) -> bool {
        let mut rest = String::new();
        matches!(reader.read_line(&mut rest), Ok(0) | Err(_))
    }

    fn status() -> Value {
        serde_json::json!({"command": "getStatuses"})
    }

    #[test]
    fn a_removed_or_downgraded_peer_loses_access_on_the_same_open_connection() {
        let setup = setup("revoke");
        setup.pair(ID_A, true, false);
        let (mut stream, mut reader) = setup.connect(ID_A);
        assert_eq!(
            ask(&mut stream, &mut reader, status()),
            serde_json::json!({"ok": "done"})
        );
        assert!(setup.server.connected().contains(ID_A));
        // Requests only the local webview may send.
        for local in [
            serde_json::json!({"command": "getDevices"}),
            serde_json::json!({"command": "saveProxyApps", "args": {"apps": []}}),
        ] {
            assert_eq!(
                ask(&mut stream, &mut reader, local),
                serde_json::json!({"error": "localOnly"})
            );
        }
        // Actions are off for this peer.
        let launch = serde_json::json!({"command": "launchProxyApp", "args": {"id": "claude"}});
        assert_eq!(
            ask(&mut stream, &mut reader, launch),
            serde_json::json!({"ok": "done"})
        );

        setup
            .peers
            .set_permissions(ID_A, false, false, false)
            .unwrap();
        assert_eq!(
            ask(&mut stream, &mut reader, status()),
            serde_json::json!({"error": "notPermitted"})
        );
        setup
            .peers
            .set_permissions(ID_A, true, false, false)
            .unwrap();
        assert_eq!(
            ask(&mut stream, &mut reader, status()),
            serde_json::json!({"ok": "done"})
        );

        setup.peers.remove(ID_A).unwrap();
        assert_eq!(
            ask(&mut stream, &mut reader, status()),
            serde_json::json!({"error": "notPaired"})
        );
        assert!(at_end(&mut reader));

        let log = setup.log.read();
        assert_eq!(log[0].outcome, "notPaired");
        assert_eq!(log[0].name, None);
        // Refused reads are logged, and so are the changes that worked.
        assert_eq!(log[1].outcome, "notPermitted");
        assert_eq!(log[1].name.as_deref(), Some("Studio"));
        assert!(log
            .iter()
            .all(|entry| entry.request.chars().all(|c| c.is_ascii_alphanumeric())));
        assert!(setup.peers.get(ID_A).is_none());
    }

    #[test]
    fn a_successful_read_is_not_logged_but_a_refused_read_and_a_change_are() {
        let setup = setup("noise");
        setup.pair(ID_A, true, false);
        let (mut stream, mut reader) = setup.connect(ID_A);
        ask(&mut stream, &mut reader, status());
        assert!(setup.log.read().is_empty());
        ask(
            &mut stream,
            &mut reader,
            serde_json::json!({"command": "getDevices"}),
        );
        let theme = serde_json::json!({"command": "setTheme", "args": {"theme": "dark"}});
        assert_eq!(
            ask(&mut stream, &mut reader, theme),
            serde_json::json!({"ok": "done"})
        );
        let log = setup.log.read();
        assert_eq!(log.len(), 2);
        assert_eq!(
            (log[0].request.as_str(), log[0].outcome.as_str()),
            ("setTheme", "ok")
        );
        assert_eq!(
            (log[1].request.as_str(), log[1].outcome.as_str()),
            ("getDevices", "localOnly")
        );
    }

    #[test]
    fn closing_a_peer_ends_its_connection_at_once() {
        let setup = setup("close");
        setup.pair(ID_A, true, false);
        setup.pair(ID_B, true, false);
        let (mut a, mut a_reader) = setup.connect(ID_A);
        let (mut b, mut b_reader) = setup.connect(ID_B);
        ask(&mut a, &mut a_reader, status());
        ask(&mut b, &mut b_reader, status());
        assert_eq!(setup.server.connected().len(), 2);
        setup.server.close_peer(ID_A);
        assert!(at_end(&mut a_reader));
        assert_eq!(
            ask(&mut b, &mut b_reader, status()),
            serde_json::json!({"ok": "done"})
        );
        setup.server.close_all();
        assert!(at_end(&mut b_reader));
        // The registry empties when the threads end.
        for _ in 0..50 {
            if setup.server.connected().is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(setup.server.connected().is_empty());
    }

    #[test]
    fn an_unknown_or_malformed_hello_is_refused() {
        let setup = setup("hello");
        setup.pair(ID_A, true, false);
        for hello in [ID_B, "short", "../../etc"] {
            let (_stream, mut reader) = setup.connect(hello);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim(), r#"{"error":"notPaired"}"#, "{hello}");
            assert!(at_end(&mut reader));
        }
        for raw in [
            "not json\n",
            "{\"request\":{}}\n",
            "{\"hello\":{\"peer\":\"x\"},\"more\":1}\n",
            "{\"hello\":{}}\n",
            "{\"hello\":{\"pair\":false}}\n",
            "{\"hello\":{\"peer\":\"x\",\"pair\":true}}\n",
        ] {
            let mut stream = UnixStream::connect(&setup.socket).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream.write_all(raw.as_bytes()).unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim(), r#"{"error":"badHello"}"#, "{raw}");
        }
        assert!(setup.server.connected().is_empty());
    }

    #[test]
    fn a_bad_request_is_answered_and_the_session_goes_on() {
        let setup = setup("bad");
        setup.pair(ID_A, true, false);
        let (mut stream, mut reader) = setup.connect(ID_A);
        let unknown = serde_json::json!({"command": "formatDisk"});
        assert_eq!(
            ask(&mut stream, &mut reader, unknown),
            serde_json::json!({"error": "badRequest"})
        );
        stream.write_all(b"{\"nothing\":1}\n").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), r#"{"error":"badRequest"}"#);
        assert_eq!(
            ask(&mut stream, &mut reader, status()),
            serde_json::json!({"ok": "done"})
        );
        // The arguments of a request never reach the log.
        let secret =
            serde_json::json!({"command": "setProxyServer", "args": {"server": "secret-host:1"}});
        ask(&mut stream, &mut reader, secret);
        let raw = fs::read_to_string(setup.dir.join("log.jsonl")).unwrap();
        assert!(raw.contains("setProxyServer") && !raw.contains("secret-host"));
    }

    #[test]
    fn an_oversize_line_ends_the_session() {
        let setup = setup("big");
        setup.pair(ID_A, true, false);
        let (mut stream, mut reader) = setup.connect(ID_A);
        assert_eq!(
            ask(&mut stream, &mut reader, status()),
            serde_json::json!({"ok": "done"})
        );
        let mut big = vec![b'a'; MAX_REQUEST_LINE + 1];
        big.push(b'\n');
        let _ = stream.write_all(&big);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), r#"{"error":"lineTooLong"}"#);
        assert!(at_end(&mut reader));
    }

    #[test]
    fn the_socket_is_private_and_a_stale_one_of_ours_is_replaced() {
        let setup = setup("stale");
        assert_eq!(fs::metadata(&setup.socket).unwrap().mode() & 0o777, 0o600);
        let again = Server::start(
            setup.socket.clone(),
            setup.peers.clone(),
            setup.log.clone(),
            setup.pairing.clone(),
            Fake,
        );
        assert!(again.is_ok(), "the old socket file is replaced");
        again.unwrap().stop();
        // A file that is not a socket is never removed.
        let other = setup.dir.join("other.sock");
        fs::write(&other, "x").unwrap();
        let busy = Server::start(
            other.clone(),
            setup.peers.clone(),
            setup.log.clone(),
            setup.pairing.clone(),
            Fake,
        );
        assert_eq!(busy.err(), Some("socketPathBusy"));
        assert_eq!(fs::read_to_string(other).unwrap(), "x");
    }

    #[test]
    fn a_path_that_is_too_long_stops_the_server() {
        let setup = setup("long");
        let long = setup.dir.join("x".repeat(100)).join("app.sock");
        let result = Server::start(
            long,
            setup.peers.clone(),
            setup.log.clone(),
            setup.pairing.clone(),
            Fake,
        );
        assert_eq!(result.err(), Some("socketPathTooLong"));
    }

    #[test]
    fn a_directory_that_others_can_read_is_refused() {
        let setup = setup("mode");
        let open = setup.dir.join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
        let result = Server::start(
            open.join("a.sock"),
            setup.peers.clone(),
            setup.log.clone(),
            setup.pairing.clone(),
            Fake,
        );
        assert_eq!(result.err(), Some("remoteDirInvalid"));
    }

    #[test]
    fn connections_past_the_cap_are_turned_away() {
        let setup = setup("cap");
        setup.pair(ID_A, true, false);
        let mut held = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            held.push(setup.connect(ID_A));
        }
        let mut over = UnixStream::connect(&setup.socket).unwrap();
        over.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(over.try_clone().unwrap());
        let _ = writeln!(over, "{{}}");
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), r#"{"error":"busy"}"#);
    }

    /// The helper and the server together, with a byte stream in place of sshd.
    #[test]
    fn the_helper_relays_requests_and_answers_over_the_socket() {
        let setup = setup("relay");
        setup.pair(ID_A, true, false);
        let sink = Sink::default();
        let input = format!(
            "{}\n{}\n",
            serde_json::json!({"request": status()}),
            serde_json::json!({"request": {"command": "getDevices"}})
        );
        let id = helper::Role::Peer(DeviceId::parse(ID_A).unwrap());
        let code = helper::session(
            &setup.socket,
            &id,
            std::io::Cursor::new(input.into_bytes()),
            helper::Output::new(sink.clone()),
        );
        assert_eq!(code, 0);
        assert_eq!(
            sink.text(),
            "{\"ok\":\"done\"}\n{\"error\":\"localOnly\"}\n"
        );
    }

    #[test]
    fn the_helper_ends_with_an_error_for_an_oversize_line_and_for_an_unknown_peer() {
        let setup = setup("relay-bad");
        setup.pair(ID_A, true, false);
        let id = helper::Role::Peer(DeviceId::parse(ID_A).unwrap());
        let sink = Sink::default();
        let mut big = vec![b'a'; MAX_REQUEST_LINE + 1];
        big.push(b'\n');
        let code = helper::session(
            &setup.socket,
            &id,
            std::io::Cursor::new(big),
            helper::Output::new(sink.clone()),
        );
        assert_eq!(code, helper::EXIT_TOO_LONG);
        assert!(sink.text().contains("lineTooLong"));

        let stranger = helper::Role::Peer(DeviceId::parse(ID_B).unwrap());
        let sink = Sink::default();
        let code = helper::session(
            &setup.socket,
            &stranger,
            std::io::Cursor::new(Vec::new()),
            helper::Output::new(sink.clone()),
        );
        assert_eq!(code, 0);
        assert_eq!(sink.text(), "{\"error\":\"notPaired\"}\n");
    }

    /// Connects as a device that is pairing, and sends the lines.
    fn pairing_talk(setup: &Setup, lines: &[&str]) -> Vec<String> {
        let mut stream = UnixStream::connect(&setup.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        writeln!(stream, "{{\"hello\":{{\"pair\":true}}}}").unwrap();
        for line in lines {
            writeln!(stream, "{line}").unwrap();
        }
        let mut answers = Vec::new();
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            answers.push(line.trim().to_owned());
            line.clear();
        }
        answers
    }

    #[test]
    fn a_pair_hello_is_served_only_while_a_window_is_open() {
        let setup = setup("pair-hello");
        assert_eq!(
            pairing_talk(&setup, &[]),
            [r#"{"error":"pairingClosed"}"#]
        );
        setup
            .pairing
            .open(PeerPermissions::default())
            .unwrap();
        // The session runs, and a line that is not a start message ends it.
        assert_eq!(
            pairing_talk(&setup, &["{\"request\":{}}"]),
            [r#"{"error":"badMessage"}"#]
        );
        // A pairing connection is not a device, and no peer is needed for it.
        assert!(setup.server.connected().is_empty());
        setup.pairing.close().unwrap();
        assert_eq!(
            pairing_talk(&setup, &[]),
            [r#"{"error":"pairingClosed"}"#]
        );
    }

    #[test]
    fn a_pairing_session_ends_when_the_server_stops() {
        let setup = setup("pair-stop");
        setup
            .pairing
            .open(PeerPermissions::default())
            .unwrap();
        let stream = UnixStream::connect(&setup.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut writer = stream.try_clone().unwrap();
        writeln!(writer, "{{\"hello\":{{\"pair\":true}}}}").unwrap();
        thread::sleep(Duration::from_millis(300));
        assert_eq!(setup.server.connected().len(), 1);
        setup.server.close_all();
        let mut reader = BufReader::new(stream);
        assert!(at_end(&mut reader));
    }
}
