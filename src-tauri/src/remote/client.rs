//! The controller side of a session: one line out, one line back, over one long-lived link.
//!
//! The link is a pair of pipes that a `Transport` opens. In the app it is the system ssh client
//! running `dispatch` on the other device. A test opens the helper in this process instead.
//! Everything that comes back is untrusted. It is size-capped, parsed strictly, and an error
//! word that we do not know becomes `remoteFailed`.

use super::{
    controlled::Controlled,
    identity::DeviceId,
    ssh::{self, RemoteCommand},
    wire::{read_line, LineError, MAX_DISPATCH_LINE, MAX_RESPONSE_LINE},
    Layout,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{BufReader, Read, Write},
    process::Child,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// How long one request may take, from its first byte out to the end of its answer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// After a link fails, the next try waits this long. The calls in between get the same error.
pub const BACKOFF: Duration = Duration::from_secs(5);

/// The error words that the other device may send. Any other word becomes `remoteFailed`, so
/// text from outside never reaches the interface as a code.
const KNOWN_CODES: &[&str] = &[
    "accessibilityRequestFailed", "accountUnknown", "adminCanceled", "appIconFailed", "appNotFound",
    "appNotRunning", "appOpenFailed",
    "argumentsInvalid", "authorizedKeysIo", "authorizedKeysUnsafe", "badHello", "badMessage",
    "badRequest", "busy", "caffeinateExited", "caffeinateInvalidDuration",
    "caffeinateInvalidPid", "caffeinateNoFlags", "caffeinateProcessListFailed",
    "caffeinateProcessUnavailable", "caffeinateReconfigureFailed", "caffeinateStartFailed",
    "caffeinateUnsupported", "commandNotAllowed", "destinationInvalid", "deviceIdInvalid",
    "devicesFileInvalid", "devicesFull", "discoveryFailed", "expiryInvalid",
    "idleMonitorUnavailable", "invalidSchedule", "lidAuthorizationCanceled",
    "lidAuthorizationFailed", "lidChangeFailed", "lidNeedsLocalPermission", "lineTooLong",
    "localOnly", "loginLaunchCheckFailed", "loginLaunchRequiresBundle",
    "loginLaunchSaveFailed", "networkInvalid", "notPaired", "notPermitted", "pairingBusy",
    "pairingClosed", "pairingCodeInvalid", "pairingFailed", "pairingSaveFailed",
    "peerInvalid", "peersFileInvalid", "peersFull", "permissionRequired", "proxyExited",
    "proxyLaunchFailed", "proxyLaunchUnsupported", "proxyPortInUse", "proxyServerInvalid",
    "proxyStartFailed", "remoteAccessChangeFailed", "remoteDirInvalid", "remoteDirOwner",
    "remoteFailed", "remoteWindowFailed", "remoteFileInvalid", "remoteFileUnsafe", "remoteIo", "remoteUnavailable",
    "responseTooLarge", "responseTooLong", "saveFailed", "sharingSettingsFailed",
    "socketBindFailed", "socketPathBusy", "socketPathTooLong", "sshDenied", "sshFailed",
    "sshHostKeyChanged", "sshHostKeyUnknown", "sshInputTooLarge", "sshOutputTooLarge",
    "sshTimeout", "sshUnreachable", "unsupported", "updateChanged", "updateCheckFailed",
    "updateFailed", "workerExited", "workerUnavailable",
    "filesChunkInvalid", "filesDenied", "filesDiskFull", "filesExists", "filesFolderInvalid", "filesIncomplete", "filesIo", "filesIsFolder", "filesNotEmpty", "filesNotFolder", "filesNotFound", "filesNotRegular", "filesOffsetInvalid", "filesOutside", "filesPathInvalid", "filesProtected", "filesRootUnknown", "filesTooLarge", "filesTooMany", "filesTooManyUploads", "filesUploadUnknown",
];

/// Errors after which the other side ends the session, so the next request opens a new one.
const ENDS_SESSION: &[&str] = &[
    "appNotRunning", "argumentsInvalid", "badHello", "busy", "commandNotAllowed", "lineTooLong",
    "notPaired", "remoteUnavailable", "responseTooLong",
];

/// The error word as one of ours. A word that we do not know is `remoteFailed`.
fn known(word: &str) -> &'static str {
    KNOWN_CODES
        .iter()
        .find(|code| **code == word)
        .copied()
        .unwrap_or("remoteFailed")
}

