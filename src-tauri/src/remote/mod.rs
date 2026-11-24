//! Reaching the user's other devices over the system SSH client.
//!
//! The code here builds on three rules. We read no SSH configuration of the user. We trust only
//! the keys we pinned. Text from outside never becomes an SSH option or a remote command.

pub mod access;
pub mod authorized_keys;
pub mod client;
pub mod controlled;
pub mod destination;
pub mod discovery;
pub mod helper;
pub mod icons;
pub mod identity;
pub mod known_hosts;
pub mod log;
pub mod pair_client;
pub mod pake;
pub mod pairing;
pub mod pairing_key;
pub mod peers;
pub mod server;
pub mod service;
pub mod ssh;
pub mod window;
pub mod wire;

use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::{ErrorKind, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

/// Where everything of this feature lives on disk.
#[derive(Clone, Debug)]
pub struct Layout {
    remote: PathBuf,
    /// The app socket. It is in `remote`, or in a short private directory when that path is too long.
    socket: PathBuf,
    /// The private directory of the multiplexing sockets. None when no socket path fits.
    control: Option<PathBuf>,
}

impl Layout {
    /// Creates the private directories under the app config directory and the temp directory.
    pub fn open(config_dir: &Path, temp_dir: &Path) -> Result<Self, &'static str> {
        let remote = config_dir.join("remote");
        ensure_private_dir(&remote)?;
        let socket = socket_path(&remote, our_uid());
        if let Some(directory) = socket.parent().filter(|_| !socket.starts_with(&remote)) {
            ensure_private_dir(directory)?;
        }
        let mut control = None;
        for candidate in control_candidates(temp_dir, our_uid()) {
            if socket_fits(&candidate) && ensure_private_dir(&candidate).is_ok() {
                control = Some(candidate);
                break;
            }
        }
        Ok(Self {
            remote,
            socket,
            control,
        })
    }

    #[cfg(test)]
    pub fn for_paths(remote: PathBuf, control: Option<PathBuf>) -> Self {
        let socket = socket_path(&remote, our_uid());
        Self {
            remote,
            socket,
            control,
        }
    }

    pub fn remote_dir(&self) -> &Path {
        &self.remote
    }

    pub fn device_id_file(&self) -> PathBuf {
        self.remote.join("device-id")
    }

    pub fn private_key(&self) -> PathBuf {
        self.remote.join("id_ed25519")
    }

    pub fn config_file(&self) -> PathBuf {
        self.remote.join("ssh_config")
    }

    pub fn known_hosts_file(&self) -> PathBuf {
        self.remote.join("known_hosts")
    }

    pub fn peers_file(&self) -> PathBuf {
        self.remote.join("peers.json")
    }

    /// The devices that this one controls.
    pub fn devices_file(&self) -> PathBuf {
        self.remote.join("devices.json")
    }

    /// The fixed pairing key, written before a pairing because ssh wants a private file.
    pub fn pairing_key_file(&self) -> PathBuf {
        self.remote.join("pairing_key")
    }

    pub fn pairing_config_file(&self) -> PathBuf {
        self.remote.join("ssh_config_pair")
    }

    /// Icons of the applications of other devices, under file names that are hashes.
    pub fn icons_dir(&self) -> PathBuf {
        self.remote.join("icons")
    }

    pub fn socket_file(&self) -> PathBuf {
        self.socket.clone()
    }

    pub fn log_file(&self) -> PathBuf {
        self.remote.join("log.jsonl")
    }

    pub fn control_dir(&self) -> Option<&Path> {
        self.control.as_deref()
    }
}

/// The longest socket path: `sun_path` holds 104 bytes with its end mark on macOS.
pub(crate) const SOCKET_PATH_MAX: usize = 103;

/// Where the app socket is, given the `remote` directory of the app. The helper and the app
/// both apply this one rule, so they agree without talking. A path that is too long for a
/// socket moves to `/tmp/lw-<uid>`, a private directory.
pub(crate) fn socket_path(remote_dir: &Path, uid: u32) -> PathBuf {
    let own = remote_dir.join("app.sock");
    if own.as_os_str().len() <= SOCKET_PATH_MAX {
        own
    } else {
        PathBuf::from(format!("/tmp/lw-{uid}/app.sock"))
    }
}

