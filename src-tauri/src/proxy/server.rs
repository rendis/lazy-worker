use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read, Write},
    net::{IpAddr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HEAD_LIMIT: usize = 64 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPT_INTERVAL: Duration = Duration::from_millis(100);
const PEER_MEMORY: Duration = Duration::from_secs(24 * 60 * 60);
const LOG_LIMIT: usize = 200;
const LOG_TARGET_LIMIT: usize = 120;

/// One request, kept in memory while the server runs.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: u64,
    /// Seconds since the Unix epoch.
    pub time: u64,
    pub peer: String,
    pub target: String,
    pub outcome: &'static str,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub address: String,
    pub name: Option<String>,
    pub connections: usize,
    pub idle_seconds: u64,
}

struct PeerState {
    connections: usize,
    last_seen: Instant,
}

struct Shared {
    stop: AtomicBool,
    allowed: Mutex<Vec<IpAddr>>,
    peers: Mutex<HashMap<IpAddr, PeerState>>,
    streams: Mutex<HashMap<u64, TcpStream>>,
    next_stream: AtomicU64,
    log: Mutex<VecDeque<LogEntry>>,
    next_entry: AtomicU64,
    // Tests reach a loopback upstream. A running proxy must keep its own machine private.
    loopback_targets: bool,
}

impl Shared {
    fn new(allowed: Vec<IpAddr>, loopback_targets: bool) -> Self {
        Self {
            stop: AtomicBool::new(false),
            allowed: Mutex::new(allowed),
            peers: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            next_stream: AtomicU64::new(0),
            log: Mutex::new(VecDeque::new()),
            next_entry: AtomicU64::new(0),
            loopback_targets,
        }
    }

    /// Keeps the newest entries only, so a server left on for days stays small.
    fn record(&self, peer: IpAddr, target: &str, outcome: &'static str) {
        let entry = LogEntry {
            id: self.next_entry.fetch_add(1, Ordering::Relaxed),
            time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
            peer: peer.to_string(),
            target: target.chars().take(LOG_TARGET_LIMIT).collect(),
            outcome,
        };
        if let Ok(mut log) = self.log.lock() {
            if log.len() == LOG_LIMIT {
                log.pop_back();
            }
            log.push_front(entry);
        }
    }
}

