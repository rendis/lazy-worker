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
    wire::{error_line, read_line, LineError, MAX_DISPATCH_LINE, MAX_REQUEST_LINE},
    SOCKET_PATH_MAX,
};
use crate::dispatch::{Origin, PeerId, Permissions, Request};
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
/// Open connections that one paired device may have.
const MAX_PER_PEER: usize = 4;
/// Open shell sessions that one paired device may have. They count apart from the connections above.
const MAX_SHELLS_PER_PEER: usize = 2;
/// Pairing connections at once. The pairing session takes one slot too.
const MAX_PAIRING: usize = 1;

/// The times that bound a connection. A test may use shorter ones.
#[derive(Clone, Copy)]
struct Limits {
    /// From connect to the end of the hello line.
    hello: Duration,
    /// From the hello to the start message of a pairing.
    start: Duration,
    /// A whole pairing session, from the hello.
    session: Duration,
    /// Between request lines.
    idle: Duration,
    /// From the first byte of a request line to its end.
    line: Duration,
    /// For every write to the peer.
    write: Duration,
}

const LIMITS: Limits = Limits {
    hello: Duration::from_secs(10),
    start: Duration::from_secs(10),
    session: SESSION_TIMEOUT,
    idle: Duration::from_secs(10 * 60),
    line: Duration::from_secs(30),
    write: Duration::from_secs(10),
};
const MAX_RESPONSE: usize = 1024 * 1024;

/// Runs one request for an origin. The app runs the same dispatcher as the webview does.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, origin: Origin, request: Request) -> Result<Value, &'static str>;

    /// A shell session of a peer began, or ended. The default does nothing.
    fn shell_started(&self, _peer: &str) {}
    fn shell_ended(&self, _peer: &str) {}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    hello: HelloBody,
}

/// A paired device names itself with `peer`. A device that is pairing says `pair: true`. A peer
/// that wants a shell adds `shell: true`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloBody {
    peer: Option<String>,
    pair: Option<bool>,
    shell: Option<bool>,
}

/// The registered name of a pairing connection. It is not a device id, so it never matches a peer.
const PAIRING_CONNECTION: &str = "pairing";

/// A stream that ends its reads at a fixed time, whatever the other side does. Until the first
/// line is complete it also ends them at `first_line`.
struct Deadlined {
    stream: UnixStream,
    deadline: Instant,
    first_line: Option<Instant>,
}

impl Read for Deadlined {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let limit = self.first_line.map_or(self.deadline, |at| at.min(self.deadline));
        let left = limit.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        let count = self.stream.read(buffer)?;
        if buffer[..count].contains(&b'\n') {
            self.first_line = None;
        }
        Ok(count)
    }
}

/// A stream with two read limits: `idle` while no line has begun, and `line` from the first
/// byte of a line to its end, so a slow drip is cut.
struct Dripped {
    stream: UnixStream,
    idle: Duration,
    line: Duration,
    /// When the line that is now partly read began.
    began: Option<Instant>,
}

/// Where the line stands after `chunk`: when the line now partly read began, if there is one.
fn line_began(began: Option<Instant>, chunk: &[u8], now: Instant) -> Option<Instant> {
    match chunk.iter().rposition(|&byte| byte == b'\n') {
        // The chunk ends a line. What follows the last end mark begins the next one.
        Some(last) => (last + 1 < chunk.len()).then_some(now),
        None if chunk.is_empty() => began,
        None => began.or(Some(now)),
    }
}

impl Read for Dripped {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let wait = match self.began {
            Some(began) => self.line.saturating_sub(began.elapsed()),
            None => self.idle,
        };
        if wait.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(wait))?;
        let count = self.stream.read(buffer)?;
        self.began = line_began(self.began, &buffer[..count], Instant::now());
        Ok(count)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    request: Value,
}

/// One listed connection. A shell session is listed too, so that revoking, cutting, or removing ends it.
struct Open {
    peer: String,
    stream: UnixStream,
    shell: bool,
}

