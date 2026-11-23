//! The devices that may control this one, and the networks the user added for them.
//!
//! The file is read once at start. Every change is written before the memory copy changes, so
//! the two never disagree. The socket server reads the memory copy on every request.

use super::{
    check_private_file,
    identity::{parse_public_key, DeviceId},
    now, read_small, write_atomic,
};
use crate::devices::Kind;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    net::{Ipv4Addr, Ipv6Addr},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

pub const MAX_NAME: usize = 60;
pub const MAX_PEERS: usize = 64;
pub const MAX_NETWORKS: usize = 32;
const VERSION: u32 = 1;
/// A new time of last access is written at most this often. The memory copy always has it.
const SAVE_EVERY: u64 = 30;

/// What a peer may do. All off until the user turns each on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerPermissions {
    #[serde(default)]
    pub control: bool,
    #[serde(default)]
    pub applications: bool,
    #[serde(default)]
    pub actions: bool,
    /// Granting the shell arrives in a later step. Nothing here sets it.
    #[serde(default)]
    pub shell: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// `ssh-ed25519 <base64>`, with no comment.
    pub public_key: String,
    pub permissions: PeerPermissions,
    pub added: u64,
    pub last_access: Option<u64>,
}

/// True for an invisible format character (Unicode Cf) that can reorder or hide text:
/// soft hyphen, zero-width and bidi marks, bidi embeds, isolates, and the word joiner range.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// A name for people: trimmed, with no control or invisible format characters, and at most
/// `MAX_NAME` characters.
pub fn clean_name(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| !c.is_control() && !is_format(*c))
        .collect();
    kept.trim()
        .chars()
        .take(MAX_NAME)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Accepts `address/prefix` for IPv4 or IPv6, and nothing else. A prefix of 0 would allow everyone.
pub fn validate_network(text: &str) -> Result<(), &'static str> {
    let (address, prefix) = text.split_once('/').ok_or("networkInvalid")?;
    let prefix: u8 = prefix
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| prefix.parse().ok())
        .flatten()
        .ok_or("networkInvalid")?;
    let maximum = if address.parse::<Ipv4Addr>().is_ok() {
        32
    } else if address.parse::<Ipv6Addr>().is_ok() {
        128
    } else {
        return Err("networkInvalid");
    };
    if prefix == 0 || prefix > maximum || !address.is_ascii() {
        return Err("networkInvalid");
    }
    Ok(())
}

impl Peer {
    pub fn new(
        id: &DeviceId,
        name: &str,
        kind: Kind,
        public_key: &str,
        added: u64,
    ) -> Result<Self, &'static str> {
        let peer = Self {
            id: id.as_str().to_owned(),
            name: clean_name(name),
            kind,
            public_key: parse_public_key(public_key).map_err(|_| "peerInvalid")?,
            permissions: PeerPermissions::default(),
            added,
            last_access: None,
        };
        peer.validate()?;
        Ok(peer)
    }

    /// What a stored peer must satisfy, so a file edited by hand cannot widen anything.
    fn validate(&self) -> Result<(), &'static str> {
        DeviceId::parse(&self.id).map_err(|_| "peerInvalid")?;
        if self.name.is_empty() || self.name != clean_name(&self.name) {
            return Err("peerInvalid");
        }
        if parse_public_key(&self.public_key).as_deref() != Ok(self.public_key.as_str()) {
            return Err("peerInvalid");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    version: u32,
    #[serde(default)]
    networks: Vec<String>,
    #[serde(default)]
    peers: Vec<Peer>,
}

