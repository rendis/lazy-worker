//! The target side of remote control as the app uses it: the peers, the log, the managed lines,
//! and the socket server, kept in step when a device is removed or all access is cut.

use super::{
    authorized_keys::AuthorizedKeys,
    identity::DeviceId,
    log::{Entry, Log},
    peers::Peers,
    server::{Handler, Server},
    Layout,
};
use crate::{
    devices::{Controller, Socket},
    dispatch::{self, Origin, Request},
};
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
    peers: Arc<Peers>,
    log: Arc<Log>,
    keys: Result<AuthorizedKeys, &'static str>,
    server: Result<Server, &'static str>,
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
        let server = Server::start(layout.socket_file(), peers.clone(), log.clone(), handler);
        Ok(Running {
            peers,
            log,
            keys,
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
            Ok(_) => Socket::listening(),
        }
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

    pub fn cut_all(&self) -> Result<(), &'static str> {
        let running = self.running()?;
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
        if let Ok(Running {
            server: Ok(server), ..
        }) = &self.0
        {
            server.stop();
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
}
