//! The lines of `~/.ssh/authorized_keys` that belong to this app.
//!
//! A managed line ends with the comment `lazy-worker:<id>`. Only those lines are ever changed.
//! Every other line keeps its exact bytes and its place. A managed line grants one thing: to run
//! this app's helper for one peer, from the networks listed, with no terminal.

use super::{
    identity::{parse_public_key, DeviceId},
    our_uid,
    pairing_key,
    peers::{validate_network, Peer},
    write_atomic,
};
use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder},
    io::ErrorKind,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Private and link-local networks, and the shared carrier-grade range that Tailscale uses.
/// Loopback is not here: a local process is not a second device.
pub const DEFAULT_NETWORKS: [&str; 7] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "100.64.0.0/10",
    "fc00::/7",
    "fe80::/10",
];

const COMMENT_PREFIX: &str = "lazy-worker:";
/// The name that stands in the managed comment of the pairing line, where a peer has its id.
pub const PAIRING: &str = "pairing";
const MAX_FILE: u64 = 1024 * 1024;

/// Serializes the read, change, and write of the file inside this process.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// The file of one user, and the program that its lines run.
#[derive(Clone, Debug)]
pub struct AuthorizedKeys {
    home: PathBuf,
    helper: PathBuf,
    trust: Trust,
}

/// Checks the helper path and gives the path that a line names.
type Trust = fn(&Path) -> Result<PathBuf, &'static str>;

/// The group of the administrators on macOS. They can become root, so a directory of root that
/// they can write is as safe as one that root alone can write.
const ADMIN_GID: u32 = 80;

/// True when an owner and a mode let nobody but root, us, and for root's directories the
/// administrators, change the entry.
fn entry_safe(uid: u32, gid: u32, mode: u32, ours: u32) -> bool {
    let owner = uid == 0 || uid == ours;
    let group_only_admin = uid == 0 && gid == ADMIN_GID;
    let others = mode & 0o002 == 0;
    let group = mode & 0o020 == 0 || group_only_admin;
    owner && others && group
}

/// Checks the file and each directory from it up to `floor`, and the path for a translocation.
/// Nothing above `floor` is looked at.
fn trusted_from(path: &Path, floor: &Path) -> Result<PathBuf, &'static str> {
    let real = fs::canonicalize(path).map_err(|_| "helperPathUnsafe")?;
    if real.to_string_lossy().contains("/AppTranslocation/") {
        return Err("helperPathUnsafe");
    }
    let ours = our_uid();
    let mut entry: Option<&Path> = Some(real.as_path());
    while let Some(current) = entry {
        let meta = fs::symlink_metadata(current).map_err(|_| "helperPathUnsafe")?;
        if !entry_safe(meta.uid(), meta.gid(), meta.mode(), ours) {
            return Err("helperPathUnsafe");
        }
        if current == floor {
            break;
        }
        entry = current.parent();
    }
    Ok(real)
}

/// The check that runs before a line is written: no translocated copy, and the program and every
/// directory above it owned by root or us and closed to group and others.
fn trusted_helper(path: &Path) -> Result<PathBuf, &'static str> {
    #[cfg(test)]
    return Ok(path.to_path_buf());
    #[cfg(not(test))]
    trusted_from(path, Path::new("/"))
}

/// The program path goes inside double quotes that sshd hands to the login shell. A character
/// that the shell would read inside double quotes, or a control character, makes it unusable.
fn check_helper(path: &Path) -> Result<&str, &'static str> {
    let text = path.to_str().ok_or("helperPathInvalid")?;
    let bad = |c: char| matches!(c, '"' | '\\' | '$' | '`' | '!') || c.is_control();
    if !path.is_absolute() || text.chars().any(bad) {
        return Err("helperPathInvalid");
    }
    Ok(text)
}