impl Data {
    fn parse(text: &str) -> Result<Self, &'static str> {
        let data: Self = serde_json::from_str(text).map_err(|_| "peersFileInvalid")?;
        if data.version != VERSION
            || data.peers.len() > MAX_PEERS
            || data.networks.len() > MAX_NETWORKS
        {
            return Err("peersFileInvalid");
        }
        let mut seen = HashSet::new();
        for peer in &data.peers {
            peer.validate().map_err(|_| "peersFileInvalid")?;
            if !seen.insert(peer.id.as_str()) {
                return Err("peersFileInvalid");
            }
        }
        let mut networks = HashSet::new();
        for network in &data.networks {
            validate_network(network).map_err(|_| "peersFileInvalid")?;
            if !networks.insert(network.as_str()) {
                return Err("peersFileInvalid");
            }
        }
        Ok(data)
    }

    fn to_bytes(&self) -> Result<Vec<u8>, &'static str> {
        serde_json::to_vec_pretty(self).map_err(|_| "remoteIo")
    }
}

struct State {
    data: Data,
    /// The last time of access that reached the file, by peer id.
    saved_access: std::collections::HashMap<String, u64>,
}

pub struct Peers {
    path: PathBuf,
    state: Mutex<State>,
}

impl Peers {
    /// Reads the file. A missing file is an empty store. A file that is unsafe, unknown, or
    /// malformed is refused, and it is never replaced.
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
        let saved_access = data
            .peers
            .iter()
            .filter_map(|peer| Some((peer.id.clone(), peer.last_access?)))
            .collect();
        Ok(Self {
            path,
            state: Mutex::new(State { data, saved_access }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Writes a changed copy, and keeps it only when the write worked.
    fn change<T>(
        &self,
        edit: impl FnOnce(&mut Data) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        let mut state = self.lock();
        let mut next = state.data.clone();
        let result = edit(&mut next)?;
        write_atomic(&self.path, &next.to_bytes()?)?;
        state.data = next;
        Ok(result)
    }

    pub fn get(&self, id: &str) -> Option<Peer> {
        self.lock()
            .data
            .peers
            .iter()
            .find(|peer| peer.id == id)
            .cloned()
    }

    pub fn list(&self) -> Vec<Peer> {
        self.lock().data.peers.clone()
    }

    pub fn networks(&self) -> Vec<String> {
        self.lock().data.networks.clone()
    }

    /// Adds a peer, or replaces the one with the same id.
    pub fn insert(&self, peer: Peer) -> Result<(), &'static str> {
        peer.validate()?;
        self.change(|data| {
            if let Some(known) = data.peers.iter_mut().find(|known| known.id == peer.id) {
                *known = peer;
            } else if data.peers.len() >= MAX_PEERS {
                return Err("peersFull");
            } else {
                data.peers.push(peer);
            }
            Ok(())
        })
    }

    /// Sets what the user can set here. The shell stays as it is.
    pub fn set_permissions(
        &self,
        id: &str,
        control: bool,
        applications: bool,
        actions: bool,
    ) -> Result<(), &'static str> {
        self.change(|data| {
            let peer = data
                .peers
                .iter_mut()
                .find(|peer| peer.id == id)
                .ok_or("notPaired")?;
            peer.permissions = PeerPermissions {
                control,
                applications,
                actions,
                shell: peer.permissions.shell,
            };
            Ok(())
        })
    }

    pub fn remove(&self, id: &str) -> Result<Option<Peer>, &'static str> {
        self.change(|data| {
            let index = data.peers.iter().position(|peer| peer.id == id);
            Ok(index.map(|index| data.peers.remove(index)))
        })
    }