pub struct Server {
    shared: Arc<Shared>,
    port: u16,
    listener: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(port: u16, allowed: Vec<IpAddr>) -> Result<Self, &'static str> {
        Self::start_with(port, allowed, false)
    }

    fn start_with(
        port: u16,
        allowed: Vec<IpAddr>,
        loopback_targets: bool,
    ) -> Result<Self, &'static str> {
        // The IPv6 wildcard also accepts IPv4 on macOS, so a `.local` name works over both.
        let listener = TcpListener::bind((Ipv6Addr::UNSPECIFIED, port))
            .or_else(|_| TcpListener::bind(("0.0.0.0", port)))
            .map_err(|error| match error.kind() {
                io::ErrorKind::AddrInUse => "proxyPortInUse",
                _ => "proxyStartFailed",
            })?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "proxyStartFailed")?;
        let port = listener
            .local_addr()
            .map_err(|_| "proxyStartFailed")?
            .port();
        let shared = Arc::new(Shared::new(allowed, loopback_targets));
        let accepting = shared.clone();
        let listener = thread::spawn(move || accept(listener, accepting));
        Ok(Self {
            shared,
            port,
            listener: Some(listener),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn is_running(&self) -> bool {
        self.listener
            .as_ref()
            .is_some_and(|listener| !listener.is_finished())
    }

    pub fn set_allowed(&self, allowed: Vec<IpAddr>) {
        if let Ok(mut current) = self.shared.allowed.lock() {
            *current = allowed;
        }
    }

    /// The newest requests first.
    pub fn log(&self) -> Vec<LogEntry> {
        self.shared
            .log
            .lock()
            .map(|log| log.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn peers(&self) -> Vec<Peer> {
        let Ok(mut peers) = self.shared.peers.lock() else {
            return Vec::new();
        };
        peers.retain(|_, peer| peer.connections > 0 || peer.last_seen.elapsed() < PEER_MEMORY);
        let mut list: Vec<_> = peers
            .iter()
            .map(|(address, peer)| Peer {
                address: address.to_string(),
                name: None,
                connections: peer.connections,
                idle_seconds: peer.last_seen.elapsed().as_secs(),
            })
            .collect();
        list.sort_unstable_by(|a, b| {
            (b.connections > 0)
                .cmp(&(a.connections > 0))
                .then(a.idle_seconds.cmp(&b.idle_seconds))
                .then(a.address.cmp(&b.address))
        });
        list
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Ok(streams) = self.shared.streams.lock() {
            for stream in streams.values() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

fn accept(listener: TcpListener, shared: Arc<Shared>) {
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, address)) => {
                let shared = shared.clone();
                thread::spawn(move || {
                    let _ = serve(stream, address, &shared);
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_INTERVAL)
            }
            Err(_) => thread::sleep(ACCEPT_INTERVAL),
        }
    }
}

/// Counts one connection for its peer while it lives.
struct Visit<'a> {
    shared: &'a Shared,
    peer: IpAddr,
}

impl<'a> Visit<'a> {
    fn start(shared: &'a Shared, peer: IpAddr) -> Self {
        if let Ok(mut peers) = shared.peers.lock() {
            let state = peers.entry(peer).or_insert(PeerState {
                connections: 0,
                last_seen: Instant::now(),
            });
            state.connections += 1;
            state.last_seen = Instant::now();
        }
        Self { shared, peer }
    }
}

impl Drop for Visit<'_> {
    fn drop(&mut self) {
        if let Ok(mut peers) = self.shared.peers.lock() {
            if let Some(state) = peers.get_mut(&self.peer) {
                state.connections = state.connections.saturating_sub(1);
                state.last_seen = Instant::now();
            }
        }
    }
}

fn is_local_network(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_private() || address.is_link_local(),
        IpAddr::V6(address) => {
            let first = address.segments()[0];
            first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    }
}

/// An empty list admits the local network only, so a forwarded port stays closed to the internet.
fn permitted(peer: IpAddr, allowed: &[IpAddr]) -> bool {
    peer.is_loopback()
        || if allowed.is_empty() {
            is_local_network(peer)
        } else {
            allowed.contains(&peer)
        }
}

fn refuse(stream: &mut TcpStream, status: &str) -> io::Result<()> {
    stream.write_all(
        format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").as_bytes(),
    )
}

fn serve(mut client: TcpStream, address: SocketAddr, shared: &Shared) -> io::Result<()> {
    // Accepted sockets can inherit the listener's non-blocking mode.
    client.set_nonblocking(false)?;
    let peer = address.ip().to_canonical();
    let allowed = shared
        .allowed
        .lock()
        .map(|list| list.clone())
        .unwrap_or_default();
    if !permitted(peer, &allowed) {
        shared.record(peer, "", "denied");
        return refuse(&mut client, "403 Forbidden");
    }
    let _visit = Visit::start(shared, peer);

    client.set_read_timeout(Some(HEAD_TIMEOUT))?;
    let Some((head, rest)) = read_head(&mut client)? else {
        shared.record(peer, "", "invalid");
        return refuse(&mut client, "400 Bad Request");
    };
    let Some(request) = Request::parse(&head) else {
        shared.record(peer, "", "invalid");
        return refuse(&mut client, "400 Bad Request");
    };
    let target = format!("{}:{}", request.host, request.port);
    let mut upstream = match connect(&request.host, request.port, shared.loopback_targets) {
        Ok(upstream) => upstream,
        Err(Refusal::Forbidden) => {
            shared.record(peer, &target, "blocked");
            return refuse(&mut client, "403 Forbidden");
        }
        Err(Refusal::Unreachable) => {
            shared.record(peer, &target, "unreachable");
            return refuse(&mut client, "502 Bad Gateway");
        }
    };
    shared.record(peer, &target, "ok");
    match request.forward {
        Some(head) => upstream.write_all(head.as_bytes())?,
        None => client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?,
    }
    upstream.write_all(&rest)?;
    client.set_read_timeout(None)?;
    tunnel(client, upstream, shared)
}

fn read_head(stream: &mut TcpStream) -> io::Result<Option<(String, Vec<u8>)>> {
    let mut data = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(None);
        }
        data.extend_from_slice(&chunk[..read]);
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            let rest = data.split_off(end + 4);
            return Ok(String::from_utf8(data).ok().map(|head| (head, rest)));
        }
        if data.len() > HEAD_LIMIT {
            return Ok(None);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    host: String,
    port: u16,
    /// The rewritten head for a plain HTTP request. `None` asks for a tunnel.
    forward: Option<String>,
}