/// How a link ends and why. The transport knows the reason, for ssh from its error text.
pub trait End: Send + Sync {
    /// Ends the link now. The pending read and write fail.
    fn close(&self);
    /// The error word that explains why the link ended.
    fn reason(&self) -> &'static str;
}

/// What a transport opens: where lines go in, where they come out, and how the link ends.
pub struct Pipes {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub end: Arc<dyn End>,
}

pub trait Transport: Send + Sync + 'static {
    /// Starts a link to the device. A connection that fails shows later, as the end of the pipes.
    fn open(&self, device: &Controlled) -> Result<Pipes, &'static str>;
}

enum Incoming {
    Line(Vec<u8>),
    TooLong,
    Ended,
}

enum Dead {
    Ended,
    Timeout,
    TooLong,
    Malformed,
}

/// One open link. A thread reads lines and another writes them, so a stalled device cannot
/// block the caller, which waits only on the channel and its clock.
struct Link {
    requests: mpsc::Sender<Vec<u8>>,
    responses: mpsc::Receiver<Incoming>,
    end: Arc<dyn End>,
}

impl Link {
    fn new(pipes: Pipes) -> Self {
        let Pipes {
            reader,
            mut writer,
            end,
        } = pipes;
        let (requests, outgoing) = mpsc::channel::<Vec<u8>>();
        let (incoming, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in outgoing {
                if writer.write_all(&line).and_then(|()| writer.flush()).is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                let item = match read_line(&mut reader, MAX_RESPONSE_LINE) {
                    Ok(Some(line)) => Incoming::Line(line),
                    Err(LineError::TooLong) => Incoming::TooLong,
                    Ok(None) | Err(LineError::Io) => Incoming::Ended,
                };
                let last = !matches!(item, Incoming::Line(_));
                if incoming.send(item).is_err() || last {
                    break;
                }
            }
        });
        Self {
            requests,
            responses,
            end,
        }
    }

    /// Sends one line and waits for one. `Ok` holds what the device answered, `Err` that the link is no use.
    fn exchange(
        &self,
        line: Vec<u8>,
        timeout: Duration,
    ) -> Result<Result<Value, &'static str>, Dead> {
        self.requests.send(line).map_err(|_| Dead::Ended)?;
        match self.responses.recv_timeout(timeout) {
            Ok(Incoming::Line(line)) => parse(&line).ok_or(Dead::Malformed),
            Ok(Incoming::TooLong) => Err(Dead::TooLong),
            Ok(Incoming::Ended) | Err(mpsc::RecvTimeoutError::Disconnected) => Err(Dead::Ended),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Dead::Timeout),
        }
    }

    /// Closes the link and says why it was no use.
    fn bury(self, dead: &Dead) -> &'static str {
        let code = match dead {
            Dead::Ended => self.end.reason(),
            Dead::Timeout => "sshTimeout",
            Dead::TooLong => "responseTooLong",
            Dead::Malformed => "remoteFailed",
        };
        self.end.close();
        code
    }
}

/// `{"ok":…}` or `{"error":"<word>"}` and nothing else. Anything more or less is not an answer.
fn parse(line: &[u8]) -> Option<Result<Value, &'static str>> {
    let Value::Object(mut object) = serde_json::from_slice(line).ok()? else {
        return None;
    };
    if object.len() != 1 {
        return None;
    }
    if let Some(value) = object.remove("ok") {
        return Some(Ok(value));
    }
    match object.remove("error")? {
        Value::String(word) => Some(Err(known(&word))),
        _ => None,
    }
}