/// The line for one peer. The networks are the defaults and the user's extra ones.
pub fn render(
    id: &str,
    public_key: &str,
    networks: &[String],
    helper: &Path,
) -> Result<String, &'static str> {
    let id = DeviceId::parse(id)?;
    let key = parse_public_key(public_key)?;
    let helper = check_helper(helper)?;
    Ok(format!(
        "restrict,from=\"{}\",command=\"\\\"{}\\\" remote --peer {}\" {} {}{}",
        allowed_networks(networks)?.join(","),
        helper,
        id.as_str(),
        key,
        COMMENT_PREFIX,
        id.as_str()
    ))
}

/// The text that `expiry-time` takes: `YYYYMMDDHHMM` in the local time of the target. sshd
/// reads a value with no `Z` as local time, so the caller passes a local time. The window end
/// plus one minute, cut to the minute, is never before the end.
pub fn expiry_time<Zone: chrono::TimeZone>(end_plus_grace: &chrono::DateTime<Zone>) -> String
where
    Zone::Offset: std::fmt::Display,
{
    end_plus_grace.format("%Y%m%d%H%M").to_string()
}

/// The line that lets the fixed pairing key run `remote --pair`, from the same networks as a
/// peer line, until `expiry` (see `expiry_time`).
pub fn render_pairing(
    networks: &[String],
    helper: &Path,
    expiry: &str,
) -> Result<String, &'static str> {
    let helper = check_helper(helper)?;
    let digits = expiry.len() == 12 && expiry.bytes().all(|byte| byte.is_ascii_digit());
    if !digits {
        return Err("expiryInvalid");
    }
    Ok(format!(
        "restrict,from=\"{}\",expiry-time=\"{expiry}\",command=\"\\\"{helper}\\\" remote --pair\" {} {COMMENT_PREFIX}{PAIRING}",
        allowed_networks(networks)?.join(","),
        pairing_key::PUBLIC,
    ))
}

fn allowed_networks(networks: &[String]) -> Result<Vec<&str>, &'static str> {
    let mut allowed: Vec<&str> = DEFAULT_NETWORKS.to_vec();
    for network in networks {
        validate_network(network)?;
        if !allowed.contains(&network.as_str()) {
            allowed.push(network);
        }
    }
    Ok(allowed)
}

/// The key of a managed line: its last word is `lazy-worker:<id>`, or `lazy-worker:pairing`.
fn managed_id(line: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(line).ok()?;
    let word = text.split_whitespace().next_back()?;
    let id = word.strip_prefix(COMMENT_PREFIX)?;
    (id == PAIRING || DeviceId::parse(id).is_ok()).then_some(id)
}

enum Others {
    Keep,
    Drop,
}

