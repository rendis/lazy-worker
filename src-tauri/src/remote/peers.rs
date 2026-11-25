//! The devices that may control this one, and the networks the user added for them.
//!
//! The file is read once at start. Every change is written before the memory copy changes, so
//! the two never disagree. The socket server reads the memory copy on every request.

use super::{
    check_private_file,
    identity::{parse_public_key, DeviceId},
    now, read_small, write_atomic,
};
use crate::devices::{FilesGrant, Kind, ShellUntil};
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
    /// Whether the device may see the Screen Sharing state and open this screen. Off until the user turns it on here.
    #[serde(default)]
    pub screen: bool,
    /// Whether the device may leave texts and files here and pick up what waits for it. Off until the user turns it on here.
    #[serde(default)]
    pub mailbox: bool,
    /// Derived from `Peer::shell_until` each time a peer is read. A value in the file is ignored.
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
    /// When the full shell that the user granted at this device ends. Only `Peers::set_shell` sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_until: Option<ShellUntil>,
    /// The files that the user shared with this device at this device. Only `Peers::set_files` sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_grant: Option<FilesGrant>,
}

/// True for an invisible format character (Unicode Cf) that can reorder or hide text:
/// soft hyphen, zero-width and bidi marks, bidi embeds, isolates, and the word joiner range.
pub(crate) fn is_format(c: char) -> bool {
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
            shell_until: None,
            files_grant: None,
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
        if let Some(grant) = &self.files_grant {
            if grant.folders.len() > crate::remote::files::MAX_FOLDERS
                || grant.folders.iter().any(|folder| folder.len() > 4096 || !folder.starts_with('/'))
            {
                return Err("peerInvalid");
            }
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

    /// The peer as the rest of the app reads it: `permissions.shell` says whether its shell grant
    /// lasts now. The stored flag is never used.
    fn derived(mut peer: Peer) -> Peer {
        peer.permissions.shell = peer.shell_until.is_some_and(|until| until.active(now()));
        peer
    }

    pub fn get(&self, id: &str) -> Option<Peer> {
        self.lock()
            .data
            .peers
            .iter()
            .find(|peer| peer.id == id)
            .cloned()
            .map(Self::derived)
    }

    pub fn list(&self) -> Vec<Peer> {
        self.lock().data.peers.iter().cloned().map(Self::derived).collect()
    }

    /// The ids of the peers whose shell grant ended at or before `at`, and has not been cleared.
    pub fn shell_expired(&self, at: u64) -> Vec<String> {
        self.lock()
            .data
            .peers
            .iter()
            .filter(|peer| matches!(peer.shell_until, Some(ShellUntil::At(end)) if end <= at))
            .map(|peer| peer.id.clone())
            .collect()
    }

    /// Sets or clears the shell grant of a peer, and returns the grant it had.
    pub fn set_shell(
        &self,
        id: &str,
        until: Option<ShellUntil>,
    ) -> Result<Option<ShellUntil>, &'static str> {
        self.change(|data| {
            let peer = data
                .peers
                .iter_mut()
                .find(|peer| peer.id == id)
                .ok_or("notPaired")?;
            peer.permissions.shell = false;
            Ok(std::mem::replace(&mut peer.shell_until, until))
        })
    }

    /// The ids of the peers whose files grant ended at or before `at`, and has not been cleared.
    pub fn files_expired(&self, at: u64) -> Vec<String> {
        self.lock()
            .data
            .peers
            .iter()
            .filter(|peer| {
                peer.files_grant
                    .as_ref()
                    .is_some_and(|grant| matches!(grant.until, ShellUntil::At(end) if end <= at))
            })
            .map(|peer| peer.id.clone())
            .collect()
    }

    /// Sets or clears the files grant of a peer, and returns the grant it had.
    pub fn set_files(
        &self,
        id: &str,
        grant: Option<FilesGrant>,
    ) -> Result<Option<FilesGrant>, &'static str> {
        self.change(|data| {
            let peer = data
                .peers
                .iter_mut()
                .find(|peer| peer.id == id)
                .ok_or("notPaired")?;
            Ok(std::mem::replace(&mut peer.files_grant, grant))
        })
    }

    pub fn networks(&self) -> Vec<String> {
        self.lock().data.networks.clone()
    }

    /// Adds a peer, or replaces the one with the same id. A shell or files grant is never carried in.
    pub fn insert(&self, mut peer: Peer) -> Result<(), &'static str> {
        peer.validate()?;
        peer.shell_until = None;
        peer.files_grant = None;
        peer.permissions.shell = false;
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

    /// Sets what the user can set here. The shell has its own grant.
    pub fn set_permissions(
        &self,
        id: &str,
        control: bool,
        applications: bool,
        actions: bool,
        screen: bool,
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
                screen,
                // The mailbox has its own switch.
                mailbox: peer.permissions.mailbox,
                shell: false,
            };
            Ok(())
        })
    }

    /// Turns the mailbox of a device on or off. Returns whether it was on.
    pub fn set_mailbox(&self, id: &str, on: bool) -> Result<bool, &'static str> {
        self.change(|data| {
            let peer = data.peers.iter_mut().find(|peer| peer.id == id).ok_or("notPaired")?;
            let before = peer.permissions.mailbox;
            peer.permissions.mailbox = on;
            Ok(before)
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
        peers.set_permissions(ID_A, true, true, false, true).unwrap();
        let stored = peers.get(ID_A).unwrap().permissions;
        assert_eq!(
            stored,
            PeerPermissions {
                control: true,
                applications: true,
                actions: false,
                screen: true,
                mailbox: false,
                shell: false
            }
        );
        assert_eq!(
            peers.set_permissions(ID_B, true, true, true, false),
            Err("notPaired")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_without_screen_loads_with_it_off() {
        let (dir, peers) = store("peers-screen-old");
        peers.insert(peer(ID_A, "Studio")).unwrap();
        peers.set_permissions(ID_A, true, true, true, true).unwrap();
        let path = dir.join("peers.json");
        let mut file: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let permissions = file["peers"][0]["permissions"].as_object_mut().unwrap();
        assert_eq!(permissions.remove("screen"), Some(serde_json::Value::Bool(true)));
        fs::write(&path, file.to_string()).unwrap();
        let stored = Peers::open(path).unwrap().get(ID_A).unwrap().permissions;
        assert!(stored.control && stored.applications && stored.actions && !stored.screen);
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

    #[test]
    fn the_shell_grant_is_stored_and_the_permission_is_derived_from_it() {
        let (dir, peers) = store("peers-shell");
        peers.insert(peer(ID_A, "A")).unwrap();
        assert!(!peers.get(ID_A).unwrap().permissions.shell);
        let later = ShellUntil::At(now() + 600);
        assert_eq!(peers.set_shell(ID_A, Some(later)).unwrap(), None);
        let granted = peers.get(ID_A).unwrap();
        assert!(granted.permissions.shell && granted.shell_until == Some(later));
        // The same state comes back from the file, with the time as a number.
        let text = fs::read_to_string(dir.join("peers.json")).unwrap();
        assert!(text.contains("\"shellUntil\""));
        assert!(Peers::open(dir.join("peers.json")).unwrap().get(ID_A).unwrap().permissions.shell);
        // Until turned off is stored as the word, and has no end.
        peers.set_shell(ID_A, Some(ShellUntil::OFF)).unwrap();
        assert!(fs::read_to_string(dir.join("peers.json")).unwrap().contains("\"untilOff\""));
        assert!(Peers::open(dir.join("peers.json")).unwrap().get(ID_A).unwrap().permissions.shell);
        assert!(peers.shell_expired(u64::MAX).is_empty(), "no end, no expiry");
        // A grant in the past is not active, and is listed as expired until it is cleared.
        peers.set_shell(ID_A, Some(ShellUntil::At(5))).unwrap();
        assert!(!peers.get(ID_A).unwrap().permissions.shell);
        assert_eq!(peers.shell_expired(5), [ID_A]);
        assert!(peers.shell_expired(4).is_empty());
        assert_eq!(peers.set_shell(ID_A, None).unwrap(), Some(ShellUntil::At(5)));
        assert!(peers.shell_expired(u64::MAX).is_empty());
        assert_eq!(peers.set_shell(ID_B, None), Err("notPaired"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn permissions_and_pairing_never_carry_or_keep_a_shell() {
        let (dir, peers) = store("peers-shell-keep");
        peers.insert(peer(ID_A, "A")).unwrap();
        peers.set_shell(ID_A, Some(ShellUntil::OFF)).unwrap();
        // Setting the other permissions leaves the grant as it is, and a stored `shell: true` is ignored.
        peers.set_permissions(ID_A, true, true, true, false).unwrap();
        assert!(peers.get(ID_A).unwrap().permissions.shell);
        peers.set_shell(ID_A, None).unwrap();
        let mut forged = peer(ID_B, "B");
        forged.permissions.shell = true;
        forged.shell_until = Some(ShellUntil::OFF);
        peers.insert(forged).unwrap();
        let stored = peers.get(ID_B).unwrap();
        assert!(!stored.permissions.shell && stored.shell_until.is_none());
        let text = serde_json::json!({"version": 1, "peers": [{
            "id": ID_A, "name": "A", "kind": "laptop", "publicKey": KEY,
            "permissions": {"shell": true}, "added": 1, "lastAccess": null
        }]});
        fs::write(dir.join("peers2.json"), text.to_string()).unwrap();
        fs::set_permissions(dir.join("peers2.json"), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!Peers::open(dir.join("peers2.json")).unwrap().get(ID_A).unwrap().permissions.shell);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_files_grant_is_stored_expires_and_is_never_set_by_permissions_or_pairing() {
        use crate::devices::{FilesGrant, FilesMode, FilesScope};
        let (dir, peers) = store("peers-files");
        peers.insert(peer(ID_A, "A")).unwrap();
        let grant = |until| FilesGrant {
            scope: FilesScope::Folders,
            folders: vec!["/Users/a/Docs".into()],
            mode: FilesMode::ReadWrite,
            until,
        };
        assert_eq!(peers.set_files(ID_A, Some(grant(ShellUntil::At(5)))).unwrap(), None);
        let text = fs::read_to_string(dir.join("peers.json")).unwrap();
        assert!(text.contains("\"filesGrant\"") && text.contains("readWrite"));
        assert_eq!(Peers::open(dir.join("peers.json")).unwrap().get(ID_A).unwrap().files_grant, Some(grant(ShellUntil::At(5))));
        assert_eq!(peers.files_expired(5), [ID_A]);
        assert!(peers.files_expired(4).is_empty());
        // Other permissions leave it alone. A peer that is inserted again starts with none.
        peers.set_permissions(ID_A, true, true, true, false).unwrap();
        assert!(peers.get(ID_A).unwrap().files_grant.is_some());
        let mut forged = peer(ID_B, "B");
        forged.files_grant = Some(grant(ShellUntil::OFF));
        peers.insert(forged).unwrap();
        assert!(peers.get(ID_B).unwrap().files_grant.is_none());
        peers.insert(peer(ID_A, "A")).unwrap();
        assert!(peers.get(ID_A).unwrap().files_grant.is_none());
        assert_eq!(peers.set_files(ID_B, None).unwrap(), None);
        assert_eq!(peers.set_files("0".repeat(32).as_str(), None), Err("notPaired"));
        // A file that holds a relative folder, or too many, is refused.
        let mut bad = peer(ID_A, "A");
        bad.files_grant = Some(FilesGrant { folders: vec!["relative".into()], ..grant(ShellUntil::OFF) });
        assert_eq!(Data { version: 1, networks: vec![], peers: vec![bad] }.to_bytes().map(|bytes| Data::parse(&String::from_utf8(bytes).unwrap()).err()), Ok(Some("peersFileInvalid")));
        fs::remove_dir_all(dir).unwrap();
    }

}