#[derive(Default)]
struct State {
    link: Option<Link>,
    /// The address the link was opened for. A device that moved gets a new link.
    address: Option<(String, String, u16)>,
    /// Until this time, a new link is not tried, and this error is the answer.
    retry: Option<(Instant, &'static str)>,
}

/// The session with one device. Calls take turns, so there is one request in flight.
pub struct Client {
    transport: Arc<dyn Transport>,
    state: Mutex<State>,
    /// The end of the open link, kept apart so `close` never waits for a call in flight.
    end: Mutex<Option<Arc<dyn End>>>,
    live: AtomicBool,
    closed: AtomicBool,
    timeout: Duration,
    backoff: Duration,
}

impl Client {
    fn new(transport: Arc<dyn Transport>, timeout: Duration, backoff: Duration) -> Self {
        Self {
            transport,
            state: Mutex::default(),
            end: Mutex::new(None),
            live: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            timeout,
            backoff,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn set_end(&self, end: Option<Arc<dyn End>>) {
        *self.end.lock().unwrap_or_else(|error| error.into_inner()) = end;
    }

    /// True while a link is open and its last request worked.
    pub fn connected(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    /// Ends the link, also one that waits for an answer.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.live.store(false, Ordering::Release);
        let end = self
            .end
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(end) = end {
            end.close();
        }
    }

    /// Sends one request and returns what the device answered. It waits, so call it off the
    /// main thread. A link that ended is opened again once for the same request.
    pub fn call(&self, device: &Controlled, request: &Value) -> Result<Value, &'static str> {
        let line = format!("{}\n", serde_json::json!({ "request": request })).into_bytes();
        if line.len() > MAX_DISPATCH_LINE {
            return Err("lineTooLong");
        }
        let mut state = self.lock();
        let address = (device.user.clone(), device.host.clone(), device.port);
        if state.address.as_ref() != Some(&address) {
            if let Some(link) = state.link.take() {
                link.end.close();
            }
            state.retry = None;
            state.address = Some(address);
        }
        for attempt in 0..2 {
            if self.closed.load(Ordering::Acquire) {
                return Err("notPaired");
            }
            let reused = state.link.is_some();
            if !reused {
                if let Some((until, code)) = state.retry {
                    if Instant::now() < until {
                        return Err(code);
                    }
                }
                match self.transport.open(device) {
                    Ok(pipes) => {
                        self.set_end(Some(pipes.end.clone()));
                        state.link = Some(Link::new(pipes));
                    }
                    Err(code) => {
                        state.retry = Some((Instant::now() + self.backoff, code));
                        return Err(code);
                    }
                }
            }
            let Some(link) = state.link.as_ref() else {
                return Err("remoteFailed");
            };
            match link.exchange(line.clone(), self.timeout) {
                Ok(answer) => {
                    self.live.store(true, Ordering::Release);
                    match answer {
                        Err(code) if ENDS_SESSION.contains(&code) => {
                            self.drop_link(&mut state, &Dead::Ended);
                            state.retry = Some((Instant::now() + self.backoff, code));
                            return Err(code);
                        }
                        answer => {
                            state.retry = None;
                            return answer;
                        }
                    }
                }
                Err(dead) => {
                    let code = self.drop_link(&mut state, &dead);
                    if reused && attempt == 0 {
                        continue;
                    }
                    state.retry = Some((Instant::now() + self.backoff, code));
                    return Err(code);
                }
            }
        }
        Err("remoteFailed")
    }

    fn drop_link(&self, state: &mut State, dead: &Dead) -> &'static str {
        self.live.store(false, Ordering::Release);
        self.set_end(None);
        state.link.take().map_or("remoteFailed", |link| link.bury(dead))
    }
}

/// The sessions of all devices, by device id.
pub struct Clients {
    transport: Arc<dyn Transport>,
    timeout: Duration,
    backoff: Duration,
    open: Mutex<HashMap<String, Arc<Client>>>,
}

impl Clients {
    pub fn new(transport: impl Transport) -> Self {
        Self::with_limits(transport, REQUEST_TIMEOUT, BACKOFF)
    }

