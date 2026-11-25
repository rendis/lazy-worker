//! The identity of this device: a random id and an ed25519 key pair.
//!
//! The key has no passphrase. A key protected by the Keychain is a later option.

use super::{check_private_file, known_hosts, read_small, write_atomic, write_new, Layout};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
    process::{Command, Stdio},
};

/// 16 random bytes in lowercase hex. It names a device for good.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let valid = text.len() == 32
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        valid
            .then(|| Self(text.to_owned()))
            .ok_or("deviceIdInvalid")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether the folder is out of Time Machine backups. A backup restored on another Mac must not
/// bring this identity along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupExclusion {
    Excluded,
    Failed,
}

#[derive(Clone, Debug)]
pub struct Identity {
    pub device_id: DeviceId,
    /// `ssh-ed25519 <base64>`, with no comment.
    pub public_key: String,
    #[cfg_attr(not(test), allow(dead_code))] // The Devices view reads it in a later step.
    pub backup: BackupExclusion,
}

/// Reads the identity, and creates what is missing. An existing id or key is never replaced.
/// It can wait on `tmutil` for seconds, so call it off the main thread.
pub fn load_or_create(layout: &Layout) -> Result<Identity, &'static str> {
    let backup = exclude_from_backup(layout.remote_dir());
    let device_id = load_or_create_device_id(&layout.device_id_file())?;
    let public_key = load_or_create_key(layout, &device_id)?;
    Ok(Identity {
        device_id,
        public_key,
        backup,
    })
}

fn load_or_create_device_id(path: &Path) -> Result<DeviceId, &'static str> {
    if let Some(text) = read_small(path)? {
        return DeviceId::parse(text.trim_end_matches('\n'));
    }
    let id = DeviceId::parse(&random_hex()?)?;
    write_new(path, format!("{}\n", id.0).as_bytes())?;
    // A second writer may have won the race, so read back the stored one.
    let text = read_small(path)?.ok_or("remoteIo")?;
    DeviceId::parse(text.trim_end_matches('\n'))
}

/// A new random id, for a name that lives only for a moment.
pub(super) fn random_id() -> Result<DeviceId, &'static str> {
    DeviceId::parse(&random_hex()?)
}

fn random_hex() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| "remoteIo")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn load_or_create_key(layout: &Layout, id: &DeviceId) -> Result<String, &'static str> {
    let private = layout.private_key();
    let public = private.with_extension("pub");
    if fs::symlink_metadata(&private).is_err() {
        generate_key(&private, &public, id)?;
    }
    check_private_file(&private).map_err(|_| "identityKeyUnsafe")?;
    let line = match read_small(&public)? {
        Some(line) => line,
        None => {
            let line = keygen(&["-y", "-f", path_str(&private)?])?;
            write_atomic(&public, line.as_bytes())?;
            line
        }
    };
    parse_public_key(&line)
}

/// Makes the pair under temporary names and links them into place, so a failure or a second
/// writer never leaves half a key or replaces one.
fn generate_key(private: &Path, public: &Path, id: &DeviceId) -> Result<(), &'static str> {
    let temp = private.with_file_name(format!(".id_ed25519.tmp-{}", std::process::id()));
    let temp_public = temp.with_file_name(format!(".id_ed25519.tmp-{}.pub", std::process::id()));
    let cleanup = || {
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(&temp_public);
    };
    cleanup();
    let comment = format!("lazy-worker:{}", id.0);
    let made = keygen(&[
        "-q",
        "-t",
        "ed25519",
        "-N",
        "",
        "-C",
        &comment,
        "-f",
        path_str(&temp)?,
    ]);
    let result = made.and_then(|_| {
        check_private_file(&temp).map_err(|_| "identityKeyUnsafe")?;
        fs::hard_link(&temp, private).map_err(|_| "identityKeyExists")?;
        // The public half is derived again when it is missing, so a failure here is not fatal.
        let _ = fs::hard_link(&temp_public, public);
        Ok(())
    });
    cleanup();
    result
}

fn path_str(path: &Path) -> Result<&str, &'static str> {
    path.to_str().ok_or("remoteIo")
}

/// Runs `ssh-keygen` with a fixed path, no shell, and no input.
fn keygen(args: &[&str]) -> Result<String, &'static str> {
    let output = Command::new("/usr/bin/ssh-keygen")
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "keygenFailed")?;
    if !output.status.success() {
        return Err("keygenFailed");
    }
    String::from_utf8(output.stdout).map_err(|_| "keygenFailed")
}

