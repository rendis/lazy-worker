//! Reaching the user's other devices over the system SSH client.
//!
//! The code here builds on three rules. We read no SSH configuration of the user. We trust only
//! the keys we pinned. Text from outside never becomes an SSH option or a remote command.
#![cfg_attr(not(test), allow(dead_code))] // The callers arrive with pairing.

pub mod access;
pub mod authorized_keys;
pub mod destination;
pub mod helper;
pub mod identity;
pub mod known_hosts;
pub mod log;
pub mod peers;
pub mod server;
pub mod service;
pub mod ssh;
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
    /// The private directory of the multiplexing sockets. None when no socket path fits.
    control: Option<PathBuf>,
}

impl Layout {
    /// Creates the private directories under the app config directory and the temp directory.
    pub fn open(config_dir: &Path, temp_dir: &Path) -> Result<Self, &'static str> {
        let remote = config_dir.join("remote");
        ensure_private_dir(&remote)?;
        let mut control = None;
        for candidate in control_candidates(temp_dir, our_uid()) {
            if socket_fits(&candidate) && ensure_private_dir(&candidate).is_ok() {
                control = Some(candidate);
                break;
            }
        }
        Ok(Self { remote, control })
    }

    #[cfg(test)]
    pub fn for_paths(remote: PathBuf, control: Option<PathBuf>) -> Self {
        Self { remote, control }
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

    pub fn socket_file(&self) -> PathBuf {
        self.remote.join("app.sock")
    }

    pub fn log_file(&self) -> PathBuf {
        self.remote.join("log.jsonl")
    }

    pub fn control_dir(&self) -> Option<&Path> {
        self.control.as_deref()
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