    pub fn with_limits(transport: impl Transport, timeout: Duration, backoff: Duration) -> Self {
        Self {
            transport: Arc::new(transport),
            timeout,
            backoff,
            open: Mutex::default(),
        }
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Client>>> {
        self.open.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Sends a request to a device. It waits, so call it off the main thread.
    pub fn call(&self, device: &Controlled, request: &Value) -> Result<Value, &'static str> {
        let client = self
            .map()
            .entry(device.id.clone())
            .or_insert_with(|| {
                Arc::new(Client::new(self.transport.clone(), self.timeout, self.backoff))
            })
            .clone();
        client.call(device, request)
    }

    /// True while the device has an open session.
    pub fn connected(&self, id: &str) -> bool {
        self.map().get(id).is_some_and(|client| client.connected())
    }

    /// Ends the session with one device, for example when it is forgotten.
    pub fn close(&self, id: &str) {
        let client = self.map().remove(id);
        if let Some(client) = client {
            client.close();
        }
    }

    pub fn close_all(&self) {
        let all: Vec<_> = self.map().drain().map(|(_, client)| client).collect();
        for client in all {
            client.close();
        }
    }
}

/// The system ssh client, through the hardened command of `ssh`.
pub struct SshTransport {
    layout: Layout,
    configured: AtomicBool,
}

impl SshTransport {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            configured: AtomicBool::new(false),
        }
    }
}

struct SshEnd {
    child: Mutex<Child>,
    errors: Arc<Mutex<Vec<u8>>>,
}

impl End for SshEnd {
    fn close(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn reason(&self) -> &'static str {
        let start = Instant::now();
        loop {
            let Ok(mut child) = self.child.lock() else {
                return "sshFailed";
            };
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if start.elapsed() < Duration::from_secs(2) => {}
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
            drop(child);
            thread::sleep(Duration::from_millis(20));
        }
        // The thread that reads stderr ends with the pipe, a moment after ssh.
        thread::sleep(Duration::from_millis(50));
        let text = self
            .errors
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        ssh::classify_stderr(&text)
    }
}

impl Transport for SshTransport {
    fn open(&self, device: &Controlled) -> Result<Pipes, &'static str> {
        if !self.configured.load(Ordering::Acquire) {
            ssh::write_config(&self.layout)?;
            self.configured.store(true, Ordering::Release);
        }
        let destination = device.destination()?;
        let peer = DeviceId::parse(&device.id)?;
        let mut child = ssh::command(&self.layout, &destination, &peer, RemoteCommand::Dispatch)?
            .spawn()
            .map_err(|_| "sshFailed")?;
        let (Some(stdin), Some(stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("sshFailed");
        };
        let errors = Arc::new(Mutex::new(Vec::<u8>::new()));
        let collected = errors.clone();
        thread::spawn(move || {
            let mut chunk = [0u8; 1024];
            while let Ok(read) = stderr.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                if let Ok(mut all) = collected.lock() {
                    let room = (8 * 1024usize).saturating_sub(all.len());
                    all.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
        });
        Ok(Pipes {
            reader: Box::new(stdout),
            writer: Box::new(stdin),
            end: Arc::new(SshEnd {
                child: Mutex::new(child),
                errors,
            }),
        })
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::remote::helper::{self, Output, Role};
    use std::{os::unix::net::UnixStream, path::PathBuf, sync::atomic::AtomicUsize};

    /// Ends a link by shutting its socket, as the end of ssh would.
    pub struct SocketEnd(pub UnixStream);

    impl End for SocketEnd {
        fn close(&self) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }

        fn reason(&self) -> &'static str {
            "sshUnreachable"
        }
    }