impl Request {
    fn parse(head: &str) -> Option<Self> {
        let mut lines = head.split("\r\n");
        let mut parts = lines.next()?.split(' ');
        let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || !version.starts_with("HTTP/1.") {
            return None;
        }
        if method.eq_ignore_ascii_case("CONNECT") {
            let (host, port) = split_authority(target, None)?;
            return Some(Self {
                host,
                port,
                forward: None,
            });
        }

        let location = target
            .get(..7)
            .filter(|scheme| scheme.eq_ignore_ascii_case("http://"))
            .map(|_| &target[7..])?;
        let (authority, rest) = location.split_at(location.find(['/', '?']).unwrap_or(location.len()));
        let (host, port) = split_authority(authority, Some(80))?;
        let path = if rest.starts_with('/') {
            rest.to_owned()
        } else {
            format!("/{rest}")
        };
        let mut forward = format!("{method} {path} {version}\r\n");
        for line in lines.take_while(|line| !line.is_empty()) {
            let name = line.split(':').next().unwrap_or_default().trim();
            // One request per connection keeps the relay free of HTTP framing rules.
            if !["proxy-connection", "proxy-authorization", "connection", "keep-alive"]
                .iter()
                .any(|hop| name.eq_ignore_ascii_case(hop))
            {
                forward.push_str(line);
                forward.push_str("\r\n");
            }
        }
        forward.push_str("Connection: close\r\n\r\n");
        Some(Self {
            host,
            port,
            forward: Some(forward),
        })
    }
}

fn split_authority(authority: &str, default_port: Option<u16>) -> Option<(String, u16)> {
    let authority = authority.rsplit('@').next()?;
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        Some(port) => port.parse().ok()?,
        None => default_port?,
    };
    (!host.is_empty() && port != 0).then(|| (host.to_owned(), port))
}

enum Refusal {
    Forbidden,
    Unreachable,
}

fn connect(host: &str, port: u16, loopback_targets: bool) -> Result<TcpStream, Refusal> {
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|_| Refusal::Unreachable)?
        .collect();
    let reachable: Vec<_> = addresses
        .iter()
        .filter(|address| {
            let ip = address.ip().to_canonical();
            loopback_targets || !(ip.is_loopback() || ip.is_unspecified())
        })
        .collect();
    if reachable.is_empty() {
        return Err(if addresses.is_empty() {
            Refusal::Unreachable
        } else {
            Refusal::Forbidden
        });
    }
    reachable
        .into_iter()
        .find_map(|address| TcpStream::connect_timeout(address, CONNECT_TIMEOUT).ok())
        .ok_or(Refusal::Unreachable)
}

