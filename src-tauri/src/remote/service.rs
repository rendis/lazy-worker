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
    server::{Handler, Server},
    ssh, Layout,
};
use crate::{
    apps,
    devices::{self, Controller, Remote, Socket},
    dispatch::{self, Origin, Request},
};
use serde::Serialize;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use tauri::{AppHandle, Manager};

/// Runs a remote request through the dispatcher that the webview uses.
struct Dispatcher(AppHandle);

impl Handler for Dispatcher {
    fn handle(&self, origin: Origin, request: Request) -> Result<Value, &'static str> {
        tauri::async_runtime::block_on(dispatch::handle(&self.0, origin, request))
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
        Self(layout.and_then(|layout| Self::open(&layout, home, helper, Dispatcher(app.clone()))))
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

    pub fn set_permissions(
        &self,
        id: &str,
        control: bool,
        applications: bool,
        actions: bool,
    ) -> Result<(), &'static str> {
        DeviceId::parse(id)?;
        self.running()?
            .peers
            .set_permissions(id, control, applications, actions)
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
        let _ = running.log.append(&Entry::new(
            Some(id),
            Some(&peer.name),
            "removePeer",
            outcome.err().unwrap_or("ok"),
        ));
        outcome
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
        running.peers.clear()?;
        if let Ok(server) = &running.server {
            server.close_all();
        }
        let outcome = running
            .keys
            .as_ref()
            .map_err(|code| *code)
            .and_then(AuthorizedKeys::remove_all);
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
}