    /// Runs the real helper in this process against the app socket, where ssh would run it.
    pub struct HelperTransport {
        pub socket: PathBuf,
        /// The id that the target knows this controller by. In the app, its `authorized_keys` line says it.
        pub peer: String,
        pub opened: Arc<AtomicUsize>,
    }

    impl Transport for HelperTransport {
        fn open(&self, _: &Controlled) -> Result<Pipes, &'static str> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            let (ours, theirs) = UnixStream::pair().map_err(|_| "sshFailed")?;
            let input = theirs.try_clone().map_err(|_| "sshFailed")?;
            let output = Output::new(theirs);
            let (socket, id) = (self.socket.clone(), DeviceId::parse(&self.peer)?);
            thread::spawn(move || {
                helper::session(&socket, &Role::Peer(id), input, output);
            });
            Ok(Pipes {
                reader: Box::new(ours.try_clone().map_err(|_| "sshFailed")?),
                writer: Box::new(ours.try_clone().map_err(|_| "sshFailed")?),
                end: Arc::new(SocketEnd(ours)),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{
        controlled::testing::device,
        peers::testing::{ID_A, ID_B},
    };
    use std::{os::unix::net::UnixStream, sync::atomic::AtomicUsize};

    /// Answers each request with the next canned line, as a device would.
    struct Script {
        answers: Mutex<Vec<Vec<u8>>>,
        opened: Arc<AtomicUsize>,
        hold: bool,
    }

    impl Script {
        fn of(answers: &[&[u8]]) -> (Self, Arc<AtomicUsize>) {
            let opened = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    answers: Mutex::new(answers.iter().rev().map(|line| line.to_vec()).collect()),
                    opened: opened.clone(),
                    hold: false,
                },
                opened,
            )
        }
    }

    impl Transport for Script {
        fn open(&self, _: &Controlled) -> Result<Pipes, &'static str> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            let (ours, theirs) = UnixStream::pair().map_err(|_| "sshFailed")?;
            let answers: Vec<Vec<u8>> = std::mem::take(&mut *self.answers.lock().unwrap());
            let hold = self.hold;
            let mut reader = BufReader::new(theirs.try_clone().unwrap());
            let mut writer = theirs;
            thread::spawn(move || {
                let mut answers = answers;
                while read_line(&mut reader, MAX_DISPATCH_LINE)
                    .ok()
                    .flatten()
                    .is_some()
                {
                    if hold {
                        thread::sleep(Duration::from_secs(5));
                        return;
                    }
                    let Some(answer) = answers.pop() else { return };
                    let _ = writer.write_all(&answer);
                    let _ = writer.write_all(b"\n");
                }
            });
            Ok(Pipes {
                reader: Box::new(ours.try_clone().unwrap()),
                writer: Box::new(ours.try_clone().unwrap()),
                end: Arc::new(testing::SocketEnd(ours)),
            })
        }
    }

    fn status() -> Value {
        serde_json::json!({"command": "getStatuses"})
    }

    fn quick(transport: impl Transport) -> Clients {
        Clients::with_limits(transport, Duration::from_millis(300), Duration::from_millis(200))
    }

    #[test]
    fn a_device_answer_is_ok_or_a_known_error_and_nothing_else() {
        assert_eq!(parse(b"{\"ok\":{\"a\":1}}"), Some(Ok(serde_json::json!({"a": 1}))));
        assert_eq!(parse(b"{\"ok\":null}"), Some(Ok(Value::Null)));
        assert_eq!(parse(b"{\"error\":\"localOnly\"}"), Some(Err("localOnly")));
        assert_eq!(parse(b"{\"error\":\"somethingNew\"}"), Some(Err("remoteFailed")));
        assert_eq!(parse(b"{\"error\":\"<script>\"}"), Some(Err("remoteFailed")));
        for bad in [
            &b"{\"ok\":1,\"error\":\"x\"}"[..],
            b"{\"ok\":1,\"extra\":2}",
            b"{}",
            b"[]",
            b"\"ok\"",
            b"{\"error\":5}",
            b"{\"other\":1}",
            b"not json",
            b"",
            b"\xff\xfe",
        ] {
            assert!(parse(bad).is_none(), "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn every_known_word_is_unique_and_the_session_enders_are_known() {
        let mut sorted = KNOWN_CODES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), KNOWN_CODES.len());
        for code in ENDS_SESSION {
            assert!(KNOWN_CODES.contains(code), "{code}");
        }
    }

