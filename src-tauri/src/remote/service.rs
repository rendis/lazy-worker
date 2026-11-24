//! The target side of remote control as the app uses it: the peers, the log, the managed lines,
//! and the socket server, kept in step when a device is removed or all access is cut.

use super::{
    authorized_keys::AuthorizedKeys,
    client::{Clients, SshTransport},
    controlled::{Controlled, Store},
    destination::Destination,
    icons::Icons,
    identity::{self, DeviceId},
    known_hosts,
    log::{Entry, Log},
    pair_client,
    pake::{Own, OwnDevice},
    pairing::{Opened, Pairing},
    peers::{clean_name, PeerPermissions, Peers},
    now,
    server::{Handler, Server},
    shell::{self, Watch},
    ssh, Layout,
};
use crate::{
    apps,
    devices::{self, Controller, KeysWarning, Remote, ShellGrant, ShellUntil, Socket},
    dispatch::{self, Origin, Request},
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

/// How often the grants are checked against the wall clock, and so how late a shell can outlive its end.
const TICK: Duration = Duration::from_secs(2);
/// The longest grant that can be asked for, in minutes: one day.
pub const MAX_SHELL_MINUTES: u32 = 1440;
use tauri::{AppHandle, Manager};

/// Runs a remote request through the dispatcher that the webview uses.
struct Dispatcher(AppHandle);

impl Handler for Dispatcher {
    fn handle(&self, origin: Origin, request: Request) -> Result<Value, &'static str> {
        tauri::async_runtime::block_on(dispatch::handle(&self.0, origin, request))
    }

    fn shell_started(&self, peer: &str) {
        if let Some(service) = self.0.try_state::<Service>() {
            service.shell_started(peer);
        }
    }

    fn shell_ended(&self, peer: &str) {
        if let Some(service) = self.0.try_state::<Service>() {
            service.shell_ended(peer);
        }
    }
}

struct Running {
    layout: Layout,
    peers: Arc<Peers>,
    log: Arc<Log>,
    keys: Result<AuthorizedKeys, &'static str>,
    pairing: Arc<Pairing>,
    own: OwnDevice,
    /// The devices that this one controls. An error code when the file could not be opened.
    controlled: Result<Store, &'static str>,
    /// The open sessions with those devices.
    clients: Clients,
    /// The icons of the applications of those devices.
    icons: Result<Icons, &'static str>,
    server: Result<Server, &'static str>,
    /// The open shell sessions and the changes of `~/.ssh/authorized_keys` seen during them.
    watch: Watch,
    /// The time of the last comparison of that file during a session.
    last_keys_check: AtomicU64,
    /// The devices whose line could not be written when their access shrank or ended. Each tick tries again.
    line_needs_rewrite: Mutex<HashSet<String>>,
}

/// What forgetting a device leaves undone: our key stays in its `authorized_keys`, and only that
/// device can remove it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Forgotten {
    pub remove_on_device: bool,
}

/// The longest version text that an icon is kept for.
const MAX_VERSION: usize = 64;

/// The bytes of the icon in the answer of a device: `{"png": "<base64>"}` and nothing else.
fn decode_icon(answer: &Value) -> Result<Vec<u8>, &'static str> {
    use base64::Engine;
    let text = answer
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("png"))
        .and_then(Value::as_str)
        // Base64 grows the bytes by a third. The bytes are checked again before they are kept.
        .filter(|text| text.len() <= 4 * super::icons::MAX_BYTES / 3 + 8)
        .ok_or("appIconFailed")?;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| "appIconFailed")
}

/// The name of a device that the system does not name.
const UNNAMED: &str = "Unnamed device";

/// This device as a party of a pairing. It creates the identity when there is none.
fn own_device(layout: &Layout) -> Result<Own, &'static str> {
    let identity = identity::load_or_create(layout)?;
    let device = devices::this_device();
    let name = clean_name(&device.name);
    Ok(Own {
        id: identity.device_id,
        name: if name.is_empty() { UNNAMED.to_owned() } else { name },
        kind: device.kind,
        public_key: identity.public_key,
    })
}