fn tunnel(client: TcpStream, upstream: TcpStream, shared: &Shared) -> io::Result<()> {
    let ids = [client.try_clone()?, upstream.try_clone()?].map(|stream| {
        let id = shared.next_stream.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut streams) = shared.streams.lock() {
            streams.insert(id, stream);
        }
        id
    });
    // A stop that arrived before the streams were listed still closes them.
    if shared.stop.load(Ordering::SeqCst) {
        let _ = client.shutdown(Shutdown::Both);
        let _ = upstream.shutdown(Shutdown::Both);
    }

    let (mut client_reader, mut upstream_writer) = (client.try_clone()?, upstream.try_clone()?);
    let upload = thread::spawn(move || {
        let _ = io::copy(&mut client_reader, &mut upstream_writer);
        let _ = upstream_writer.shutdown(Shutdown::Write);
    });
    let (mut upstream_reader, mut client_writer) = (upstream, client);
    let _ = io::copy(&mut upstream_reader, &mut client_writer);
    // The origin is done, so a client that keeps its side open must not hold the relay.
    let _ = client_writer.shutdown(Shutdown::Both);
    let _ = upload.join();

    if let Ok(mut streams) = shared.streams.lock() {
        for id in ids {
            streams.remove(&id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    /// Accepts one connection, returns what it received, and answers with `reply`.
    fn upstream(reply: &'static [u8], expected: usize) -> (u16, JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut received = vec![0; expected];
            stream.read_exact(&mut received).unwrap();
            stream.write_all(reply).unwrap();
            received
        });
        (port, handle)
    }

    fn client(server: &Server) -> TcpStream {
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port())).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
    }

    #[test]
    fn an_empty_list_admits_only_the_local_network() {
        assert!(permitted(ip("192.168.1.33"), &[]));
        assert!(permitted(ip("10.0.0.7"), &[]));
        assert!(permitted(ip("169.254.10.1"), &[]));
        assert!(permitted(ip("fe80::1"), &[]));
        assert!(permitted(ip("127.0.0.1"), &[]));
        assert!(!permitted(ip("203.0.113.9"), &[]));
        assert!(!permitted(ip("2001:db8::1"), &[]));
    }

    #[test]
    fn the_log_keeps_only_the_newest_requests() {
        let shared = Shared::new(Vec::new(), false);
        for index in 0..LOG_LIMIT + 5 {
            shared.record(ip("192.168.1.33"), &format!("host-{index}:443"), "ok");
        }
        let log = shared.log.lock().unwrap();
        assert_eq!(log.len(), LOG_LIMIT);
        assert_eq!(log.front().unwrap().target, format!("host-{}:443", LOG_LIMIT + 4));
        assert_eq!(log.back().unwrap().target, "host-5:443");
    }

    #[test]
    fn a_list_admits_only_its_addresses_and_this_machine() {
        let allowed = [ip("192.168.1.33")];
        assert!(permitted(ip("192.168.1.33"), &allowed));
        assert!(permitted(ip("::1"), &allowed));
        assert!(!permitted(ip("192.168.1.50"), &allowed));
    }

    #[test]
    fn connect_requests_ask_for_a_tunnel() {
        assert_eq!(
            Request::parse("CONNECT api.example.com:443 HTTP/1.1\r\nHost: api.example.com:443\r\n\r\n"),
            Some(Request {
                host: "api.example.com".into(),
                port: 443,
                forward: None
            })
        );
        assert_eq!(
            Request::parse("CONNECT [2001:db8::1]:443 HTTP/1.1\r\n\r\n").map(|request| request.host),
            Some("2001:db8::1".into())
        );
        assert_eq!(Request::parse("CONNECT example.com HTTP/1.1\r\n\r\n"), None);
    }

    #[test]
    fn plain_requests_are_rewritten_for_the_origin() {
        let request = Request::parse(
            "GET http://example.com:8080/a?b=1 HTTP/1.1\r\nHost: example.com:8080\r\nProxy-Connection: keep-alive\r\nProxy-Authorization: Basic x\r\nAccept: */*\r\n\r\n",
        )
        .unwrap();
        assert_eq!((request.host.as_str(), request.port), ("example.com", 8080));
        assert_eq!(
            request.forward.as_deref(),
            Some("GET /a?b=1 HTTP/1.1\r\nHost: example.com:8080\r\nAccept: */*\r\nConnection: close\r\n\r\n")
        );
        assert_eq!(
            Request::parse("GET http://example.com HTTP/1.1\r\n\r\n").and_then(|request| request.forward),
            Some("GET / HTTP/1.1\r\nConnection: close\r\n\r\n".into())
        );
        assert_eq!(Request::parse("GET /local HTTP/1.1\r\n\r\n"), None);
        assert_eq!(Request::parse("GET https://example.com/ HTTP/1.1\r\n\r\n"), None);
    }

    #[test]
    fn a_tunnel_carries_bytes_both_ways_and_reports_its_peer() {
        let (port, origin) = upstream(b"pong", 4);
        let server = Server::start_with(0, Vec::new(), true).unwrap();
        let mut stream = client(&server);
        stream
            .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").as_bytes())
            .unwrap();
        let mut established = [0; 39];
        stream.read_exact(&mut established).unwrap();
        assert_eq!(&established, b"HTTP/1.1 200 Connection Established\r\n\r\n");

        stream.write_all(b"ping").unwrap();
        let mut reply = [0; 4];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"pong");
        assert_eq!(origin.join().unwrap(), b"ping");

        let log = server.log();
        assert_eq!(log.len(), 1);
        assert_eq!(
            (log[0].peer.as_str(), log[0].target.as_str(), log[0].outcome),
            ("127.0.0.1", format!("127.0.0.1:{port}").as_str(), "ok")
        );

        let peers = server.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].address, "127.0.0.1");
    }

    #[test]
    fn a_plain_request_reaches_the_origin_in_origin_form() {
        let expected = "GET /status HTTP/1.1\r\nHost: origin\r\nConnection: close\r\n\r\n";
        let (port, origin) = upstream(b"HTTP/1.1 204 No Content\r\n\r\n", expected.len());
        let server = Server::start_with(0, Vec::new(), true).unwrap();
        let mut stream = client(&server);
        stream
            .write_all(
                format!("GET http://127.0.0.1:{port}/status HTTP/1.1\r\nHost: origin\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert_eq!(reply, "HTTP/1.1 204 No Content\r\n\r\n");
        assert_eq!(origin.join().unwrap(), expected.as_bytes());
    }

    #[test]
    fn this_machine_is_not_a_target_and_stopping_closes_open_tunnels() {
        let server = Server::start(0, Vec::new()).unwrap();
        let mut stream = client(&server);
        stream
            .write_all(b"CONNECT 127.0.0.1:22 HTTP/1.1\r\n\r\n")
            .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 403 Forbidden"));
        assert_eq!(server.log()[0].outcome, "blocked");

        let origin = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = origin.local_addr().unwrap().port();
        let server = Server::start_with(0, Vec::new(), true).unwrap();
        let mut stream = client(&server);
        stream
            .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").as_bytes())
            .unwrap();
        let mut established = [0; 39];
        stream.read_exact(&mut established).unwrap();
        let (held, _) = origin.accept().unwrap();
        drop(server);
        assert_eq!(stream.read(&mut [0; 1]).unwrap(), 0);
        drop(held);
    }
}