    #[test]
    fn a_request_goes_out_as_one_line_and_the_answer_comes_back() {
        let (script, opened) = Script::of(&[b"{\"ok\":\"one\"}", b"{\"ok\":\"two\"}"]);
        let clients = quick(script);
        let device = device(ID_A, "Studio");
        assert_eq!(clients.call(&device, &status()), Ok(serde_json::json!("one")));
        assert!(clients.connected(ID_A));
        assert_eq!(clients.call(&device, &status()), Ok(serde_json::json!("two")));
        assert_eq!(opened.load(Ordering::SeqCst), 1, "one link serves both");
        clients.close(ID_A);
        assert!(!clients.connected(ID_A));
    }

    #[test]
    fn a_refusal_passes_through_and_the_link_stays() {
        let (script, opened) = Script::of(&[
            b"{\"error\":\"localOnly\"}",
            b"{\"error\":\"notPermitted\"}",
            b"{\"ok\":1}",
        ]);
        let clients = quick(script);
        let device = device(ID_A, "Studio");
        assert_eq!(clients.call(&device, &status()), Err("localOnly"));
        assert_eq!(clients.call(&device, &status()), Err("notPermitted"));
        assert_eq!(clients.call(&device, &status()), Ok(serde_json::json!(1)));
        assert_eq!(opened.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_unknown_error_word_is_remote_failed() {
        let (script, _) = Script::of(&[b"{\"error\":\"surprise\"}"]);
        let clients = quick(script);
        assert_eq!(
            clients.call(&device(ID_A, "Studio"), &status()),
            Err("remoteFailed")
        );
    }

    #[test]
    fn a_malformed_answer_is_refused_and_ends_the_link() {
        let (script, opened) = Script::of(&[b"{\"ok\":1,\"extra\":true}"]);
        let clients = quick(script);
        let device = device(ID_A, "Studio");
        assert_eq!(clients.call(&device, &status()), Err("remoteFailed"));
        assert!(!clients.connected(ID_A));
        assert_eq!(opened.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_oversized_answer_is_refused() {
        let mut big = b"{\"ok\":\"".to_vec();
        big.resize(MAX_RESPONSE_LINE + 10, b'a');
        big.extend_from_slice(b"\"}");
        let (script, _) = Script::of(&[&big]);
        let clients = quick(script);
        assert_eq!(
            clients.call(&device(ID_A, "Studio"), &status()),
            Err("responseTooLong")
        );
        // A line right under the cap is fine.
        let mut fits = b"{\"ok\":\"".to_vec();
        fits.resize(MAX_RESPONSE_LINE - 10, b'a');
        fits.extend_from_slice(b"\"}");
        let (script, _) = Script::of(&[&fits]);
        let clients = quick(script);
        assert!(clients.call(&device(ID_A, "Studio"), &status()).is_ok());
    }

    #[test]
    fn an_oversized_request_never_leaves() {
        let (script, opened) = Script::of(&[b"{\"ok\":1}"]);
        let clients = quick(script);
        let big = serde_json::json!({"command": "saveProxyApps", "args": {"apps": "a".repeat(MAX_DISPATCH_LINE)}});
        assert_eq!(
            clients.call(&device(ID_A, "Studio"), &big),
            Err("lineTooLong")
        );
        assert_eq!(opened.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_device_that_does_not_answer_times_out_and_is_not_tried_again_at_once() {
        let (mut script, opened) = Script::of(&[]);
        script.hold = true;
        let clients = quick(script);
        let device = device(ID_A, "Studio");
        let start = Instant::now();
        assert_eq!(clients.call(&device, &status()), Err("sshTimeout"));
        assert!(start.elapsed() < Duration::from_secs(2));
        // Inside the back-off the same error comes back without a new link.
        assert_eq!(clients.call(&device, &status()), Err("sshTimeout"));
        assert_eq!(opened.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_link_that_ended_is_opened_again_once_for_the_same_request() {
        // The first link answers once and ends. The second answers the retry.
        struct TwoLinks {
            opened: Arc<AtomicUsize>,
        }
        impl Transport for TwoLinks {
            fn open(&self, _: &Controlled) -> Result<Pipes, &'static str> {
                let number = self.opened.fetch_add(1, Ordering::SeqCst);
                let (ours, theirs) = UnixStream::pair().unwrap();
                let mut reader = BufReader::new(theirs.try_clone().unwrap());
                let mut writer = theirs;
                thread::spawn(move || {
                    while read_line(&mut reader, MAX_DISPATCH_LINE).ok().flatten().is_some() {
                        let answer = if number == 0 { "{\"ok\":\"first\"}\n" } else { "{\"ok\":\"again\"}\n" };
                        let _ = writer.write_all(answer.as_bytes());
                        if number == 0 {
                            let _ = writer.shutdown(std::net::Shutdown::Both);
                            return;
                        }
                    }
                });
                Ok(Pipes {
                    reader: Box::new(ours.try_clone().unwrap()),
                    writer: Box::new(ours.try_clone().unwrap()),
                    end: Arc::new(testing::SocketEnd(ours)),
                })
            }
        }
        let opened = Arc::new(AtomicUsize::new(0));
        let clients = quick(TwoLinks { opened: opened.clone() });
        let device = device(ID_A, "Studio");
        assert_eq!(clients.call(&device, &status()), Ok(serde_json::json!("first")));
        thread::sleep(Duration::from_millis(50));
        assert_eq!(clients.call(&device, &status()), Ok(serde_json::json!("again")));
        assert_eq!(opened.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_start_backs_off_and_then_tries_again() {
        struct Refuses(Arc<AtomicUsize>);
        impl Transport for Refuses {
            fn open(&self, _: &Controlled) -> Result<Pipes, &'static str> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err("sshFailed")
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        let clients = quick(Refuses(count.clone()));
        let device = device(ID_A, "Studio");
        assert_eq!(clients.call(&device, &status()), Err("sshFailed"));
        assert_eq!(clients.call(&device, &status()), Err("sshFailed"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        thread::sleep(Duration::from_millis(250));
        assert_eq!(clients.call(&device, &status()), Err("sshFailed"));
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_closed_session_does_not_reconnect_and_ends_the_waiting_call() {
        let (mut script, opened) = Script::of(&[]);
        script.hold = true;
        let clients = Arc::new(Clients::with_limits(
            script,
            Duration::from_secs(10),
            Duration::from_millis(200),
        ));
        let waiting = {
            let clients = clients.clone();
            thread::spawn(move || clients.call(&device(ID_B, "Studio"), &status()))
        };
        thread::sleep(Duration::from_millis(200));
        clients.close(ID_B);
        let result = waiting.join().unwrap();
        assert!(result.is_err());
        assert_eq!(opened.load(Ordering::SeqCst), 1, "no reconnect after close");
    }

    #[test]
    fn a_session_error_ends_the_link_and_waits_before_the_next() {
        let (script, opened) = Script::of(&[b"{\"error\":\"notPaired\"}"]);
        let clients = quick(script);
        let device = device(ID_A, "Studio");
        assert_eq!(clients.call(&device, &status()), Err("notPaired"));
        assert_eq!(clients.call(&device, &status()), Err("notPaired"));
        assert_eq!(opened.load(Ordering::SeqCst), 1);
        assert!(!clients.connected(ID_A));
    }
}
