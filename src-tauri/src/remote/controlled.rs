//! The devices that this one controls, as pairing saved them.
//!
//! The file is read once at start. Every change is written before the memory copy changes. A
//! file that is unsafe or malformed is refused and never replaced.

use super::{
    check_private_file,
    destination::Destination,
    identity::DeviceId,
    peers::{clean_name, MAX_PEERS},
    read_small, write_atomic,
};
use crate::devices::Kind;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

const VERSION: u32 = 1;
/// How many applications a device can have pinned.
pub const MAX_PINNED: usize = 3;
/// A device is checked again for reachability at most this often.
const CHECK_EVERY: Duration = Duration::from_secs(30);
const CHECK_TIMEOUT: Duration = Duration::from_secs(1);

/// A device that this one controls. `user`, `host`, and `port` are the address pairing found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Controlled {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub user: String,
    pub host: String,
    pub port: u16,
    pub added: u64,
    /// The bundle identifiers of the applications that the Devices view shows first.
    #[serde(default)]
    pub pinned: Vec<String>,
}

/// At most `MAX_PINNED` different, valid bundle identifiers.
fn valid_pins(pinned: &[String]) -> Result<(), &'static str> {
    let mut seen = HashSet::new();
    if pinned.len() > MAX_PINNED
        || !pinned
            .iter()
            .all(|id| crate::apps::valid_bundle_id(id) && seen.insert(id.as_str()))
    {
        return Err("badRequest");
    }
    Ok(())
}

impl Controlled {
    pub fn new(
        id: &DeviceId,
        name: &str,
        kind: Kind,
        destination: &Destination,
        added: u64,
    ) -> Result<Self, &'static str> {
        let device = Self {
            id: id.as_str().to_owned(),
            name: clean_name(name),
            kind,
            user: destination.user().to_owned(),
            host: destination.host().to_owned(),
            port: destination.port(),
            added,
            pinned: Vec::new(),
        };
        device.validate()?;
        Ok(device)
    }

    /// What a stored device must satisfy, so a file edited by hand cannot reach another place.
    fn validate(&self) -> Result<(), &'static str> {
        DeviceId::parse(&self.id).map_err(|_| "devicesFileInvalid")?;
        if self.name.is_empty() || self.name != clean_name(&self.name) {
            return Err("devicesFileInvalid");
        }
        // The address must be one that `Destination` would give back unchanged.
        let destination = Destination::new(&self.user, &self.host, Some(self.port))
            .map_err(|_| "devicesFileInvalid")?;
        if destination.host() != self.host || destination.user() != self.user {
            return Err("devicesFileInvalid");
        }
        valid_pins(&self.pinned).map_err(|_| "devicesFileInvalid")
    }

    pub fn destination(&self) -> Result<Destination, &'static str> {
        Destination::new(&self.user, &self.host, Some(self.port))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    version: u32,
    #[serde(default)]
    devices: Vec<Controlled>,
}

impl Data {
    fn parse(text: &str) -> Result<Self, &'static str> {
        let data: Self = serde_json::from_str(text).map_err(|_| "devicesFileInvalid")?;
        if data.version != VERSION || data.devices.len() > MAX_PEERS {
            return Err("devicesFileInvalid");
        }
        let mut seen = HashSet::new();
        for device in &data.devices {
            device.validate()?;
            if !seen.insert(device.id.as_str()) {
                return Err("devicesFileInvalid");
            }
        }
        Ok(data)
    }
}

pub struct Store {
    path: PathBuf,
    data: Mutex<Data>,
    reach: Arc<Mutex<HashMap<String, Reach>>>,
}

/// What the last check of a device found, and whether another check is running.
#[derive(Clone, Copy)]
struct Reach {
    checked: Instant,
    online: Option<bool>,
    running: bool,
}

/// True when a device has no answer yet, or its answer is older than `CHECK_EVERY`.
fn due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|checked| now.saturating_duration_since(checked) >= CHECK_EVERY)
}