#[derive(Default)]
struct Connections {
    next: u64,
    open: HashMap<u64, Open>,
}

struct Shared {
    peers: Arc<Peers>,
    log: Arc<Log>,
    pairing: Arc<Pairing>,
    handler: Box<dyn Handler>,
    limits: Limits,
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
        Self::start_with(path, peers, log, pairing, handler, LIMITS)
    }

    fn start_with(
        path: PathBuf,
        peers: Arc<Peers>,
        log: Arc<Log>,
        pairing: Arc<Pairing>,
        handler: impl Handler,
        limits: Limits,
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
            limits,
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
        for open in self.shared.lock().open.values() {
            if open.peer == id {
                let _ = open.stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    /// Ends the shell sessions of one peer and leaves its other connections open.
    pub fn close_shell(&self, id: &str) {
        for open in self.shared.lock().open.values() {
            if open.shell && open.peer == id {
                let _ = open.stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    pub fn close_all(&self) {
        for open in self.shared.lock().open.values() {
            let _ = open.stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// The ids of the peers that have a connection now.
    pub fn connected(&self) -> HashSet<String> {
        self.shared
            .lock()
            .open
            .values()
            .filter(|open| !open.shell)
            .map(|open| open.peer.clone())
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
            let _ = stream.set_write_timeout(Some(shared.limits.write));
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

/// Lists the connection, so that cutting a device or stopping the server ends it. A name that
/// has `max` connections already is refused, and the check and the listing are one step.
fn register(
    shared: &Shared,
    slot: &mut Slot,
    name: &str,
    max: usize,
    stream: UnixStream,
    shell: bool,
) -> Result<(), ()> {
    let mut open = shared.lock();
    let same = |open: &&Open| open.peer == name && open.shell == shell;
    if open.open.values().filter(same).count() >= max {
        return Err(());
    }
    open.next += 1;
    let key = open.next;
    open.open.insert(
        key,
        Open {
            peer: name.to_owned(),
            stream,
            shell,
        },
    );
    slot.id = Some(key);
    Ok(())
}

/// Runs one pairing session. It has `limits.session` in all, from the hello, and its start
/// message must come within `limits.start`.
fn serve_pairing(reader: BufReader<Dripped>, mut writer: UnixStream, shared: &Shared) {
    // The start message may have come in the same read as the hello, so keep what is buffered.
    let buffered = reader.buffer().to_vec();
    let stream = reader.into_inner().stream;
    let now = Instant::now();
    let limits = shared.limits;
    let first_line = (!buffered.contains(&b'\n')).then(|| now + limits.start);
    let _ = writer.set_write_timeout(Some(limits.write));
    let mut input = BufReader::new(Cursor::new(buffered).chain(Deadlined {
        stream,
        deadline: now + limits.session,
        first_line,
    }));
    shared.pairing.serve(&mut input, &mut writer);
}

/// Keeps a shell session open. It answers `ok` only while the grant of this peer lasts now, and it
/// holds the connection until the helper or the app ends it. The app ends it when the grant ends,
/// when the peer is removed, when all access is cut, or when it quits.
fn serve_shell(
    mut reader: BufReader<Dripped>,
    mut writer: UnixStream,
    registered: UnixStream,
    slot: &mut Slot,
    shared: &Shared,
    id: &str,
) {
    let record = |name: Option<&str>, request: &str, outcome: &str| {
        let _ = shared.log.append(&Entry::new(Some(id), name, request, outcome));
    };
    let granted = || shared.peers.get(id).filter(|peer| peer.permissions.shell);
    let Some(peer) = granted() else {
        let name = shared.peers.get(id).map(|peer| peer.name);
        record(name.as_deref(), "shellStart", "notPermitted");
        let _ = writer.write_all(error_line("notPermitted").as_bytes());
        return;
    };
    if register(shared, slot, id, MAX_SHELLS_PER_PEER, registered, true).is_err() {
        record(Some(&peer.name), "shellStart", "busy");
        let _ = writer.write_all(error_line("busy").as_bytes());
        return;
    }
    // A grant that ended between the check and the listing would have missed the call that
    // closes the sessions. The grant is read again now that the session is listed.
    if granted().is_none() {
        record(Some(&peer.name), "shellStart", "notPermitted");
        let _ = writer.write_all(error_line("notPermitted").as_bytes());
        return;
    }
    // The start is recorded before the helper hears `ok`, so no shell runs without its line in the log.
    record(Some(&peer.name), "shellStart", "ok");
    shared.handler.shell_started(id);
    let stream = &mut reader.get_mut().stream;
    if writer.write_all(b"{\"ok\":\"shell\"}\n").is_ok() {
        // The helper sends nothing during a session. The read ends when either side closes the socket.
        let _ = stream.set_read_timeout(None);
        let mut sink = [0u8; 256];
        while matches!(stream.read(&mut sink), Ok(count) if count > 0) {}
    }
    let name = shared.peers.get(id).map_or(peer.name, |peer| peer.name);
    shared.handler.shell_ended(id);
    record(Some(&name), "shellEnd", "ok");
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
    let limits = shared.limits;
    let _ = writer.set_write_timeout(Some(limits.write));
    let mut reader = BufReader::new(Dripped {
        stream,
        idle: limits.hello,
        line: limits.hello,
        began: None,
    });
    let reply = |writer: &mut UnixStream, code: &str| {
        let _ = writer.write_all(error_line(code).as_bytes());
    };

    let hello = match read_line(&mut reader, MAX_REQUEST_LINE) {
        Ok(Some(line)) => line,
        Err(LineError::TooLong) => return reply(&mut writer, "lineTooLong"),
        _ => return,
    };
    let Ok(Hello {
        hello: HelloBody { peer, pair, shell },
    }) = serde_json::from_slice(&hello)
    else {
        return reply(&mut writer, "badHello");
    };
    let id = match (peer, pair, shell) {
        (Some(peer), None, _) => match DeviceId::parse(&peer) {
            Ok(id) if shared.peers.get(id.as_str()).is_some() => id.as_str().to_owned(),
            _ => return reply(&mut writer, "notPaired"),
        },
        (None, Some(true), None) => {
            if register(&shared, &mut slot, PAIRING_CONNECTION, MAX_PAIRING, registered, false).is_err() {
                return reply(&mut writer, "pairingBusy");
            }
            return serve_pairing(reader, writer, &shared);
        }
        _ => return reply(&mut writer, "badHello"),
    };
    if shell == Some(true) {
        return serve_shell(reader, writer, registered, &mut slot, &shared, &id);
    }
    if register(&shared, &mut slot, &id, MAX_PER_PEER, registered, false).is_err() {
        return reply(&mut writer, "busy");
    }
    {
        let timed = reader.get_mut();
        timed.idle = limits.idle;
        timed.line = limits.line;
    }

    loop {
        let line = match read_line(&mut reader, MAX_DISPATCH_LINE) {
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
                .append_request(&Entry::new(Some(&id), None, &name, "notPaired"));
            return reply(&mut writer, "notPaired");
        };
        let mut read = false;
        let result = match envelope.map(serde_json::from_value::<Request>) {
            Ok(Ok(request)) => {
                // A chunk of a file is one of hundreds, so it never fills the log. Uploads and deletes are logged as events.
                read = request.is_read() || request.is_file_chunk();
                let origin = Origin::Remote(Permissions {
                    control: peer.permissions.control,
                    applications: peer.permissions.applications,
                    actions: peer.permissions.actions,
                    screen: peer.permissions.screen,
                    files: peer
                        .files_grant
                        .as_ref()
                        .filter(|grant| grant.until.active(super::now()))
                        .map(|grant| grant.mode),
                    peer: PeerId::parse(&id),
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
                .append_request(&Entry::new(Some(&id), Some(&peer.name), &name, outcome));
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
        setup_with(LIMITS)
    }

    fn setup_with(limits: Limits) -> Setup {
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
        let server = Server::start_with(
            socket.clone(),
            peers.clone(),
            log.clone(),
            pairing.clone(),
            Fake,
            limits,
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
                .set_permissions(id, control, false, actions, false)
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
            .set_permissions(ID_A, false, false, false, false)
            .unwrap();
        assert_eq!(
            ask(&mut stream, &mut reader, status()),
            serde_json::json!({"error": "notPermitted"})
        );
        setup
            .peers
            .set_permissions(ID_A, true, false, false, false)
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
    fn the_applications_permission_of_a_peer_reaches_the_dispatcher_on_every_request() {
        let setup = setup("apps-permission");
        setup.pair(ID_A, true, false);
        let (mut stream, mut reader) = setup.connect(ID_A);
        let list = serde_json::json!({"command": "listApplications"});
        let open = serde_json::json!({"command": "openApplication", "args": {"bundleId": "com.apple.Safari"}});
        // Control alone is refused.
        assert_eq!(ask(&mut stream, &mut reader, list.clone()), serde_json::json!({"error": "notPermitted"}));
        // Applications alone is enough, and it is read from the saved peer on the next request.
        setup.peers.set_permissions(ID_A, false, true, false, false).unwrap();
        assert_eq!(ask(&mut stream, &mut reader, list.clone()), serde_json::json!({"ok": "done"}));
        assert_eq!(ask(&mut stream, &mut reader, open), serde_json::json!({"ok": "done"}));
        assert_eq!(ask(&mut stream, &mut reader, status()), serde_json::json!({"error": "notPermitted"}));
        setup.peers.set_permissions(ID_A, true, false, true, false).unwrap();
        assert_eq!(ask(&mut stream, &mut reader, list), serde_json::json!({"error": "notPermitted"}));
        // A refused read is logged. A read that worked is not, and opening an application always is.
        let outcomes: Vec<_> = setup.log.read().iter().map(|e| (e.request.clone(), e.outcome.clone())).collect();
        // The two refused lists share one line with a count.
        assert_eq!(setup.log.read().iter().map(|entry| entry.count).sum::<u32>(), 4);
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes.contains(&("openApplication".to_owned(), "ok".to_owned())));
    }

    #[test]
    fn the_files_grant_of_a_peer_reaches_the_dispatcher_on_every_request_and_chunks_are_not_logged() {
        use crate::devices::{FilesGrant, FilesMode, FilesScope, ShellUntil};
        let setup = setup("files-permission");
        setup.pair(ID_A, true, false);
        let (mut stream, mut reader) = setup.connect(ID_A);
        let list = serde_json::json!({"command": "listFiles", "args": {"root": "r", "path": ""}});
        let chunk = serde_json::json!({"command": "writeFileChunk", "args": {"uploadId": "x", "offset": 0, "data": "AA=="}});
        let grant = |mode, until| FilesGrant { scope: FilesScope::Disk, folders: vec![], mode, until };
        // Control alone reaches no file.
        assert_eq!(ask(&mut stream, &mut reader, list.clone()), serde_json::json!({"error": "notPermitted"}));
        // Read only: a read passes, and a write does not.
        setup.peers.set_files(ID_A, Some(grant(FilesMode::Read, ShellUntil::OFF))).unwrap();
        assert_eq!(ask(&mut stream, &mut reader, list.clone()), serde_json::json!({"ok": "done"}));
        assert_eq!(ask(&mut stream, &mut reader, chunk.clone()), serde_json::json!({"error": "notPermitted"}));
        // Read and write passes both, and the grant is read again on the same connection.
        setup.peers.set_files(ID_A, Some(grant(FilesMode::ReadWrite, ShellUntil::OFF))).unwrap();
        assert_eq!(ask(&mut stream, &mut reader, chunk.clone()), serde_json::json!({"ok": "done"}));
        // A grant that has ended refuses at once, with no tick needed.
        setup.peers.set_files(ID_A, Some(grant(FilesMode::ReadWrite, ShellUntil::At(1)))).unwrap();
        assert_eq!(ask(&mut stream, &mut reader, list.clone()), serde_json::json!({"error": "notPermitted"}));
        setup.peers.set_files(ID_A, None).unwrap();
        assert_eq!(ask(&mut stream, &mut reader, chunk), serde_json::json!({"error": "notPermitted"}));
        // Chunks that worked left no line. Only the refusals did.
        let log = setup.log.read();
        assert!(log.iter().all(|entry| entry.outcome == "notPermitted"), "{log:?}");
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
        setup.log.flush();
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
        let mut big = vec![b'a'; MAX_DISPATCH_LINE + 1];
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
        // Connections that have not said hello yet count too.
        let held: Vec<_> = (0..MAX_CONNECTIONS)
            .map(|_| UnixStream::connect(&setup.socket).unwrap())
            .collect();
        thread::sleep(Duration::from_millis(200));
        let mut over = UnixStream::connect(&setup.socket).unwrap();
        over.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(over.try_clone().unwrap());
        let _ = writeln!(over, "{{}}");
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), r#"{"error":"busy"}"#);
        drop(held);
    }

    #[test]
    fn one_peer_may_hold_only_a_few_connections_and_others_are_not_affected() {
        let setup = setup("per-peer");
        setup.pair(ID_A, true, false);
        setup.pair(ID_B, true, false);
        let mut held = Vec::new();
        for _ in 0..MAX_PER_PEER {
            let (mut stream, mut reader) = setup.connect(ID_A);
            assert_eq!(
                ask(&mut stream, &mut reader, status()),
                serde_json::json!({"ok": "done"})
            );
            held.push((stream, reader));
        }
        let (_stream, mut reader) = setup.connect(ID_A);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), r#"{"error":"busy"}"#);
        let (mut other, mut other_reader) = setup.connect(ID_B);
        assert_eq!(
            ask(&mut other, &mut other_reader, status()),
            serde_json::json!({"ok": "done"})
        );
        // A closed connection frees its place.
        let (first, first_reader) = held.remove(0);
        drop((first, first_reader));
        for _ in 0..50 {
            if setup.server.shared.lock().open.values().filter(|open| open.peer == ID_A).count()
                < MAX_PER_PEER
            {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let (mut again, mut again_reader) = setup.connect(ID_A);
        assert_eq!(
            ask(&mut again, &mut again_reader, status()),
            serde_json::json!({"ok": "done"})
        );
    }

    #[test]
    fn a_slow_drip_of_one_request_line_is_cut_but_an_idle_connection_is_not() {
        let setup = setup_with(Limits {
            line: Duration::from_millis(400),
            idle: Duration::from_secs(5),
            ..LIMITS
        });
        setup.pair(ID_A, true, false);
        let (mut stream, mut reader) = setup.connect(ID_A);
        assert_eq!(
            ask(&mut stream, &mut reader, status()),
            serde_json::json!({"ok": "done"})
        );
        // Quiet between lines is fine.
        thread::sleep(Duration::from_millis(700));
        assert_eq!(
            ask(&mut stream, &mut reader, status()),
            serde_json::json!({"ok": "done"})
        );
        // One byte at a time, never finishing the line.
        let started = Instant::now();
        for byte in b"{\"request\":{\"command\":\"getStatuses\"" {
            if stream.write_all(&[*byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(60));
        }
        assert!(at_end(&mut reader));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn the_line_clock_starts_at_the_first_byte_and_stops_at_the_end_mark() {
        let at = Instant::now();
        let later = at + Duration::from_secs(1);
        assert_eq!(line_began(None, b"", at), None);
        assert_eq!(line_began(None, b"abc", at), Some(at));
        assert_eq!(line_began(Some(at), b"def", later), Some(at));
        assert_eq!(line_began(Some(at), b"end\n", later), None);
        assert_eq!(line_began(Some(at), b"end\nnext", later), Some(later));
        assert_eq!(line_began(None, b"one\ntwo\n", later), None);
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
        let mut big = vec![b'a'; MAX_DISPATCH_LINE + 1];
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
    fn a_silent_pairing_client_is_cut_and_frees_the_slot() {
        let setup = setup_with(Limits {
            start: Duration::from_millis(300),
            ..LIMITS
        });
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
        // A second pairing connection is turned away while the first one holds the slot.
        thread::sleep(Duration::from_millis(100));
        assert_eq!(pairing_talk(&setup, &[]), [r#"{"error":"pairingBusy"}"#]);
        let started = Instant::now();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), r#"{"error":"pairingFailed"}"#);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(at_end(&mut reader));
        // The slot is free at once: the next client is served, and a bad line ends it.
        assert_eq!(
            pairing_talk(&setup, &["{\"request\":{}}"]),
            [r#"{"error":"badMessage"}"#]
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

    /// A controller that reaches the app socket through the real helper, in this process.
    fn controller(setup: &Setup, peer: &str) -> (crate::remote::client::Clients, Arc<AtomicUsize>) {
        let opened = Arc::new(AtomicUsize::new(0));
        let clients = crate::remote::client::Clients::with_limits(
            crate::remote::client::testing::HelperTransport {
                socket: setup.socket.clone(),
                peer: peer.to_owned(),
                opened: opened.clone(),
            },
            Duration::from_secs(5),
            Duration::from_millis(150),
        );
        (clients, opened)
    }

    fn target() -> crate::remote::controlled::Controlled {
        crate::remote::controlled::testing::device(ID_B, "Studio")
    }

    #[test]
    fn the_client_gets_answers_and_refusals_from_the_real_helper_and_server() {
        let setup = setup("client-round");
        setup.pair(ID_A, true, false);
        let (clients, opened) = controller(&setup, ID_A);
        assert_eq!(clients.call(&target(), &status()), Ok(serde_json::json!("done")));
        assert!(clients.connected(&target().id));
        // The refusals of the target reach the controller as they are.
        assert_eq!(
            clients.call(&target(), &serde_json::json!({"command": "getDevices"})),
            Err("localOnly")
        );
        let launch = serde_json::json!({"command": "setSeenVersion", "args": {"version": null}});
        assert_eq!(clients.call(&target(), &launch), Ok(serde_json::json!("done")));
        let own_command = serde_json::json!({"command": "getViewer"});
        assert_eq!(clients.call(&target(), &own_command), Err("localOnly"));
        assert_eq!(opened.load(Ordering::SeqCst), 1, "one link carried all of it");
        // Unknown words and bad requests come back as codes of ours.
        assert_eq!(
            clients.call(&target(), &serde_json::json!({"command": "nope"})),
            Err("badRequest")
        );
    }

    #[test]
    fn a_paired_device_with_no_permission_may_ask_who_answers_and_nothing_else() {
        let setup = setup("client-session");
        setup.pair(ID_A, false, false);
        let (clients, _) = controller(&setup, ID_A);
        assert_eq!(
            clients.call(&target(), &serde_json::json!({"command": "getSession"})),
            Ok(serde_json::json!("done"))
        );
        assert_eq!(clients.call(&target(), &status()), Err("notPermitted"));
    }

    #[test]
    fn an_unknown_device_cannot_even_ask_who_answers() {
        let setup = setup("client-unknown");
        let (clients, _) = controller(&setup, ID_A);
        assert_eq!(
            clients.call(&target(), &serde_json::json!({"command": "getSession"})),
            Err("notPaired")
        );
        assert!(!clients.connected(&target().id));
    }

    #[test]
    fn a_server_that_stops_ends_the_session_and_the_next_call_after_the_wait_reconnects() {
        let setup = setup("client-restart");
        setup.pair(ID_A, true, false);
        let (clients, opened) = controller(&setup, ID_A);
        assert!(clients.call(&target(), &status()).is_ok());
        setup.server.stop();
        // The app is gone, so the retry reaches a helper that says so.
        let ended = clients.call(&target(), &status());
        assert_eq!(ended, Err("appNotRunning"));
        assert!(!clients.connected(&target().id));
        let links = opened.load(Ordering::SeqCst);
        assert_eq!(links, 2, "one retry for that request and no more");
        // Inside the wait there is no new link.
        assert_eq!(clients.call(&target(), &status()), Err("appNotRunning"));
        assert_eq!(opened.load(Ordering::SeqCst), links);
        // The app starts again, and the next call after the wait works.
        let again = Server::start(
            setup.socket.clone(),
            setup.peers.clone(),
            setup.log.clone(),
            setup.pairing.clone(),
            Fake,
        )
        .unwrap();
        thread::sleep(Duration::from_millis(200));
        assert_eq!(clients.call(&target(), &status()), Ok(serde_json::json!("done")));
        assert!(clients.connected(&target().id));
        again.stop();
    }

    #[test]
    fn a_peer_that_is_removed_ends_the_session_of_the_client() {
        let setup = setup("client-removed");
        setup.pair(ID_A, true, false);
        let (clients, _) = controller(&setup, ID_A);
        assert!(clients.call(&target(), &status()).is_ok());
        setup.peers.remove(ID_A).unwrap();
        setup.server.close_peer(ID_A);
        assert_eq!(clients.call(&target(), &status()), Err("notPaired"));
    }

    // The shell sessions: a real helper function, the real server, and `sh` in place of the login shell.

    fn grant_shell(setup: &Setup, id: &str, until: crate::devices::ShellUntil) {
        setup.pair(id, true, false);
        setup.peers.set_shell(id, Some(until)).unwrap();
    }

    /// Runs the helper's shell session on a thread. The stand-in shell writes the id of its `sleep` to the file.
    fn start_shell(setup: &Setup, id: &str, pids: &PathBuf) -> (Sink, std::sync::mpsc::Receiver<i32>) {
        let (socket, peer, pids) = (setup.socket.clone(), DeviceId::parse(id).unwrap(), pids.clone());
        let sink = Sink::default();
        let (done, result) = std::sync::mpsc::channel();
        let output = sink.clone();
        thread::spawn(move || {
            use std::os::unix::process::CommandExt;
            let code = helper::shell_session_with(
                &socket,
                &peer,
                true,
                Some(PathBuf::from("/bin/sh")),
                &helper::Output::new(output),
                Duration::from_millis(500),
                move |_| {
                    let mut command = std::process::Command::new("/bin/sh");
                    command
                        .args(["-c", &format!("sleep 60 & echo $! > '{}'; wait", pids.display())])
                        .stdin(std::process::Stdio::null())
                        .process_group(0);
                    command
                },
            );
            let _ = done.send(code);
        });
        (sink, result)
    }

    fn sleeper(pids: &PathBuf) -> i32 {
        for _ in 0..200 {
            if let Some(pid) = fs::read_to_string(pids).ok().and_then(|text| text.trim().parse().ok()) {
                return pid;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("the shell did not start");
    }

    fn process_gone(pid: i32) -> bool {
        for _ in 0..200 {
            // SAFETY: signal 0 only checks that the process exists.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        false
    }

    fn shell_open(setup: &Setup) -> bool {
        setup.server.shared.lock().open.values().any(|open| open.shell)
    }

    #[test]
    fn the_helper_refuses_a_shell_without_an_active_grant_and_starts_nothing() {
        use crate::devices::ShellUntil;
        let setup = setup("shell-refused");
        setup.pair(ID_A, true, true);
        for until in [None, Some(ShellUntil::At(1))] {
            setup.peers.set_shell(ID_A, until).unwrap();
            let sink = Sink::default();
            let code = helper::shell_session_with(
                &setup.socket,
                &DeviceId::parse(ID_A).unwrap(),
                true,
                Some(PathBuf::from("/bin/sh")),
                &helper::Output::new(sink.clone()),
                Duration::from_secs(1),
                |_| panic!("no shell may start"),
            );
            assert_eq!(code, helper::EXIT_REFUSED);
            assert_eq!(sink.text(), "Full shell is off on this device.\r\n");
        }
        assert!(!shell_open(&setup));
        let log = setup.log.read();
        assert!(log.iter().all(|entry| entry.request == "shellStart" && entry.outcome == "notPermitted"));
        assert_eq!((log.len(), log[0].count), (1, 2), "two refusals, one line");
        // A device that is not paired gets nothing either.
        let stranger = DeviceId::parse(ID_B).unwrap();
        let sink = Sink::default();
        let code = helper::shell_session_with(
            &setup.socket, &stranger, true, Some(PathBuf::from("/bin/sh")), &helper::Output::new(sink.clone()),
            Duration::from_secs(1), |_| panic!("no shell may start"),
        );
        assert_eq!(code, helper::EXIT_REFUSED);
    }

    #[test]
    fn the_pairing_hello_cannot_ask_for_a_shell() {
        let setup = setup("shell-pair");
        for hello in [
            serde_json::json!({"hello": {"pair": true, "shell": true}}),
            serde_json::json!({"hello": {"shell": true}}),
            serde_json::json!({"hello": {"peer": ID_A, "pair": true, "shell": true}}),
        ] {
            let stream = UnixStream::connect(&setup.socket).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            writeln!(&stream, "{hello}").unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            assert_eq!(line, "{\"error\":\"badHello\"}\n", "{hello}");
        }
        assert!(!shell_open(&setup));
    }

    #[test]
    fn a_shell_runs_while_the_grant_lasts_and_each_way_to_end_it_ends_the_session() {
        use crate::devices::ShellUntil;
        for ending in ["revoke", "cut", "remove", "stop"] {
            let setup = setup("shell-ends");
            grant_shell(&setup, ID_A, ShellUntil::OFF);
            let pids = setup.dir.join("pids");
            let (_sink, result) = start_shell(&setup, ID_A, &pids);
            let sleeping = sleeper(&pids);
            assert!(shell_open(&setup), "{ending}");
            // A shell is not a connection that makes the device show as connected.
            assert!(!setup.server.connected().contains(ID_A));
            match ending {
                "revoke" => {
                    setup.peers.set_shell(ID_A, None).unwrap();
                    setup.server.close_shell(ID_A);
                }
                "cut" => {
                    setup.peers.clear().unwrap();
                    setup.server.close_all();
                }
                "remove" => {
                    setup.peers.remove(ID_A).unwrap();
                    setup.server.close_peer(ID_A);
                }
                _ => setup.server.stop(),
            }
            result.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| panic!("{ending}: the helper did not end"));
            assert!(process_gone(sleeping), "{ending}: the shell group is still running");
            for _ in 0..100 {
                if setup.log.read().iter().any(|entry| entry.request == "shellEnd") {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            let events: Vec<_> = setup.log.read().iter().rev().map(|entry| (entry.request.clone(), entry.outcome.clone())).collect();
            assert_eq!(
                events,
                [("shellStart".to_owned(), "ok".to_owned()), ("shellEnd".to_owned(), "ok".to_owned())],
                "{ending}"
            );
        }
    }

    #[test]
    fn revoking_a_shell_leaves_the_other_connections_of_the_device_open() {
        use crate::devices::ShellUntil;
        let setup = setup("shell-keeps");
        grant_shell(&setup, ID_A, ShellUntil::OFF);
        let (mut stream, mut reader) = setup.connect(ID_A);
        assert_eq!(ask(&mut stream, &mut reader, status()), serde_json::json!({"ok": "done"}));
        let pids = setup.dir.join("pids");
        let (_sink, result) = start_shell(&setup, ID_A, &pids);
        let sleeping = sleeper(&pids);
        setup.server.close_shell(ID_A);
        result.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(process_gone(sleeping));
        assert_eq!(ask(&mut stream, &mut reader, status()), serde_json::json!({"ok": "done"}));
    }
}