/// Accepts one `ssh-ed25519` line and returns it without its comment.
pub fn parse_public_key(line: &str) -> Result<String, &'static str> {
    let mut fields = line.split_whitespace();
    let (Some(kind), Some(body)) = (fields.next(), fields.next()) else {
        return Err("identityKeyInvalid");
    };
    if kind != "ssh-ed25519" || known_hosts::check_key(kind, body).is_err() {
        return Err("identityKeyInvalid");
    }
    Ok(format!("{kind} {body}"))
}

/// The extended attribute that marks a sticky Time Machine exclusion.
const EXCLUDE_ATTRIBUTE: &str = "com.apple.metadata:com_apple_backup_excludeItem";

/// Sets the sticky Time Machine exclusion, which needs no root. `tmutil` can take ten seconds,
/// so a folder that already has the mark is not sent to it again. A failure is only a status.
pub fn exclude_from_backup(directory: &Path) -> BackupExclusion {
    let Some(path) = directory.to_str() else {
        return BackupExclusion::Failed;
    };
    let succeeds = |program: &str, args: &[&str]| {
        super::ssh::tool(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    let done = succeeds("/usr/bin/xattr", &["-p", EXCLUDE_ATTRIBUTE, path])
        || succeeds("/usr/bin/tmutil", &["addexclusion", path]);
    if done {
        BackupExclusion::Excluded
    } else {
        BackupExclusion::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{ensure_private_dir, testing::scratch};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDiFbQEPp+zd0e6F/i0IEqDDrxYfKTFowPI3v8NSyB+I lazy-worker:x";

    #[test]
    fn device_id_needs_32_lowercase_hex_characters() {
        assert!(DeviceId::parse(&"0a".repeat(16)).is_ok());
        for bad in [
            "",
            &"0a".repeat(15),
            &"0A".repeat(16),
            &"0g".repeat(16),
            &"0a".repeat(17),
            "../../etc/passwd",
        ] {
            assert_eq!(DeviceId::parse(bad), Err("deviceIdInvalid"), "{bad}");
        }
    }

    #[test]
    fn public_key_line_is_checked_and_stripped() {
        assert_eq!(
            parse_public_key(KEY).unwrap(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDiFbQEPp+zd0e6F/i0IEqDDrxYfKTFowPI3v8NSyB+I"
        );
        for bad in [
            "",
            "ssh-ed25519",
            "ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIDiFbQEPp+zd0e6F/i0IEqDDrxYfKTFowPI3v8NSyB+I",
            "ssh-ed25519 AAAA",
            "ssh-ed25519 not*base64!",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJ",
        ] {
            assert_eq!(parse_public_key(bad), Err("identityKeyInvalid"), "{bad}");
        }
    }

    #[test]
    fn identity_is_created_once_and_reused() {
        if !Path::new("/usr/bin/ssh-keygen").exists() {
            return;
        }
        let base = scratch("identity");
        let remote = base.join("remote");
        ensure_private_dir(&remote).unwrap();
        let layout = Layout::for_paths(remote, None);
        let first = load_or_create(&layout).unwrap();
        eprintln!("backup exclusion: {:?}", first.backup);
        assert!(matches!(
            first.backup,
            BackupExclusion::Excluded | BackupExclusion::Failed
        ));
        let mode = |path: &Path| fs::metadata(path).unwrap().mode() & 0o777;
        assert_eq!(mode(layout.remote_dir()), 0o700);
        assert_eq!(mode(&layout.private_key()), 0o600);
        assert_eq!(mode(&layout.device_id_file()), 0o600);
        assert!(first.public_key.starts_with("ssh-ed25519 "));
        assert_eq!(first.public_key.split(' ').count(), 2);
        let public = fs::read_to_string(layout.private_key().with_extension("pub")).unwrap();
        assert!(public
            .trim_end()
            .ends_with(&format!("lazy-worker:{}", first.device_id.as_str())));
        let names: Vec<_> = fs::read_dir(layout.remote_dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            names
                .iter()
                .all(|name| !name.to_string_lossy().contains("tmp")),
            "{names:?}"
        );

        let second = load_or_create(&layout).unwrap();
        assert_eq!(first.device_id, second.device_id);
        assert_eq!(first.public_key, second.public_key);

        // A lost public half is derived again from the private key.
        fs::remove_file(layout.private_key().with_extension("pub")).unwrap();
        assert_eq!(
            load_or_create(&layout).unwrap().public_key,
            first.public_key
        );

        // Loose key permissions and a malformed id are refused, not repaired.
        fs::set_permissions(layout.private_key(), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_or_create(&layout).unwrap_err(), "identityKeyUnsafe");
        fs::set_permissions(layout.private_key(), fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(layout.device_id_file(), "nope\n").unwrap();
        assert_eq!(load_or_create(&layout).unwrap_err(), "deviceIdInvalid");
        fs::remove_dir_all(base).unwrap();
    }
}