/// The per-user temp directory first. Its path is too long for a socket on macOS, so a private
/// directory in `/tmp` follows. Its sticky bit stops other users from replacing ours.
fn control_candidates(temp_dir: &Path, uid: u32) -> Vec<PathBuf> {
    vec![
        temp_dir.join("lw-ssh"),
        PathBuf::from(format!("/tmp/lw-{uid}")),
    ]
}

/// `sun_path` holds 104 bytes with its end mark on macOS. ssh binds a temporary name first:
/// the path, a dot, and 16 random characters. The name is a 32-character device id.
fn socket_fits(directory: &Path) -> bool {
    directory.as_os_str().len() + 1 + 32 + 17 <= 103
}

extern "C" {
    fn geteuid() -> u32;
}

#[cfg(target_os = "macos")]
extern "C" {
    fn getpeereid(fd: i32, uid: *mut u32, gid: *mut u32) -> i32;
}

/// The user id of the process at the other end of a Unix socket. Nothing where the system gives
/// none, and the caller then refuses the connection.
#[cfg(target_os = "macos")]
pub(crate) fn peer_uid(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let (mut uid, mut gid) = (0u32, 0u32);
    // SAFETY: the descriptor is open for the whole call, and both pointers are valid for writes.
    let result = unsafe { getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    (result == 0).then_some(uid)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn peer_uid(_stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    None
}

/// Unix seconds now.
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The user id of this process.
pub(crate) fn our_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, cannot fail, and touches no memory.
    unsafe { geteuid() }
}

/// Creates the directory with mode 0700. Fixes the mode of an existing one that we own and
/// refuses one that someone else owns or that is a link.
pub(crate) fn ensure_private_dir(path: &Path) -> Result<(), &'static str> {
    match DirBuilder::new().recursive(true).mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return Err("remoteIo"),
    }
    let meta = fs::symlink_metadata(path).map_err(|_| "remoteIo")?;
    if !meta.is_dir() {
        return Err("remoteDirInvalid");
    }
    if meta.uid() != our_uid() {
        return Err("remoteDirOwner");
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| "remoteIo")?;
    }
    Ok(())
}

/// Checks a regular file that we own and that group and others cannot touch.
pub(crate) fn check_private_file(path: &Path) -> Result<(), &'static str> {
    let meta = fs::symlink_metadata(path).map_err(|_| "remoteIo")?;
    if !meta.is_file() || meta.uid() != our_uid() || meta.mode() & 0o077 != 0 {
        return Err("remoteFileUnsafe");
    }
    Ok(())
}

static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A temporary file next to `path`, mode 0600, with the bytes written and flushed to disk.
fn write_temp(path: &Path, bytes: &[u8]) -> Result<PathBuf, &'static str> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("remoteIo")?;
    let temp = path.with_file_name(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let write = || -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    write().map_err(|_| {
        let _ = fs::remove_file(&temp);
        "remoteIo"
    })?;
    Ok(temp)
}

/// Replaces the file in one step.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    let temp = write_temp(path, bytes)?;
    fs::rename(&temp, path).map_err(|_| {
        let _ = fs::remove_file(&temp);
        "remoteIo"
    })
}

/// Creates the file in one step and never replaces one. Returns false when it exists.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<bool, &'static str> {
    let temp = write_temp(path, bytes)?;
    let result = fs::hard_link(&temp, path);
    let _ = fs::remove_file(&temp);
    match result {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(_) => Err("remoteIo"),
    }
}