/// Managed by the app. Holds an error code when the files could not be opened, so the Devices
/// view says why and no request is served.
pub struct Service(Result<Running, &'static str>);

impl Service {
    /// Opens the stores and starts the socket. Call it once, after the instance lock.
    pub fn start(app: &AppHandle, config_dir: &std::path::Path) -> Self {
        let home = app.path().home_dir().ok();
        let helper = std::env::current_exe().and_then(std::fs::canonicalize).ok();
        let layout = Layout::open(config_dir, &std::env::temp_dir());
        let service =
            Self(layout.and_then(|layout| Self::open(&layout, home, helper, Dispatcher(app.clone()))));
        // The wall clock ends the grants. A sleep of the Mac only delays the next tick.
        if service.0.is_ok() {
            let handle = app.clone();
            let _ = std::thread::Builder::new().name("shell-timer".into()).spawn(move || loop {
                std::thread::sleep(TICK);
                if let Some(service) = handle.try_state::<Service>() {
                    service.tick(now());
                }
            });
        }
        service
    }

    fn open(
        layout: &Layout,
        home: Option<PathBuf>,
        helper: Option<PathBuf>,
        handler: impl Handler,
    ) -> Result<Running, &'static str> {
        let peers = Arc::new(Peers::open(layout.peers_file())?);
        let log = Arc::new(Log::open(layout.log_file())?);
        let keys = match (home, helper) {
            (Some(home), Some(helper)) => AuthorizedKeys::new(home, helper),
            _ => Err("helperPathInvalid"),
        };
        // A grant that ended while the app was not running is revoked before anything else. The
        // lines are rendered again right after, so they lose `pty` with it.
        for id in peers.shell_expired(now()) {
            if let Some(peer) = peers.get(&id) {
                if peers.set_shell(&id, None).is_ok() {
                    let _ = log.append(&Entry::new(Some(&id), Some(&peer.name), "shellExpired", "ok"));
                }
            }
        }
        // After a move of the app, the lines must name the new program.
        if let Ok(keys) = &keys {
            if !peers.list().is_empty() {
                let _ = keys.refresh_all(&peers.list(), &peers.networks());
            }
        }
        // A pairing line that is still there belongs to a window that ended with the last run.
        if let Ok(keys) = &keys {
            let _ = keys.remove_pairing();
        }
        let own: OwnDevice = {
            let layout = layout.clone();
            Arc::new(move || own_device(&layout))
        };
        let pairing = Arc::new(Pairing::new(
            peers.clone(),
            log.clone(),
            keys.clone(),
            own.clone(),
        ));
        let controlled = Store::open(layout.devices_file());
        let server = Server::start(
            layout.socket_file(),
            peers.clone(),
            log.clone(),
            pairing.clone(),
            handler,
        );
        Ok(Running {
            layout: layout.clone(),
            peers,
            log,
            keys,
            pairing,
            own,
            controlled,
            clients: Clients::new(SshTransport::new(layout.clone())),
            icons: Icons::open(layout.icons_dir()),
            server,
            watch: Watch::default(),
            last_keys_check: AtomicU64::new(now()),
            line_needs_rewrite: Mutex::default(),
        })
    }

    #[cfg(test)]
    fn for_test(layout: &Layout, home: PathBuf, handler: impl Handler) -> Self {
        let helper = PathBuf::from("/Applications/Lazy Worker.app/Contents/MacOS/lazy-worker");
        Self(Self::open(layout, Some(home), Some(helper), handler))
    }

    fn running(&self) -> Result<&Running, &'static str> {
        self.0.as_ref().map_err(|code| *code)
    }

    pub fn socket(&self) -> Socket {
        match &self.0 {
            Err(error)
            | Ok(Running {
                server: Err(error), ..
            }) => Socket::unavailable(error),
            // Without a safe helper no line can be written, so no device can reach the socket.
            Ok(running) => match running
                .keys
                .as_ref()
                .map_err(|code| *code)
                .and_then(|keys| keys.trusted_helper())
            {
                Ok(_) => Socket::listening(),
                Err(code) => Socket::unavailable(code),
            },
        }
    }

    /// A device that this one controls.
    pub fn device(&self, id: &str) -> Option<Controlled> {
        match &self.0 {
            Ok(Running {
                controlled: Ok(store),
                ..
            }) => store.get(id),
            _ => None,
        }
    }

    /// Sends a request to a device that this one controls, and returns its answer as it came.
    /// It waits for the device, so call it off the main thread.
    pub fn forward(&self, id: &str, request: &Value) -> Result<Value, &'static str> {
        let running = self.running()?;
        let device = self.device(id).ok_or("notPermitted")?;
        running.clients.call(&device, request)
    }

    /// Where the icon of an application of a device is kept. It asks the device for the icon when
    /// none is kept, and drops one that is not a small PNG. It waits for the device, so call it
    /// off the main thread.
    pub fn cached_icon(
        &self,
        id: &str,
        bundle_id: &str,
        version: &str,
    ) -> Result<String, &'static str> {
        DeviceId::parse(id)?;
        if !apps::valid_bundle_id(bundle_id) || version.len() > MAX_VERSION {
            return Err("badRequest");
        }
        let running = self.running()?;
        let icons = running.icons.as_ref().map_err(|code| *code)?;
        self.device(id).ok_or("notPermitted")?;
        let path = match icons.cached(id, bundle_id, version) {
            Some(path) => path,
            None => {
                let answer = self.forward(
                    id,
                    &serde_json::json!({"command": "getAppIcon", "args": {"bundleId": bundle_id}}),
                )?;
                let png = decode_icon(&answer)?;
                icons.store(id, bundle_id, version, &png)?
            }
        };
        path.to_str().map(str::to_owned).ok_or("remoteIo")
    }

    /// Saves the applications that the Devices view shows first for a device that this one controls.
    pub fn set_pinned(&self, id: &str, bundle_ids: &[String]) -> Result<(), &'static str> {
        DeviceId::parse(id)?;
        let store = self.running()?.controlled.as_ref().map_err(|code| *code)?;
        store.set_pinned(id, bundle_ids)
    }

    /// This device as the other devices see it. It reads the system, so call it off the main thread.
    pub fn own_device(&self) -> impl FnOnce() -> Result<Own, &'static str> {
        let own = self.running().map(|running| running.own.clone());
        move || own.and_then(|own| own())
    }

    /// The name that a paired device has now.
    pub fn peer_name(&self, id: &str) -> Option<String> {
        self.running().ok()?.peers.get(id).map(|peer| peer.name)
    }

    /// Adds an entry to the log of remote access, with the name that the device has now.
    pub fn record(&self, mut entry: Entry) {
        let Ok(running) = self.running() else { return };
        if entry.name.is_none() {
            entry.name = entry.peer.as_deref().and_then(|id| running.peers.get(id)).map(|peer| peer.name);
        }
        let _ = running.log.append(&entry);
    }

    pub fn controllers(&self) -> Vec<Controller> {
        let Ok(running) = self.running() else {
            return Vec::new();
        };
        let connected = running
            .server
            .as_ref()
            .map(Server::connected)
            .unwrap_or_default();
        running
            .peers
            .list()
            .into_iter()
            .map(|peer| Controller::new(&peer, connected.contains(&peer.id)))
            .collect()
    }

    /// Returns whether the device had Actions and has lost them, so the caller can end what it started.
    pub fn set_permissions(
        &self,
        id: &str,
        control: bool,
        applications: bool,
        actions: bool,
    ) -> Result<bool, &'static str> {
        DeviceId::parse(id)?;
        let running = self.running()?;
        let before = running.peers.get(id).ok_or("notPaired")?;
        running.peers.set_permissions(id, control, applications, actions)?;
        let _ = running.log.append(&Entry::new(Some(id), Some(&before.name), "peerPermissions", "ok"));
        Ok(before.permissions.actions && !actions)
    }

    /// Whether a device is paired and may run actions here.
    pub fn peer_may_run_actions(&self, id: &str) -> bool {
        self.running()
            .ok()
            .and_then(|running| running.peers.get(id))
            .is_some_and(|peer| peer.permissions.actions)
    }

    /// Notes a line that must be written again, and tries on each tick until it works.
    fn line_failed(running: &Running, id: &str) {
        running
            .line_needs_rewrite
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id.to_owned());
    }

    fn retry_lines(&self, running: &Running) {
        let ids: Vec<String> = running
            .line_needs_rewrite
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .cloned()
            .collect();
        for id in ids {
            // A device that is gone has its line removed. One that is still paired has it written as its grant says.
            let outcome = if running.peers.get(&id).is_some() {
                self.rewrite_line(running, &id)
            } else {
                running.keys.as_ref().map_err(|code| *code).and_then(|keys| keys.remove(&id))
            };
            if outcome.is_ok() {
                running
                    .line_needs_rewrite
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&id);
            }
        }
    }

    /// The device loses access first, in memory and on its open connections, and then its line goes.
    /// A line that cannot be removed is reported, and the device is gone either way.
    pub fn remove_peer(&self, id: &str) -> Result<(), &'static str> {
        DeviceId::parse(id)?;
        let running = self.running()?;
        let peer = running.peers.remove(id)?.ok_or("notPaired")?;
        if let Ok(server) = &running.server {
            server.close_peer(id);
        }
        let outcome = running
            .keys
            .as_ref()
            .map_err(|code| *code)
            .and_then(|keys| keys.remove(id));
        if outcome.is_err() {
            Self::line_failed(running, id);
        }
        let _ = running.log.append(&Entry::new(
            Some(id),
            Some(&peer.name),
            "removePeer",
            outcome.err().unwrap_or("ok"),
        ));
        outcome
    }

    /// Grants a full shell to a device for `minutes` (1 to 1440), or until it is turned off. The
    /// line of the device gets `pty` first in memory and then on disk, and a line that cannot be
    /// written leaves the grant as it was.
    pub fn grant_shell(&self, id: &str, minutes: Option<u32>) -> Result<ShellGrant, &'static str> {
        DeviceId::parse(id)?;
        if minutes.is_some_and(|minutes| !(1..=MAX_SHELL_MINUTES).contains(&minutes)) {
            return Err("shellDurationInvalid");
        }
        let running = self.running()?;
        let name = running.peers.get(id).ok_or("notPaired")?.name;
        let until = minutes.map_or(ShellUntil::OFF, |minutes| {
            ShellUntil::At(now() + u64::from(minutes) * 60)
        });
        let previous = running.peers.set_shell(id, Some(until))?;
        let outcome = self.rewrite_line(running, id);
        if let Err(code) = outcome {
            let _ = running.peers.set_shell(id, previous);
            let _ = running.log.append(&Entry::new(Some(id), Some(&name), "shellGranted", code));
            return Err(code);
        }
        let _ = running.log.append(&Entry::new(Some(id), Some(&name), "shellGranted", "ok"));
        Ok(ShellGrant { until })
    }

    /// Ends the shell of a device now.
    pub fn revoke_shell(&self, id: &str) -> Result<(), &'static str> {
        DeviceId::parse(id)?;
        self.end_shell(self.running()?, id, "shellRevoked")
    }

    /// The shell that a device has now, as it is told in `GetSession`.
    pub fn shell_of(&self, id: &str) -> Option<ShellGrant> {
        let peer = self.running().ok()?.peers.get(id)?;
        peer.shell_until
            .filter(|until| until.active(now()))
            .map(|until| ShellGrant { until })
    }

    /// Writes the line of a peer again, with `pty` or without it as its grant says.
    fn rewrite_line(&self, running: &Running, id: &str) -> Result<(), &'static str> {
        let peer = running.peers.get(id).ok_or("notPaired")?;
        running
            .keys
            .as_ref()
            .map_err(|code| *code)
            .and_then(|keys| keys.install(&peer, &running.peers.networks()))
    }

    /// Takes the grant away, ends the open shell sessions, and writes the line without `pty`. The
    /// access ends first, in memory and on the open connections. Does nothing when there is no grant.
    fn end_shell(&self, running: &Running, id: &str, event: &str) -> Result<(), &'static str> {
        let name = running.peers.get(id).ok_or("notPaired")?.name;
        if running.peers.set_shell(id, None)?.is_none() {
            return Ok(());
        }
        if let Ok(server) = &running.server {
            server.close_shell(id);
        }
        let outcome = self.rewrite_line(running, id);
        if outcome.is_err() {
            Self::line_failed(running, id);
        }
        let _ = running.log.append(&Entry::new(
            Some(id),
            Some(&name),
            event,
            outcome.err().unwrap_or("ok"),
        ));
        outcome
    }

    /// Ends the grants whose time has come, and compares `authorized_keys` once a minute while a
    /// shell is open. The timer calls it with the wall clock.
    pub fn tick(&self, at: u64) {
        let Ok(running) = self.running() else { return };
        for id in running.peers.shell_expired(at) {
            let _ = self.end_shell(running, &id, "shellExpired");
        }
        self.retry_lines(running);
        let last = running.last_keys_check.load(Ordering::Relaxed);
        if running.watch.active() && at.saturating_sub(last) >= shell::CHECK_EVERY.as_secs() {
            running.last_keys_check.store(at, Ordering::Relaxed);
            self.compare_keys(running, |digest| running.watch.check(digest, at));
        }
    }

    /// The hash of the lines of `authorized_keys` that are not ours, and nothing when it cannot be read.
    fn digest(running: &Running) -> Option<String> {
        running.keys.as_ref().ok()?.foreign_digest().ok()
    }

    /// Runs a comparison and records each warning in the log.
    fn compare_keys(&self, running: &Running, compare: impl FnOnce(Option<&str>) -> Vec<(String, String)>) {
        let digest = Self::digest(running);
        for (peer, name) in compare(digest.as_deref()) {
            let _ = running.log.append(&Entry::new(Some(&peer), Some(&name), "keysChanged", "ok"));
        }
    }

    fn shell_started(&self, id: &str) {
        let Ok(running) = self.running() else { return };
        let Some(peer) = running.peers.get(id) else { return };
        running.last_keys_check.store(now(), Ordering::Relaxed);
        self.compare_keys(running, |digest| running.watch.started(id, &peer.name, digest, now()));
    }

    fn shell_ended(&self, id: &str) {
        let Ok(running) = self.running() else { return };
        self.compare_keys(running, |digest| running.watch.ended(id, digest, now()));
    }

    /// The changes of `authorized_keys` that the user has not dismissed.
    pub fn warnings(&self) -> Vec<KeysWarning> {
        self.running().map(|running| running.watch.warnings()).unwrap_or_default()
    }

    pub fn dismiss_warning(&self, id: u64) -> Result<(), &'static str> {
        self.running()?.watch.dismiss(id);
        Ok(())
    }

    /// Opens a shell on a device that this one controls, in Terminal. The device decides whether it is
    /// granted. It writes a file and starts a program, so call it off the main thread.
    pub fn open_shell(&self, id: &str) -> Result<(), &'static str> {
        let peer = DeviceId::parse(id)?;
        let running = self.running()?;
        let destination = self.device(id).ok_or("notPermitted")?.destination()?;
        shell::open(&running.layout, &destination, &peer)
    }

    pub fn pairing_state(&self) -> devices::Pairing {
        self.running()
            .map_or_else(|_| devices::Pairing::closed(), |running| running.pairing.state())
    }

    /// Opens a pairing window with the permissions that the user chose here. It reads the
    /// system, so call it off the main thread.
    pub fn open_pairing(
        &self,
        control: bool,
        applications: bool,
        actions: bool,
    ) -> Result<Opened, &'static str> {
        self.running()?.pairing.open(PeerPermissions {
            control,
            applications,
            actions,
            shell: false,
        })
    }

    pub fn close_pairing(&self) -> Result<(), &'static str> {
        self.running()?.pairing.close()
    }

    /// The devices that this one controls, with what is known of whether they answer.
    pub fn remote_devices(&self) -> Vec<Remote> {
        let Ok(Running {
            controlled: Ok(store),
            ..
        }) = &self.0
        else {
            return Vec::new();
        };
        let list = store.list();
        let online = store.online(&list);
        let Ok(running) = self.running() else {
            return Vec::new();
        };
        list.iter()
            .zip(online)
            // A session that works says more than a check of the port.
            .map(|(device, online)| {
                let online = if running.clients.connected(&device.id) {
                    Some(true)
                } else {
                    online
                };
                Remote::new(device, online)
            })
            .collect()
    }

    /// Pairs with the device at the address, with the code that it shows. It runs ssh and can
    /// take a minute, so call it off the main thread.
    pub fn pair_device(
        &self,
        user: &str,
        host: &str,
        port: Option<u16>,
        code: &str,
    ) -> Result<Remote, &'static str> {
        let running = self.running()?;
        let store = running.controlled.as_ref().map_err(|code| *code)?;
        let destination = Destination::new(user, host, port)?;
        let device = pair_client::pair(&running.layout, store, &running.own, &destination, code)?;
        let _ = running.log.append(&Entry::new(
            Some(&device.id),
            Some(&device.name),
            "pairDevice",
            "ok",
        ));
        Ok(Remote::new(&device, None))
    }

    /// Removes a device from this one: the entry, the pinned host key, and the open connection.
    /// Our key stays in the `authorized_keys` of that device, so the answer says that it must be
    /// removed there too.
    pub fn forget_device(&self, id: &str) -> Result<Forgotten, &'static str> {
        let device_id = DeviceId::parse(id)?;
        let running = self.running()?;
        let store = running.controlled.as_ref().map_err(|code| *code)?;
        let device = store.remove(id)?.ok_or("notPaired")?;
        running.clients.close(id);
        // The device is gone either way. A step that fails is reported after the others ran.
        let unpinned = known_hosts::unpin(&running.layout, &device_id);
        let stopped = device
            .destination()
            .and_then(|destination| ssh::stop_master(&running.layout, &destination, &device_id));
        let outcome = unpinned.and(stopped);
        let _ = running.log.append(&Entry::new(
            Some(id),
            Some(&device.name),
            "forgetDevice",
            outcome.err().unwrap_or("ok"),
        ));
        outcome.map(|()| Forgotten {
            remove_on_device: true,
        })
    }

    pub fn cut_all(&self) -> Result<(), &'static str> {
        let running = self.running()?;
        // Nobody can pair while access is being cut.
        let _ = running.pairing.close();
        let gone = running.peers.clear()?;
        if let Ok(server) = &running.server {
            server.close_all();
        }
        let outcome = running
            .keys
            .as_ref()
            .map_err(|code| *code)
            .and_then(AuthorizedKeys::remove_all);
        if outcome.is_err() {
            for peer in &gone {
                Self::line_failed(running, &peer.id);
            }
        }
        let _ = running.log.append(&Entry::new(
            None,
            None,
            "cutAll",
            outcome.err().unwrap_or("ok"),
        ));
        outcome
    }

    pub fn log(&self) -> Result<Vec<Entry>, &'static str> {
        Ok(self.running()?.log.read())
    }

    /// Ends the connections and removes the socket, when the app quits.
    pub fn stop(&self) {
        if let Ok(running) = &self.0 {
            running.log.flush();
            // The window and its line must not outlive the app.
            let _ = running.pairing.close();
            running.clients.close_all();
            if let Ok(server) = &running.server {
                server.stop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{
        peers::testing::{peer, ID_A, ID_B},
        testing::scratch,
    };
    use crate::remote::peers::Peers;
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        time::Duration,
    };

    struct Deny;

    impl Handler for Deny {
        fn handle(&self, _: Origin, _: Request) -> Result<Value, &'static str> {
            Ok(Value::Null)
        }
    }

    #[test]
    fn removing_a_device_cuts_its_connection_and_its_line_and_is_logged() {
        let home = scratch("svc-home");
        let short = PathBuf::from(format!("/tmp/lw-svc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&short);
        let layout = Layout::for_paths(short.clone(), None);
        fs::create_dir(&short).unwrap();
        fs::set_permissions(&short, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let service = Service::for_test(&layout, home.clone(), Deny);
        let running = service.running().unwrap();
        for id in [ID_A, ID_B] {
            running.peers.insert(peer(id, "Studio")).unwrap();
            running
                .keys
                .as_ref()
                .unwrap()
                .install(&running.peers.get(id).unwrap(), &[])
                .unwrap();
        }
        assert_eq!(service.controllers().len(), 2);

        let stream = UnixStream::connect(layout.socket_file()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        writeln!(writer, "{{\"hello\":{{\"peer\":\"{ID_A}\"}}}}").unwrap();
        writeln!(writer, "{{\"request\":{{\"command\":\"getStatuses\"}}}}").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            service.controllers().iter().filter(|c| c.connected).count(),
            1
        );

        service.remove_peer(ID_A).unwrap();
        line.clear();
        assert_eq!(
            reader.read_line(&mut line).unwrap_or(0),
            0,
            "the connection ended"
        );
        let keys = fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
        assert!(!keys.contains(ID_A) && keys.contains(ID_B));
        assert_eq!(service.remove_peer(ID_A), Err("notPaired"));
        assert_eq!(service.log().unwrap()[0].request, "removePeer");

        service.cut_all().unwrap();
        assert!(service.controllers().is_empty());
        assert_eq!(
            fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap(),
            ""
        );
        assert_eq!(service.log().unwrap()[0].request, "cutAll");
        service.stop();
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(short).unwrap();
    }

    #[test]
    fn a_store_that_cannot_open_is_reported_and_nothing_runs() {
        let short = PathBuf::from(format!("/tmp/lw-svc-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&short);
        fs::create_dir(&short).unwrap();
        fs::set_permissions(&short, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        fs::write(short.join("peers.json"), "garbage").unwrap();
        fs::set_permissions(
            short.join("peers.json"),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        let layout = Layout::for_paths(short.clone(), None);
        let service = Service::for_test(&layout, short.clone(), Deny);
        assert_eq!(service.cut_all(), Err("peersFileInvalid"));
        assert_eq!(service.socket(), Socket::unavailable("peersFileInvalid"));
        assert!(!layout.socket_file().exists());
        fs::remove_dir_all(short).unwrap();
    }

    #[test]
    fn a_cached_icon_is_served_without_asking_and_bad_requests_are_refused() {
        let short = PathBuf::from(format!("/tmp/lw-svc-icon-{}", std::process::id()));
        let _ = fs::remove_dir_all(&short);
        fs::create_dir(&short).unwrap();
        fs::set_permissions(&short, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let layout = Layout::for_paths(short.clone(), None);
        let service = Service::for_test(&layout, short.clone(), Deny);
        let running = service.running().unwrap();
        running
            .controlled
            .as_ref()
            .unwrap()
            .insert(crate::remote::controlled::testing::device(ID_A, "Studio"))
            .unwrap();
        // A device that this one does not control, and arguments that are not valid.
        assert_eq!(service.cached_icon(ID_B, "com.apple.Safari", "1"), Err("notPermitted"));
        assert_eq!(service.cached_icon("short", "com.apple.Safari", "1"), Err("deviceIdInvalid"));
        assert_eq!(service.cached_icon(ID_A, "../x", "1"), Err("badRequest"));
        assert_eq!(service.cached_icon(ID_A, "com.apple.Safari", &"v".repeat(65)), Err("badRequest"));
        // A kept icon comes back as a path in the icons folder, with no request to the device.
        let png = {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(&mut bytes, 8, 8);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&[0; 8 * 8 * 4]).unwrap();
            bytes
        };
        let kept = running.icons.as_ref().unwrap().store(ID_A, "com.apple.Safari", "17.0", &png).unwrap();
        assert_eq!(service.cached_icon(ID_A, "com.apple.Safari", "17.0").unwrap(), kept.to_str().unwrap());
        assert!(kept.starts_with(layout.icons_dir()));
        service.set_pinned(ID_A, &["com.apple.Safari".to_owned()]).unwrap();
        assert_eq!(service.device(ID_A).unwrap().pinned, ["com.apple.Safari"]);
        assert_eq!(service.set_pinned(ID_A, &["bad id".to_owned()]), Err("badRequest"));
        service.stop();
        fs::remove_dir_all(short).unwrap();
    }

    #[test]
    fn the_answer_of_a_device_is_one_small_base64_png_and_nothing_else() {
        use serde_json::json;
        assert!(decode_icon(&json!({"png": "aGk="})).is_ok());
        for bad in [
            json!({}),
            json!({"png": 5}),
            json!({"png": "not base64!"}),
            json!({"png": "aGk=", "extra": 1}),
            json!({"png": "A".repeat(400_000)}),
            json!("aGk="),
        ] {
            assert_eq!(decode_icon(&bad), Err("appIconFailed"), "{bad}");
        }
    }

    struct Fixture {
        home: PathBuf,
        short: PathBuf,
        layout: Layout,
        service: Service,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.service.stop();
            let _ = fs::remove_dir_all(&self.home);
            let _ = fs::remove_dir_all(&self.short);
        }
    }

    /// A service with one paired device and its line, in short directories because of the socket path.
    fn fixture(label: &str) -> Fixture {
        let home = scratch(label);
        let short = PathBuf::from(format!("/tmp/lw-svc-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&short);
        fs::create_dir(&short).unwrap();
        fs::set_permissions(&short, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let layout = Layout::for_paths(short.clone(), None);
        let service = Service::for_test(&layout, home.clone(), Deny);
        let running = service.running().unwrap();
        running.peers.insert(peer(ID_A, "Studio")).unwrap();
        running.keys.as_ref().unwrap().install(&running.peers.get(ID_A).unwrap(), &[]).unwrap();
        Fixture { home, short, layout, service }
    }

    fn line_of(fixture: &Fixture, id: &str) -> String {
        fs::read_to_string(fixture.home.join(".ssh/authorized_keys"))
            .unwrap()
            .lines()
            .find(|line| line.ends_with(&format!("lazy-worker:{id}")))
            .unwrap()
            .to_owned()
    }

    fn requests(fixture: &Fixture) -> Vec<(String, String)> {
        fixture.service.log().unwrap().into_iter().map(|entry| (entry.request, entry.outcome)).collect()
    }

    /// A raw shell connection, as the helper makes one. Returns it with the first answer line.
    fn shell_connection(fixture: &Fixture, id: &str) -> (UnixStream, BufReader<UnixStream>, String) {
        let stream = UnixStream::connect(fixture.layout.socket_file()).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        writeln!(writer, "{{\"hello\":{{\"peer\":\"{id}\",\"shell\":true}}}}").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        (writer, reader, line)
    }

    fn ended(reader: &mut BufReader<UnixStream>) -> bool {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap_or(0) == 0
    }

    const FROM: &str = "from=\"10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,169.254.0.0/16,100.64.0.0/10,fc00::/7,fe80::/10\"";

    #[test]
    fn a_grant_and_its_end_rewrite_the_line_with_and_without_pty_and_are_logged() {
        let fixture = fixture("svc-shell-line");
        let service = &fixture.service;
        let command = format!(
            "command=\"\\\"/Applications/Lazy Worker.app/Contents/MacOS/lazy-worker\\\" remote --peer {ID_A}\" {} lazy-worker:{ID_A}",
            crate::remote::peers::testing::KEY
        );
        assert_eq!(line_of(&fixture, ID_A), format!("restrict,{FROM},{command}"));
        let grant = service.grant_shell(ID_A, Some(60)).unwrap();
        assert!(matches!(grant.until, ShellUntil::At(end) if end > now() + 3500 && end <= now() + 3600));
        assert_eq!(line_of(&fixture, ID_A), format!("restrict,pty,{FROM},{command}"));
        assert!(service.controllers()[0].permissions.shell && service.controllers()[0].shell == Some(grant));
        assert_eq!(service.shell_of(ID_A), Some(grant));
        service.revoke_shell(ID_A).unwrap();
        assert_eq!(line_of(&fixture, ID_A), format!("restrict,{FROM},{command}"));
        assert!(service.shell_of(ID_A).is_none() && !service.controllers()[0].permissions.shell);
        // Until turned off has no end time.
        assert_eq!(service.grant_shell(ID_A, None).unwrap().until, ShellUntil::OFF);
        assert!(line_of(&fixture, ID_A).starts_with("restrict,pty,"));
        service.revoke_shell(ID_A).unwrap();
        // Ending it again changes nothing and logs nothing.
        let before = requests(&fixture).len();
        service.revoke_shell(ID_A).unwrap();
        assert_eq!(requests(&fixture).len(), before);
        let mut events: Vec<_> = requests(&fixture);
        events.reverse();
        let ok = |name: &str| (name.to_owned(), "ok".to_owned());
        assert_eq!(events, [ok("shellGranted"), ok("shellRevoked"), ok("shellGranted"), ok("shellRevoked")]);
        // Bad durations, unknown ids, and another device are refused.
        for minutes in [0, 1441, u32::MAX] {
            assert_eq!(service.grant_shell(ID_A, Some(minutes)), Err("shellDurationInvalid"));
        }
        assert_eq!(service.grant_shell(ID_B, Some(5)), Err("notPaired"));
        assert_eq!(service.grant_shell("short", Some(5)), Err("deviceIdInvalid"));
        assert_eq!(service.grant_shell(ID_A, Some(1440)).map(|_| ()), Ok(()));
        // Pairing the device again never keeps a grant.
        let running = service.running().unwrap();
        running.peers.insert(peer(ID_A, "Studio")).unwrap();
        assert!(service.shell_of(ID_A).is_none());
    }

    #[test]
    fn a_line_that_cannot_be_written_leaves_no_grant() {
        let fixture = fixture("svc-shell-fail");
        let file = fixture.home.join(".ssh/authorized_keys");
        let moved = fixture.home.join("elsewhere");
        fs::rename(&file, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &file).unwrap();
        assert_eq!(fixture.service.grant_shell(ID_A, Some(5)).map(|_| ()), Err("authorizedKeysUnsafe"));
        assert!(fixture.service.shell_of(ID_A).is_none());
        assert!(!fixture.service.controllers()[0].permissions.shell);
        assert_eq!(requests(&fixture), [("shellGranted".to_owned(), "authorizedKeysUnsafe".to_owned())]);
    }

    #[test]
    fn a_line_that_could_not_be_rewritten_when_a_grant_ended_is_tried_again_on_every_tick() {
        let fixture = fixture("svc-shell-retry");
        let service = &fixture.service;
        service.grant_shell(ID_A, Some(60)).unwrap();
        let file = fixture.home.join(".ssh/authorized_keys");
        let moved = fixture.home.join("elsewhere");
        fs::rename(&file, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &file).unwrap();
        // The access ends at once, and the failure is logged once.
        assert_eq!(service.revoke_shell(ID_A), Err("authorizedKeysUnsafe"));
        assert!(service.shell_of(ID_A).is_none());
        let failures = || requests(&fixture).iter().filter(|(name, outcome)| name == "shellRevoked" && outcome == "authorizedKeysUnsafe").count();
        assert_eq!(failures(), 1);
        service.tick(now());
        service.tick(now());
        assert_eq!(failures(), 1, "a retry that fails logs nothing more");
        assert!(fs::read_to_string(&moved).unwrap().contains("pty,"), "the line still has pty");
        // The file is fine again, and the next tick writes the line without pty.
        fs::remove_file(&file).unwrap();
        fs::rename(&moved, &file).unwrap();
        service.tick(now());
        assert!(!line_of(&fixture, ID_A).contains("pty"));
        assert!(service.running().unwrap().line_needs_rewrite.lock().unwrap().is_empty());
        // Nothing is written on the ticks after that.
        let written = fs::read_to_string(&file).unwrap();
        service.tick(now());
        assert_eq!(fs::read_to_string(&file).unwrap(), written);
    }

    #[test]
    fn losing_actions_is_reported_once_and_logged() {
        let fixture = fixture("svc-perm-actions");
        let service = &fixture.service;
        assert_eq!(service.set_permissions(ID_A, true, false, true), Ok(false));
        assert!(service.peer_may_run_actions(ID_A));
        assert_eq!(service.set_permissions(ID_A, true, false, false), Ok(true));
        assert!(!service.peer_may_run_actions(ID_A));
        assert_eq!(service.set_permissions(ID_A, true, false, false), Ok(false));
        assert!(!service.peer_may_run_actions(ID_B), "a device that is not paired may run nothing");
        assert_eq!(service.set_permissions(ID_B, true, false, false), Err("notPaired"));
        assert_eq!(requests(&fixture).iter().filter(|(name, _)| name == "peerPermissions").count(), 3);
    }

    #[test]
    fn the_server_answers_a_shell_only_while_granted_and_ends_it_on_revoke_expiry_cut_and_remove() {
        let fixture = fixture("svc-shell-ends");
        let service = &fixture.service;
        let (_writer, mut reader, line) = shell_connection(&fixture, ID_A);
        assert_eq!(line, "{\"error\":\"notPermitted\"}\n");
        assert!(ended(&mut reader));

        // Revoke.
        service.grant_shell(ID_A, None).unwrap();
        let (_writer, mut reader, line) = shell_connection(&fixture, ID_A);
        assert_eq!(line, "{\"ok\":\"shell\"}\n");
        service.revoke_shell(ID_A).unwrap();
        assert!(ended(&mut reader), "revoke");

        // Expiry, by the wall clock.
        service.grant_shell(ID_A, Some(1)).unwrap();
        let (_writer, mut reader, line) = shell_connection(&fixture, ID_A);
        assert_eq!(line, "{\"ok\":\"shell\"}\n");
        service.tick(now() + 30);
        assert!(service.shell_of(ID_A).is_some(), "not yet");
        service.tick(now() + 61);
        assert!(ended(&mut reader), "expiry");
        assert!(!line_of(&fixture, ID_A).contains("pty"));
        assert!(service.shell_of(ID_A).is_none());
        // A shell cannot start again from the grant that ended.
        let (_writer, mut reader, line) = shell_connection(&fixture, ID_A);
        assert_eq!(line, "{\"error\":\"notPermitted\"}\n");
        assert!(ended(&mut reader));

        // Cut all.
        service.grant_shell(ID_A, None).unwrap();
        let (_writer, mut reader, _) = shell_connection(&fixture, ID_A);
        service.cut_all().unwrap();
        assert!(ended(&mut reader), "cut all");

        // Remove the device.
        let running = service.running().unwrap();
        running.peers.insert(peer(ID_A, "Studio")).unwrap();
        service.grant_shell(ID_A, None).unwrap();
        let (_writer, mut reader, _) = shell_connection(&fixture, ID_A);
        service.remove_peer(ID_A).unwrap();
        assert!(ended(&mut reader), "remove");

        let names: Vec<String> = requests(&fixture).into_iter().map(|(name, _)| name).collect();
        for expected in ["shellGranted", "shellRevoked", "shellExpired", "shellStart"] {
            assert!(names.iter().any(|name| name == expected), "{expected} in {names:?}");
        }
        let starts: u32 = service.log().unwrap().iter().filter(|entry| entry.request == "shellStart").map(|entry| entry.count).sum();
        assert_eq!(starts, 6);
    }

    #[test]
    fn a_grant_found_expired_at_start_is_revoked_before_the_line_is_used() {
        let home = scratch("svc-shell-start");
        let short = PathBuf::from(format!("/tmp/lw-svc-start-{}", std::process::id()));
        let _ = fs::remove_dir_all(&short);
        fs::create_dir(&short).unwrap();
        fs::set_permissions(&short, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let layout = Layout::for_paths(short.clone(), None);
        // The state of an earlier run: a grant that ended, one that has not, and the lines with `pty`.
        {
            let peers = Peers::open(layout.peers_file()).unwrap();
            peers.insert(peer(ID_A, "Studio")).unwrap();
            peers.insert(peer(ID_B, "Mini")).unwrap();
            peers.set_shell(ID_A, Some(ShellUntil::At(now() - 5))).unwrap();
            peers.set_shell(ID_B, Some(ShellUntil::At(now() + 3600))).unwrap();
            let keys = AuthorizedKeys::new(home.clone(), PathBuf::from("/Applications/Lazy Worker.app/Contents/MacOS/lazy-worker")).unwrap();
            for id in [ID_A, ID_B] {
                let mut stale = peers.get(id).unwrap();
                stale.permissions.shell = true;
                keys.install(&stale, &[]).unwrap();
            }
            assert!(fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap().matches("restrict,pty,").count() == 2);
        }
        let service = Service::for_test(&layout, home.clone(), Deny);
        let keys = fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
        let line = |id: &str| keys.lines().find(|line| line.ends_with(&format!("lazy-worker:{id}"))).unwrap().to_owned();
        assert!(!line(ID_A).contains("pty"), "the ended grant lost pty");
        assert!(line(ID_B).starts_with("restrict,pty,"), "the grant that lasts keeps it");
        assert!(service.shell_of(ID_A).is_none() && service.shell_of(ID_B).is_some());
        let log = service.log().unwrap();
        assert_eq!((log[0].request.as_str(), log[0].peer.as_deref(), log[0].outcome.as_str()), ("shellExpired", Some(ID_A), "ok"));
        assert_eq!(log.len(), 1);
        // The peers file does not keep it either.
        let stored = Peers::open(layout.peers_file()).unwrap();
        assert!(stored.get(ID_A).unwrap().shell_until.is_none());
        service.stop();
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(short).unwrap();
    }

    #[test]
    fn a_change_to_the_keys_during_a_shell_raises_a_warning_that_can_be_dismissed() {
        let fixture = fixture("svc-shell-keys");
        let service = &fixture.service;
        let file = fixture.home.join(".ssh/authorized_keys");
        let mut text = fs::read_to_string(&file).unwrap();
        text.insert_str(0, "ssh-ed25519 AAAAuser user@host\n");
        fs::write(&file, &text).unwrap();
        service.grant_shell(ID_A, None).unwrap();
        service.shell_started(ID_A);
        let start = now() + 1000;
        // Our own lines change as the grant is turned on and off: no warning.
        service.revoke_shell(ID_A).unwrap();
        service.grant_shell(ID_A, None).unwrap();
        service.tick(start + 61);
        assert!(service.warnings().is_empty());
        // A key that the shell added is a warning, once, with the device's name.
        fs::write(&file, format!("{text}ssh-ed25519 AAAAattacker evil\n")).unwrap();
        service.tick(start + 100);
        assert!(service.warnings().is_empty(), "the check runs once a minute");
        service.tick(start + 130);
        let warnings = service.warnings();
        assert_eq!(warnings.len(), 1);
        assert_eq!((warnings[0].peer.as_str(), warnings[0].name.as_str()), (ID_A, "Studio"));
        service.tick(start + 200);
        assert_eq!(service.warnings().len(), 1);
        assert!(requests(&fixture).contains(&("keysChanged".to_owned(), "ok".to_owned())));
        // Another change at the end of the session is a second warning.
        fs::write(&file, format!("{text}ssh-ed25519 AAAAattacker evil\nssh-ed25519 AAAAmore x\n")).unwrap();
        service.shell_ended(ID_A);
        assert_eq!(service.warnings().len(), 2);
        let id = service.warnings()[0].id;
        service.dismiss_warning(id).unwrap();
        assert_eq!(service.warnings().len(), 1);
        // With no shell open, nothing is compared.
        fs::write(&file, text).unwrap();
        service.tick(start + 400);
        assert_eq!(service.warnings().len(), 1);
    }
}