/// Whether a TCP connection to the address works within a second.
fn reachable(host: &str, port: u16) -> bool {
    (host, port)
        .to_socket_addrs()
        .is_ok_and(|mut addresses| {
            addresses.any(|address| TcpStream::connect_timeout(&address, CHECK_TIMEOUT).is_ok())
        })
}

impl Store {
    /// Reads the file. A missing file is an empty list.
    pub fn open(path: PathBuf) -> Result<Self, &'static str> {
        let data = match read_small(&path)? {
            None => Data {
                version: VERSION,
                ..Data::default()
            },
            Some(text) => {
                check_private_file(&path)?;
                Data::parse(&text)?
            }
        };
        Ok(Self {
            path,
            data: Mutex::new(data),
            reach: Arc::default(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn change<T>(
        &self,
        edit: impl FnOnce(&mut Data) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        let mut data = self.lock();
        let mut next = data.clone();
        let result = edit(&mut next)?;
        let bytes = serde_json::to_vec_pretty(&next).map_err(|_| "remoteIo")?;
        write_atomic(&self.path, &bytes)?;
        *data = next;
        Ok(result)
    }

    pub fn get(&self, id: &str) -> Option<Controlled> {
        self.lock().devices.iter().find(|device| device.id == id).cloned()
    }

    pub fn list(&self) -> Vec<Controlled> {
        self.lock().devices.clone()
    }

    /// Adds a device, or replaces the one with the same id.
    pub fn insert(&self, device: Controlled) -> Result<(), &'static str> {
        device.validate()?;
        self.change(|data| {
            if let Some(known) = data.devices.iter_mut().find(|known| known.id == device.id) {
                // Pairing again keeps what the user pinned.
                let pinned = std::mem::take(&mut known.pinned);
                *known = device;
                known.pinned = pinned;
            } else if data.devices.len() >= MAX_PEERS {
                return Err("devicesFull");
            } else {
                data.devices.push(device);
            }
            Ok(())
        })
    }

    /// Saves the applications to show first for a device.
    pub fn set_pinned(&self, id: &str, pinned: &[String]) -> Result<(), &'static str> {
        valid_pins(pinned)?;
        self.change(|data| {
            let device = data
                .devices
                .iter_mut()
                .find(|device| device.id == id)
                .ok_or("notPaired")?;
            device.pinned = pinned.to_vec();
            Ok(())
        })
    }

    pub fn remove(&self, id: &str) -> Result<Option<Controlled>, &'static str> {
        let removed = self.change(|data| {
            let index = data.devices.iter().position(|device| device.id == id);
            Ok(index.map(|index| data.devices.remove(index)))
        })?;
        self.reach
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id);
        Ok(removed)
    }

    /// The last known answer of each device: `Some(true)` or `Some(false)`, or nothing before
    /// the first check ends. It never waits. A device whose answer is old is checked again in
    /// the background, so the next call has a fresh one.
    pub fn online(&self, devices: &[Controlled]) -> Vec<Option<bool>> {
        let now = Instant::now();
        let mut table = self.reach.lock().unwrap_or_else(|error| error.into_inner());
        devices
            .iter()
            .map(|device| {
                let known = table.get(&device.id).copied();
                if known.is_none_or(|reach| !reach.running && due(Some(reach.checked), now)) {
                    table.insert(
                        device.id.clone(),
                        Reach {
                            checked: now,
                            online: known.and_then(|reach| reach.online),
                            running: true,
                        },
                    );
                    self.check(&mut table, device);
                }
                table.get(&device.id).and_then(|reach| reach.online)
            })
            .collect()
    }

    /// Checks the device now and keeps the answer, so the next list shows it. It waits up to
    /// `CHECK_TIMEOUT` for each address, so call it off the main thread.
    pub fn check_now(&self, device: &Controlled) -> bool {
        let online = reachable(&device.host, device.port);
        // A device that was removed meanwhile stays out of the table.
        if self.get(&device.id).is_some() {
            self.reach
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    device.id.clone(),
                    Reach {
                        checked: Instant::now(),
                        online: Some(online),
                        running: false,
                    },
                );
        }
        online
    }

    /// Starts a check in the background. `table` is the locked table of the caller.
    fn check(&self, table: &mut HashMap<String, Reach>, device: &Controlled) {
        let (id, host, port) = (device.id.clone(), device.host.clone(), device.port);
        let shared = Arc::clone(&self.reach);
        let spawned = thread::Builder::new()
            .name("remote-reach".into())
            .spawn(move || {
                let online = reachable(&host, port);
                let mut table = shared.lock().unwrap_or_else(|error| error.into_inner());
                // A device that was removed meanwhile stays out of the table.
                if let Some(reach) = table.get_mut(&id) {
                    *reach = Reach {
                        checked: Instant::now(),
                        online: Some(online),
                        running: false,
                    };
                }
            });
        if spawned.is_err() {
            if let Some(reach) = table.get_mut(&device.id) {
                reach.running = false;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub fn device(id: &str, name: &str) -> Controlled {
        Controlled::new(
            &DeviceId::parse(id).unwrap(),
            name,
            Kind::Desktop,
            &Destination::new("me", "studio.local", Some(22)).unwrap(),
            1_700_000_000,
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::device, *};
    use crate::remote::{
        peers::testing::{ID_A, ID_B},
        testing::scratch,
    };
    use std::{fs, os::unix::fs::PermissionsExt};

    fn file(label: &str) -> (PathBuf, PathBuf) {
        let base = scratch(label);
        (base.clone(), base.join("devices.json"))
    }

    #[test]
    fn devices_are_stored_privately_and_come_back_after_a_restart() {
        let (base, path) = file("ctl-store");
        let store = Store::open(path.clone()).unwrap();
        assert!(store.list().is_empty());
        store.insert(device(ID_A, "Studio")).unwrap();
        store.insert(device(ID_B, "Office")).unwrap();
        store.insert(device(ID_A, "Studio 2")).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let again = Store::open(path).unwrap();
        assert_eq!(again.list().len(), 2);
        assert_eq!(again.get(ID_A).unwrap().name, "Studio 2");
        assert_eq!(again.remove(ID_A).unwrap().unwrap().id, ID_A);
        assert_eq!(again.remove(ID_A).unwrap(), None);
        assert_eq!(again.list().len(), 1);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_malformed_file_is_refused_and_left_alone() {
        let valid = |extra: &str| {
            format!(
                "{{\"version\":1,\"devices\":[{{\"id\":\"{ID_A}\",\"name\":\"Studio\",\"kind\":\"desktop\",\"user\":\"me\",\"host\":\"studio.local\",\"port\":22,\"added\":1{extra}}}]}}"
            )
        };
        for (label, text) in [
            ("garbage", "garbage".to_owned()),
            ("version", valid("").replace("\"version\":1", "\"version\":2")),
            ("unknown", valid(",\"extra\":1")),
            ("id", valid("").replace(ID_A, "short")),
            ("name", valid("").replace("Studio", " ")),
            ("host", valid("").replace("studio.local", "-oProxyCommand=x")),
            ("host case", valid("").replace("studio.local", "Studio.Local")),
            ("user", valid("").replace("\"me\"", "\"-me\"")),
            ("port", valid("").replace("\"port\":22", "\"port\":0")),
            ("kind", valid("").replace("desktop", "tablet")),
            (
                "duplicate",
                valid("").replace("}]}", &format!("}},{{\"id\":\"{ID_A}\",\"name\":\"x\",\"kind\":\"desktop\",\"user\":\"me\",\"host\":\"h.local\",\"port\":22,\"added\":1}}]}}")),
            ),
        ] {
            let (base, path) = file(&format!("ctl-bad-{}", label.replace(' ', "-")));
            fs::write(&path, &text).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(Store::open(path.clone()).is_err(), "{label}");
            assert_eq!(fs::read_to_string(&path).unwrap(), text, "{label}");
            fs::remove_dir_all(base).unwrap();
        }
        let (base, path) = file("ctl-open-mode");
        fs::write(&path, valid("")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(Store::open(path).err(), Some("remoteFileUnsafe"));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_device_is_checked_again_only_when_its_answer_is_old() {
        let now = Instant::now();
        assert!(due(None, now));
        assert!(!due(Some(now), now));
        assert!(!due(Some(now), now + Duration::from_secs(29)));
        assert!(due(Some(now), now + Duration::from_secs(30)));
    }

    #[test]
    fn the_online_answer_never_waits_and_a_listener_makes_it_true() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (base, path) = file("ctl-online");
        let store = Store::open(path).unwrap();
        let mut here = device(ID_A, "Here");
        here.host = "127.0.0.1".into();
        here.port = port;
        let start = Instant::now();
        assert_eq!(store.online(std::slice::from_ref(&here)), [None]);
        assert!(start.elapsed() < Duration::from_millis(500));
        let mut answer = None;
        for _ in 0..200 {
            thread::sleep(Duration::from_millis(25));
            answer = store.online(std::slice::from_ref(&here))[0];
            if answer.is_some() {
                break;
            }
        }
        assert_eq!(answer, Some(true));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_check_now_waits_for_the_answer_and_keeps_it() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (base, path) = file("ctl-check-now");
        let store = Store::open(path).unwrap();
        let mut here = device(ID_A, "Here");
        here.host = "127.0.0.1".into();
        here.port = port;
        store.insert(here.clone()).unwrap();
        assert!(store.check_now(&here));
        assert_eq!(store.online(std::slice::from_ref(&here)), [Some(true)]);
        drop(listener);
        assert!(!store.check_now(&here));
        assert_eq!(store.online(std::slice::from_ref(&here)), [Some(false)]);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn pinned_applications_are_validated_saved_and_kept_when_pairing_again() {
        let (base, path) = file("ctl-pins");
        let store = Store::open(path.clone()).unwrap();
        store.insert(device(ID_A, "Studio")).unwrap();
        let pins = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        store.set_pinned(ID_A, &pins(&["com.apple.Safari", "com.apple.Notes"])).unwrap();
        assert_eq!(store.get(ID_A).unwrap().pinned, pins(&["com.apple.Safari", "com.apple.Notes"]));
        for bad in [
            pins(&["a.b", "c.d", "e.f", "g.h"]),
            pins(&["a.b", "a.b"]),
            pins(&["bad id"]),
            pins(&["-b"]),
            pins(&[""]),
            pins(&["x/y"]),
        ] {
            assert_eq!(store.set_pinned(ID_A, &bad), Err("badRequest"), "{bad:?}");
        }
        assert_eq!(store.set_pinned(ID_B, &pins(&["a.b"])), Err("notPaired"));
        // A refused change leaves the saved pins as they were.
        assert_eq!(store.get(ID_A).unwrap().pinned, pins(&["com.apple.Safari", "com.apple.Notes"]));
        // Pairing again keeps them, and a restart reads them back.
        store.insert(device(ID_A, "Studio renamed")).unwrap();
        let reopened = Store::open(path.clone()).unwrap();
        assert_eq!(reopened.get(ID_A).unwrap().pinned, pins(&["com.apple.Safari", "com.apple.Notes"]));
        assert_eq!(reopened.get(ID_A).unwrap().name, "Studio renamed");
        // A file that was edited by hand to hold a bad pin is refused.
        let text = fs::read_to_string(&path).unwrap().replace("com.apple.Notes", "bad id");
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(Store::open(path), Err("devicesFileInvalid")));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_file_from_before_pins_has_none() {
        let (base, path) = file("ctl-old");
        let store = Store::open(path.clone()).unwrap();
        store.insert(device(ID_A, "Studio")).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let without: String = text.lines().filter(|line| !line.contains("pinned")).collect::<Vec<_>>().join("\n");
        let cleaned = without.replace("\"added\": 1700000000,", "\"added\": 1700000000");
        fs::write(&path, cleaned).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Store::open(path).unwrap().get(ID_A).unwrap().pinned.is_empty());
        fs::remove_dir_all(base).unwrap();
    }
}