/// Reads a small file, and nothing when it does not exist.
pub(crate) fn read_small(path: &Path) -> Result<Option<String>, &'static str> {
    match fs::read_to_string(path) {
        Ok(text) if text.len() <= 64 * 1024 => Ok(Some(text)),
        Ok(_) => Err("remoteFileInvalid"),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err("remoteIo"),
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
    };

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A writer whose bytes the test reads back.
    #[derive(Clone, Default)]
    pub struct Sink(pub std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Sink {
        pub fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A fresh directory under the system temp directory. The caller removes it.
    pub fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lw-test-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_length_rule_matches_the_macos_limit() {
        assert!(socket_fits(Path::new("/tmp/lw-501")));
        // A typical per-user temp directory is 55 characters with our folder.
        assert!(!socket_fits(Path::new(
            "/var/folders/_m/qwqcplzs1s7_6wd4vz4qf0180000gn/T/lw-ssh"
        )));
    }

    #[test]
    fn the_socket_stays_in_the_app_directory_when_its_path_fits() {
        let remote = Path::new("/Users/me/Library/Application Support/com.rendis.lazyworker/remote");
        assert_eq!(socket_path(remote, 501), remote.join("app.sock"));
        // 103 bytes is the longest that binds.
        let edge = format!("/{}/app.sock", "a".repeat(SOCKET_PATH_MAX - "/app.sock".len() - 1));
        assert_eq!(edge.len(), SOCKET_PATH_MAX);
        let edge_dir = Path::new(&edge).parent().unwrap().to_owned();
        assert_eq!(socket_path(&edge_dir, 501), Path::new(&edge));
    }

    #[test]
    fn a_socket_path_that_is_too_long_moves_to_a_private_directory_for_both_sides() {
        let long = format!("/Users/{}/Library/Application Support/com.rendis.lazyworker/remote", "n".repeat(60));
        assert!(Path::new(&long).join("app.sock").as_os_str().len() > SOCKET_PATH_MAX);
        assert_eq!(socket_path(Path::new(&long), 501), Path::new("/tmp/lw-501/app.sock"));
        // One byte over the edge moves it too.
        let over = format!("/{}", "a".repeat(SOCKET_PATH_MAX - "/app.sock".len()));
        assert_eq!(socket_path(Path::new(&over), 7), Path::new("/tmp/lw-7/app.sock"));
    }

    #[test]
    fn the_app_and_the_helper_agree_on_the_socket_in_both_cases() {
        // A short home: the socket is in the app directory.
        let short = Path::new("/Users/me");
        let remote = short
            .join("Library/Application Support")
            .join(helper::IDENTIFIER)
            .join("remote");
        assert_eq!(helper::socket_path(short), socket_path(&remote, our_uid()));
        assert_eq!(helper::socket_path(short), remote.join("app.sock"));
        // A long home: both fall back to the same directory.
        let long = PathBuf::from(format!("/Users/{}", "n".repeat(60)));
        let remote = long
            .join("Library/Application Support")
            .join(helper::IDENTIFIER)
            .join("remote");
        let expected = PathBuf::from(format!("/tmp/lw-{}/app.sock", our_uid()));
        assert_eq!(helper::socket_path(&long), expected);
        assert_eq!(socket_path(&remote, our_uid()), expected);
    }

    #[test]
    fn a_layout_with_a_long_path_binds_its_socket_in_a_private_directory() {
        let base = testing::scratch("long-sock");
        let long = base.join("x".repeat(80));
        fs::create_dir_all(&long).unwrap();
        let layout = Layout::open(&long, &std::env::temp_dir()).unwrap();
        let socket = layout.socket_file();
        assert_eq!(socket, PathBuf::from(format!("/tmp/lw-{}/app.sock", our_uid())));
        let directory = socket.parent().unwrap();
        assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(directory).unwrap().uid(), our_uid());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn layout_opens_private_directories_with_a_socket_path_that_fits() {
        let base = testing::scratch("open");
        let layout = Layout::open(&base, &std::env::temp_dir()).unwrap();
        assert_eq!(
            fs::metadata(layout.remote_dir()).unwrap().mode() & 0o777,
            0o700
        );
        let control = layout.control_dir().expect("a socket directory fits");
        assert!(socket_fits(control));
        assert_eq!(fs::metadata(control).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(control).unwrap().uid(), our_uid());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn private_dir_is_created_and_fixed() {
        let base = testing::scratch("dir");
        let dir = base.join("a").join("remote");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn private_dir_refuses_a_link_and_a_file() {
        let base = testing::scratch("link");
        std::os::unix::fs::symlink(&base, base.join("link")).unwrap();
        assert_eq!(
            ensure_private_dir(&base.join("link")),
            Err("remoteDirInvalid")
        );
        fs::write(base.join("file"), "x").unwrap();
        assert_eq!(
            ensure_private_dir(&base.join("file")),
            Err("remoteDirInvalid")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn writes_are_private_and_new_files_are_never_replaced() {
        let base = testing::scratch("write");
        let path = base.join("f");
        assert_eq!(write_new(&path, b"one"), Ok(true));
        assert_eq!(write_new(&path, b"two"), Ok(false));
        assert_eq!(fs::read(&path).unwrap(), b"one");
        write_atomic(&path, b"three").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"three");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            1,
            "no temp file is left"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(check_private_file(&path), Err("remoteFileUnsafe"));
        fs::remove_dir_all(base).unwrap();
    }
}