    /// Drops every peer. The networks stay, because they are the user's own choice.
    pub fn clear(&self) -> Result<Vec<Peer>, &'static str> {
        self.change(|data| Ok(std::mem::take(&mut data.peers)))
    }

    /// Notes an access. The file follows at most every `SAVE_EVERY` seconds, and a failed write is
    /// not an error, because the time is only for people to read.
    pub fn touch(&self, id: &str) {
        let at = now();
        let mut state = self.lock();
        let Some(peer) = state.data.peers.iter_mut().find(|peer| peer.id == id) else {
            return;
        };
        peer.last_access = Some(at);
        let due = state
            .saved_access
            .get(id)
            .is_none_or(|saved| at.saturating_sub(*saved) >= SAVE_EVERY);
        if due {
            if let Ok(bytes) = state.data.to_bytes() {
                if write_atomic(&self.path, &bytes).is_ok() {
                    state.saved_access.insert(id.to_owned(), at);
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A valid ed25519 public key line.
    pub const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEAIQGBb5/OG0Dy0+bwjhEsK9YwJibGeEwvVJZ+kP2Mw";
    pub const ID_A: &str = "0123456789abcdef0123456789abcdef";
    pub const ID_B: &str = "fedcba9876543210fedcba9876543210";

    pub fn peer(id: &str, name: &str) -> Peer {
        Peer::new(
            &DeviceId::parse(id).unwrap(),
            name,
            Kind::Laptop,
            KEY,
            1_700_000_000,
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};
    use crate::remote::testing::scratch;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn store(label: &str) -> (PathBuf, Peers) {
        let dir = scratch(label);
        let path = dir.join("peers.json");
        let peers = Peers::open(path).unwrap();
        (dir, peers)
    }

    #[test]
    fn the_key_in_the_tests_is_accepted_by_the_real_check() {
        assert!(parse_public_key(KEY).is_ok());
    }

    #[test]
    fn a_missing_file_is_an_empty_store_and_a_change_writes_a_private_file() {
        let (dir, peers) = store("peers-new");
        assert!(peers.list().is_empty());
        assert!(peers.networks().is_empty());
        peers.insert(peer(ID_A, "  Studio\u{7}  ")).unwrap();
        let path = dir.join("peers.json");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let reopened = Peers::open(path).unwrap();
        let stored = reopened.get(ID_A).unwrap();
        assert_eq!(stored.name, "Studio");
        assert_eq!(stored.permissions, PeerPermissions::default());
        assert_eq!(stored.last_access, None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn names_are_cleaned_and_cut() {
        assert_eq!(clean_name("  a\nb\t "), "ab");
        assert_eq!(clean_name(&"é".repeat(80)).chars().count(), MAX_NAME);
        assert_eq!(clean_name("\u{0}\u{1}"), "");
        // Bidi controls, zero-width characters, and the byte order mark are removed.
        assert_eq!(clean_name("a\u{202E}b\u{2066}c\u{2069}d\u{200B}e\u{FEFF}f\u{2060}g\u{00AD}h"), "abcdefgh");
        assert_eq!(clean_name("\u{202E}\u{200F}"), "");
        let id = DeviceId::parse(ID_A).unwrap();
        assert_eq!(
            Peer::new(&id, "\n ", Kind::Desktop, KEY, 0).unwrap_err(),
            "peerInvalid"
        );
    }

    #[test]
    fn a_key_that_is_not_ed25519_is_refused() {
        let id = DeviceId::parse(ID_A).unwrap();
        for key in [
            "ssh-rsa AAAAB3NzaC1yc2E",
            "ssh-ed25519",
            "",
            "ssh-ed25519 !!!!",
        ] {
            assert!(Peer::new(&id, "x", Kind::Laptop, key, 0).is_err(), "{key}");
        }
    }

    #[test]
    fn networks_are_checked_strictly() {
        for good in [
            "10.0.0.0/8",
            "192.168.1.5/24",
            "fc00::/7",
            "127.0.0.1/32",
            "::1/128",
        ] {
            assert_eq!(validate_network(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "10.0.0.0",
            "10.0.0.0/",
            "/8",
            "10.0.0.0/0",
            "10.0.0.0/33",
            "fc00::/129",
            "10.0.0.0/+8",
            "10.0.0.0/8 ",
            "10.0.0/8",
            "a.b.c.d/8",
            "10.0.0.0/8,0.0.0.0/1",
            "10.0.0.0/8\"",
            "10.0.0.0/-1",
            "010.0.0.0/8",
            "*/8",
            "10.0.0.0/08x",
        ] {
            assert!(validate_network(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn malformed_unknown_or_unsafe_files_are_refused_and_left_alone() {
        let dir = scratch("peers-bad");
        let path = dir.join("peers.json");
        let valid_peer = serde_json::to_value(peer(ID_A, "Studio")).unwrap();
        let cases = [
            "not json".to_owned(),
            "{}".to_owned(),
            r#"{"version":2,"peers":[]}"#.to_owned(),
            r#"{"version":1,"peers":[],"extra":1}"#.to_owned(),
            r#"{"version":1,"networks":["0.0.0.0/0"],"peers":[]}"#.to_owned(),
            serde_json::json!({"version":1,"peers":[valid_peer.clone(), valid_peer.clone()]})
                .to_string(),
            serde_json::json!({"version":1,"peers":[{"id":"short"}]}).to_string(),
            {
                let mut bad = valid_peer.clone();
                bad["name"] = "  padded ".into();
                serde_json::json!({"version":1,"peers":[bad]}).to_string()
            },
            {
                let mut bad = valid_peer.clone();
                bad["publicKey"] = "ssh-ed25519 AAAA extra-comment".into();
                serde_json::json!({"version":1,"peers":[bad]}).to_string()
            },
            {
                let mut bad = valid_peer;
                bad["permissions"]["root"] = true.into();
                serde_json::json!({"version":1,"peers":[bad]}).to_string()
            },
        ];
        for text in cases {
            fs::write(&path, &text).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(
                Peers::open(path.clone()).err(),
                Some("peersFileInvalid"),
                "{text}"
            );
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                text,
                "the file is untouched"
            );
        }
        fs::write(&path, r#"{"version":1}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(Peers::open(path).err(), Some("remoteFileUnsafe"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn permissions_default_off_and_shell_cannot_be_set_here() {
        let (dir, peers) = store("peers-perm");
        peers.insert(peer(ID_A, "Studio")).unwrap();
        peers.set_permissions(ID_A, true, true, false).unwrap();
        let stored = peers.get(ID_A).unwrap().permissions;
        assert_eq!(
            stored,
            PeerPermissions {
                control: true,
                applications: true,
                actions: false,
                shell: false
            }
        );
        assert_eq!(
            peers.set_permissions(ID_B, true, true, true),
            Err("notPaired")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn remove_and_clear_drop_peers_and_keep_networks() {
        let (dir, peers) = store("peers-remove");
        peers.insert(peer(ID_A, "A")).unwrap();
        peers.insert(peer(ID_B, "B")).unwrap();
        assert_eq!(peers.remove(ID_A).unwrap().unwrap().name, "A");
        assert!(peers.remove(ID_A).unwrap().is_none());
        assert_eq!(peers.clear().unwrap().len(), 1);
        assert!(peers.list().is_empty());
        let reopened = Peers::open(dir.join("peers.json")).unwrap();
        assert!(reopened.list().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_write_leaves_the_memory_copy_unchanged() {
        let (dir, peers) = store("peers-fail");
        peers.insert(peer(ID_A, "A")).unwrap();
        // The directory is gone, so the temporary file cannot be created.
        fs::remove_dir_all(&dir).unwrap();
        assert!(peers.insert(peer(ID_B, "B")).is_err());
        assert!(peers.get(ID_B).is_none());
        assert!(peers.get(ID_A).is_some());
    }

    #[test]
    fn touch_updates_memory_at_once_and_the_file_not_every_time() {
        let (dir, peers) = store("peers-touch");
        peers.insert(peer(ID_A, "A")).unwrap();
        peers.touch(ID_A);
        let first = peers.get(ID_A).unwrap().last_access.unwrap();
        let on_disk = |path: PathBuf| Peers::open(path).unwrap().get(ID_A).unwrap().last_access;
        assert_eq!(on_disk(dir.join("peers.json")), Some(first));
        peers.touch(ID_A);
        peers.touch("ffffffffffffffffffffffffffffffff");
        fs::remove_dir_all(dir).unwrap();
    }
}