impl AuthorizedKeys {
    /// `home` is the home directory of the user. `helper` is the app executable.
    pub fn new(home: PathBuf, helper: PathBuf) -> Result<Self, &'static str> {
        Self::with_trust(home, helper, trusted_helper)
    }

    fn with_trust(home: PathBuf, helper: PathBuf, trust: Trust) -> Result<Self, &'static str> {
        check_helper(&helper)?;
        Ok(Self { home, helper, trust })
    }

    /// The program path for a line, or `helperPathUnsafe`. Run before any line is written.
    pub fn trusted_helper(&self) -> Result<PathBuf, &'static str> {
        (self.trust)(&self.helper)
    }

    fn ssh_dir(&self) -> PathBuf {
        self.home.join(".ssh")
    }

    fn file(&self) -> PathBuf {
        self.ssh_dir().join("authorized_keys")
    }

    pub fn install(&self, peer: &Peer, networks: &[String]) -> Result<(), &'static str> {
        let line = render(&peer.id, &peer.public_key, networks, &self.trusted_helper()?)?;
        self.rewrite(
            BTreeMap::from([(peer.id.clone(), Some(line))]),
            Others::Keep,
        )
    }

    pub fn remove(&self, id: &str) -> Result<(), &'static str> {
        DeviceId::parse(id)?;
        self.rewrite(BTreeMap::from([(id.to_owned(), None)]), Others::Keep)
    }

    /// Adds the pairing line, or replaces the one that is there.
    pub fn install_pairing(&self, networks: &[String], expiry: &str) -> Result<(), &'static str> {
        let line = render_pairing(networks, &self.trusted_helper()?, expiry)?;
        self.rewrite(
            BTreeMap::from([(PAIRING.to_owned(), Some(line))]),
            Others::Keep,
        )
    }

    /// Removes the pairing line. Succeeds when there is none.
    pub fn remove_pairing(&self) -> Result<(), &'static str> {
        self.rewrite(BTreeMap::from([(PAIRING.to_owned(), None)]), Others::Keep)
    }

    pub fn remove_all(&self) -> Result<(), &'static str> {
        self.rewrite(BTreeMap::new(), Others::Drop)
    }

    /// Renders every managed line again, for instance after the app moved. A managed line of an id
    /// that is not in `peers` is dropped, and a peer without a line gets one.
    pub fn refresh_all(&self, peers: &[Peer], networks: &[String]) -> Result<(), &'static str> {
        let mut wanted = BTreeMap::new();
        for peer in peers {
            let line = render(&peer.id, &peer.public_key, networks, &self.trusted_helper()?)?;
            wanted.insert(peer.id.clone(), Some(line));
        }
        self.rewrite(wanted, Others::Drop)
    }

    /// Makes `~/.ssh` when it is missing, and checks that it is ours and that others cannot write it.
    fn prepare_dir(&self) -> Result<(), &'static str> {
        let dir = self.ssh_dir();
        match DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return Err("sshDirUnsafe"),
        }
        let meta = fs::symlink_metadata(&dir).map_err(|_| "sshDirUnsafe")?;
        if !meta.is_dir() || meta.uid() != our_uid() || meta.mode() & 0o022 != 0 {
            return Err("sshDirUnsafe");
        }
        Ok(())
    }

    /// The bytes of the file, or nothing when it does not exist. A link or a file of someone else is refused.
    fn read(&self) -> Result<Vec<u8>, &'static str> {
        let path = self.file();
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                let kind = meta.file_type();
                if !kind.is_file()
                    || kind.is_symlink()
                    || kind.is_socket()
                    || meta.uid() != our_uid()
                    || meta.len() > MAX_FILE
                {
                    return Err("authorizedKeysUnsafe");
                }
                fs::read(&path).map_err(|_| "authorizedKeysIo")
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
            Err(_) => Err("authorizedKeysIo"),
        }
    }

    /// `wanted` maps an id to its new line, or to `None` to remove it. A managed line that is not
    /// in `wanted` stays, or goes when `others` says so. The first line of an id takes the new
    /// text in its place, and its duplicates go.
    fn rewrite(
        &self,
        mut wanted: BTreeMap<String, Option<String>>,
        others: Others,
    ) -> Result<(), &'static str> {
        let _guard = FILE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        // A removal has nothing to do where there is no `~/.ssh`, and it must not make one.
        if wanted.values().all(Option::is_none)
            && matches!(fs::symlink_metadata(self.ssh_dir()), Err(error) if error.kind() == ErrorKind::NotFound)
        {
            return Ok(());
        }
        self.prepare_dir()?;
        let original = self.read()?;
        let mut output: Vec<u8> = Vec::with_capacity(original.len() + 512);
        let mut placed: Vec<String> = Vec::new();
        for segment in original.split_inclusive(|&byte| byte == b'\n') {
            let content = segment.strip_suffix(b"\n").unwrap_or(segment);
            let Some(id) = managed_id(content) else {
                output.extend_from_slice(segment);
                continue;
            };
            match wanted.get(id) {
                Some(Some(line)) if !placed.iter().any(|done| done == id) => {
                    output.extend_from_slice(line.as_bytes());
                    output.push(b'\n');
                    placed.push(id.to_owned());
                }
                Some(_) => {}
                None => match others {
                    Others::Keep => output.extend_from_slice(segment),
                    Others::Drop => {}
                },
            }
        }
        wanted.retain(|id, line| line.is_some() && !placed.contains(id));
        for line in wanted.values().flatten() {
            if !output.is_empty() && !output.ends_with(b"\n") {
                output.push(b'\n');
            }
            output.extend_from_slice(line.as_bytes());
            output.push(b'\n');
        }
        if output == original && !original.is_empty() {
            return Ok(());
        }
        if output.is_empty() && original.is_empty() {
            return Ok(());
        }
        let path = self.file();
        write_atomic(&path, &output).map_err(|_| "authorizedKeysIo")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "authorizedKeysIo")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{
        peers::testing::{peer, ID_A, ID_B, KEY},
        testing::scratch,
    };

    const HELPER: &str = "/Applications/Lazy Worker.app/Contents/MacOS/lazy-worker";

    fn keys(label: &str) -> (PathBuf, AuthorizedKeys) {
        let home = scratch(label);
        let keys = AuthorizedKeys::new(home.clone(), PathBuf::from(HELPER)).unwrap();
        (home, keys)
    }

    fn contents(home: &Path) -> Vec<u8> {
        fs::read(home.join(".ssh/authorized_keys")).unwrap()
    }

    #[test]
    fn the_line_has_the_exact_text_with_a_path_with_spaces() {
        let line = render(ID_A, KEY, &["203.0.113.0/24".into()], Path::new(HELPER)).unwrap();
        let expected = format!(
            "restrict,from=\"10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,169.254.0.0/16,100.64.0.0/10,fc00::/7,fe80::/10,203.0.113.0/24\",\
command=\"\\\"{HELPER}\\\" remote --peer {ID_A}\" {KEY} lazy-worker:{ID_A}"
        );
        assert_eq!(line, expected);
        assert!(!line.contains("pty"));
        assert!(!line.contains("127.0.0.1"));
        // An extra network that is already a default is not listed twice.
        let again = render(ID_A, KEY, &["10.0.0.0/8".into()], Path::new(HELPER)).unwrap();
        assert_eq!(again.matches("10.0.0.0/8").count(), 1);
    }

    #[test]
    fn the_pairing_line_has_the_exact_text() {
        let line = render_pairing(
            &["203.0.113.0/24".into()],
            Path::new(HELPER),
            "202611231215",
        )
        .unwrap();
        let expected = format!(
            "restrict,from=\"10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,169.254.0.0/16,100.64.0.0/10,fc00::/7,fe80::/10,203.0.113.0/24\",\
expiry-time=\"202611231215\",command=\"\\\"{HELPER}\\\" remote --pair\" {} lazy-worker:pairing",
            pairing_key::PUBLIC
        );
        assert_eq!(line, expected);
        assert!(!line.contains("pty") && !line.contains("remote --peer"));
        for bad in ["", "2026", "20261123121", "2026112312155", "2026-11-23 12", "20261123121x"] {
            assert_eq!(
                render_pairing(&[], Path::new(HELPER), bad),
                Err("expiryInvalid"),
                "{bad:?}"
            );
        }
        assert_eq!(
            render_pairing(&[], Path::new("/a/b\"c"), "202611231215"),
            Err("helperPathInvalid")
        );
    }

    #[test]
    fn the_expiry_time_is_local_year_to_minute() {
        use chrono::{FixedOffset, TimeZone};
        let zone = FixedOffset::west_opt(5 * 3600).unwrap();
        let moment = zone.with_ymd_and_hms(2026, 1, 2, 3, 4, 59).unwrap();
        assert_eq!(expiry_time(&moment), "202601020304");
        let night = zone.with_ymd_and_hms(2026, 12, 31, 23, 59, 0).unwrap();
        assert_eq!(expiry_time(&night), "202612312359");
    }

    #[test]
    fn the_pairing_line_is_managed_like_a_peer_line_and_removed_with_them() {
        let (home, keys) = keys("ak-pairing");
        fs::create_dir(home.join(".ssh")).unwrap();
        fs::set_permissions(home.join(".ssh"), fs::Permissions::from_mode(0o700)).unwrap();
        let foreign: &[u8] = b"ssh-ed25519 AAAA mine\n";
        fs::write(home.join(".ssh/authorized_keys"), foreign).unwrap();
        keys.install(&peer(ID_A, "A"), &[]).unwrap();
        keys.install_pairing(&[], "202611231215").unwrap();
        let text = String::from_utf8(contents(&home)).unwrap();
        assert!(text.starts_with("ssh-ed25519 AAAA mine\n"));
        assert_eq!(text.matches("remote --pair").count(), 1);
        // A second window replaces the line in place.
        keys.install_pairing(&["203.0.113.0/24".into()], "202611231300").unwrap();
        let text = String::from_utf8(contents(&home)).unwrap();
        assert_eq!(text.matches("remote --pair").count(), 1);
        assert!(text.contains("202611231300") && !text.contains("202611231215"));
        keys.remove_pairing().unwrap();
        let text = String::from_utf8(contents(&home)).unwrap();
        assert!(!text.contains("remote --pair") && text.contains(ID_A));
        keys.remove_pairing().unwrap();
        keys.install_pairing(&[], "202611231215").unwrap();
        keys.remove_all().unwrap();
        assert_eq!(contents(&home), foreign);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn removing_the_pairing_line_does_not_make_the_ssh_directory() {
        let (home, keys) = keys("ak-pairing-none");
        keys.remove_pairing().unwrap();
        assert!(!home.join(".ssh").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_path_that_the_shell_would_read_is_refused() {
        for path in [
            "/a/b\"c",
            "/a/b\\c",
            "/a/$HOME/x",
            "/a/`id`/x",
            "/a/b!c",
            "/a/b\nc",
            "/a/b\tc",
            "relative/x",
            "/a/\u{7f}",
        ] {
            assert_eq!(
                render(ID_A, KEY, &[], Path::new(path)),
                Err("helperPathInvalid"),
                "{path:?}"
            );
            assert!(AuthorizedKeys::new(PathBuf::from("/h"), PathBuf::from(path)).is_err());
        }
    }

    #[test]
    fn a_bad_id_key_or_network_is_refused() {
        let helper = Path::new(HELPER);
        assert!(render("short", KEY, &[], helper).is_err());
        assert!(render(ID_A, "ssh-rsa AAAA", &[], helper).is_err());
        assert!(render(ID_A, KEY, &["10.0.0.0/8\",command=\"x".into()], helper).is_err());
    }

    #[test]
    fn install_creates_ssh_and_the_file_with_private_modes() {
        let (home, keys) = keys("ak-new");
        keys.install(&peer(ID_A, "A"), &[]).unwrap();
        let mode = |path: PathBuf| fs::metadata(path).unwrap().mode() & 0o777;
        assert_eq!(mode(home.join(".ssh")), 0o700);
        assert_eq!(mode(home.join(".ssh/authorized_keys")), 0o600);
        let text = String::from_utf8(contents(&home)).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.starts_with("restrict,from="));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn foreign_lines_keep_their_exact_bytes_and_order() {
        let (home, keys) = keys("ak-foreign");
        fs::create_dir(home.join(".ssh")).unwrap();
        fs::set_permissions(home.join(".ssh"), fs::Permissions::from_mode(0o700)).unwrap();
        let foreign: &[u8] = b"# my keys\n\nssh-rsa AAAAB3Nza me@laptop\r\n  command=\"x y\"   ssh-ed25519 AAAA\tcomment  \nssh-ed25519 \xff\xfe not utf8\n";
        let mut start = foreign.to_vec();
        start.extend_from_slice(b"ssh-ed25519 AAAA no-newline-at-end");
        fs::write(home.join(".ssh/authorized_keys"), &start).unwrap();

        keys.install(&peer(ID_A, "A"), &[]).unwrap();
        keys.install(&peer(ID_B, "B"), &[]).unwrap();
        let after = contents(&home);
        assert!(
            after.starts_with(&start),
            "foreign bytes come first and unchanged"
        );
        let rest = String::from_utf8(after[start.len()..].to_vec()).unwrap();
        assert!(
            rest.starts_with('\n'),
            "a newline separates the last foreign line"
        );
        assert_eq!(rest.lines().filter(|line| !line.is_empty()).count(), 2);

        keys.remove(ID_A).unwrap();
        keys.remove(ID_B).unwrap();
        // The separator newline stays, so the original bytes plus it remain.
        let mut expected = start.clone();
        expected.push(b'\n');
        assert_eq!(contents(&home), expected);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn install_replaces_in_place_and_duplicates_collapse() {
        let (home, keys) = keys("ak-dup");
        let a = render(ID_A, KEY, &[], Path::new(HELPER)).unwrap();
        let b = render(ID_B, KEY, &[], Path::new(HELPER)).unwrap();
        fs::create_dir(home.join(".ssh")).unwrap();
        fs::set_permissions(home.join(".ssh"), fs::Permissions::from_mode(0o700)).unwrap();
        let old = a.replace("10.0.0.0/8,", "");
        fs::write(
            home.join(".ssh/authorized_keys"),
            format!("first\n{old}\nmiddle\n{old}\n{b}\nlast\n"),
        )
        .unwrap();
        keys.install(&peer(ID_A, "A"), &[]).unwrap();
        assert_eq!(
            String::from_utf8(contents(&home)).unwrap(),
            format!("first\n{a}\nmiddle\n{b}\nlast\n")
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn remove_all_drops_every_managed_line_and_only_those() {
        let (home, keys) = keys("ak-all");
        keys.install(&peer(ID_A, "A"), &[]).unwrap();
        keys.install(&peer(ID_B, "B"), &[]).unwrap();
        let mut text = contents(&home);
        text.extend_from_slice(b"ssh-ed25519 AAAA lazy-worker:not-an-id\nother key\n");
        fs::write(home.join(".ssh/authorized_keys"), &text).unwrap();
        keys.remove_all().unwrap();
        assert_eq!(
            contents(&home),
            b"ssh-ed25519 AAAA lazy-worker:not-an-id\nother key\n"
        );
        fs::remove_dir_all(home).unwrap();
    }

    /// A program in a directory of `mode`, under a scratch root. Returns the root and the program.
    fn program_in(label: &str, mode: u32) -> (PathBuf, PathBuf) {
        let root = crate::remote::testing::scratch(label);
        let dir = root.join("App.app");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
        let program = dir.join("lw");
        fs::write(&program, "x").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        (fs::canonicalize(root).unwrap(), fs::canonicalize(program).unwrap())
    }

    #[test]
    fn an_entry_is_safe_only_when_root_or_we_own_it_and_group_and_others_cannot_write() {
        assert!(entry_safe(0, 0, 0o755, 501));
        assert!(entry_safe(501, 20, 0o755, 501));
        assert!(entry_safe(0, ADMIN_GID, 0o775, 501), "/Applications");
        assert!(!entry_safe(501, 20, 0o775, 501), "group write on ours");
        assert!(!entry_safe(0, 0, 0o775, 501), "group write on root, not admin");
        assert!(!entry_safe(501, 20, 0o757, 501), "others write");
        assert!(!entry_safe(0, 0, 0o777, 501));
        assert!(!entry_safe(502, 20, 0o755, 501), "someone else owns it");
    }

    #[test]
    fn a_helper_in_a_group_writable_directory_is_refused_and_no_line_is_written() {
        let (root, program) = program_in("ak-unsafe", 0o775);
        assert_eq!(trusted_from(&program, &root), Err("helperPathUnsafe"));
        let home = crate::remote::testing::scratch("ak-unsafe-home");
        let keys =
            AuthorizedKeys::with_trust(home.clone(), program.clone(), |path| {
                trusted_from(path, path.parent().and_then(Path::parent).unwrap())
            })
            .unwrap();
        assert_eq!(keys.install(&peer(ID_A, "A"), &[]), Err("helperPathUnsafe"));
        assert_eq!(keys.install_pairing(&[], "202611231215"), Err("helperPathUnsafe"));
        assert_eq!(keys.refresh_all(&[peer(ID_A, "A")], &[]), Err("helperPathUnsafe"));
        assert!(!home.join(".ssh/authorized_keys").exists());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_helper_in_a_closed_directory_passes_and_a_translocated_one_does_not() {
        let (root, program) = program_in("ak-safe", 0o755);
        assert_eq!(trusted_from(&program, &root), Ok(program.clone()));
        let moved = root.join("AppTranslocation").join("x");
        fs::create_dir_all(&moved).unwrap();
        let copy = moved.join("lw");
        fs::write(&copy, "x").unwrap();
        assert_eq!(trusted_from(&copy, &root), Err("helperPathUnsafe"));
        assert_eq!(trusted_from(&root.join("missing"), &root), Err("helperPathUnsafe"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_renders_again_drops_orphans_and_adds_missing() {
        let (home, keys) = keys("ak-refresh");
        keys.install(&peer(ID_A, "A"), &[]).unwrap();
        keys.install(&peer(ID_B, "B"), &[]).unwrap();
        let moved = AuthorizedKeys::new(
            home.clone(),
            PathBuf::from("/Applications/Moved App.app/x/lw"),
        )
        .unwrap();
        moved
            .refresh_all(&[peer(ID_A, "A")], &["203.0.113.0/24".into()])
            .unwrap();
        let text = String::from_utf8(contents(&home)).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("/Applications/Moved App.app/x/lw"));
        assert!(text.contains("203.0.113.0/24"));
        assert!(!text.contains(ID_B));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_symlinked_file_or_directory_is_refused() {
        let (home, keys) = keys("ak-link");
        let target = home.join("elsewhere");
        fs::write(&target, "x\n").unwrap();
        fs::create_dir(home.join(".ssh")).unwrap();
        fs::set_permissions(home.join(".ssh"), fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&target, home.join(".ssh/authorized_keys")).unwrap();
        assert_eq!(
            keys.install(&peer(ID_A, "A"), &[]),
            Err("authorizedKeysUnsafe")
        );
        assert_eq!(keys.remove_all(), Err("authorizedKeysUnsafe"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "x\n");

        let (other, keys2) = self::keys("ak-link-dir");
        let real = other.join("real");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, other.join(".ssh")).unwrap();
        assert_eq!(keys2.install(&peer(ID_A, "A"), &[]), Err("sshDirUnsafe"));
        assert!(fs::read_dir(&real).unwrap().next().is_none());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(other).unwrap();
    }

    #[test]
    fn a_directory_that_others_can_write_is_refused() {
        let (home, keys) = keys("ak-mode");
        fs::create_dir(home.join(".ssh")).unwrap();
        fs::set_permissions(home.join(".ssh"), fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(keys.install(&peer(ID_A, "A"), &[]), Err("sshDirUnsafe"));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn an_unchanged_file_is_not_rewritten_and_a_missing_one_is_not_created_by_a_removal() {
        let (home, keys) = keys("ak-noop");
        keys.remove(ID_A).unwrap();
        assert!(!home.join(".ssh/authorized_keys").exists());
        keys.install(&peer(ID_A, "A"), &[]).unwrap();
        let before = fs::metadata(home.join(".ssh/authorized_keys"))
            .unwrap()
            .ino();
        keys.install(&peer(ID_A, "A"), &[]).unwrap();
        assert_eq!(
            fs::metadata(home.join(".ssh/authorized_keys"))
                .unwrap()
                .ino(),
            before
        );
        fs::remove_dir_all(home).unwrap();
    }
}
