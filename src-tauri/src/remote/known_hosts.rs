//! The host keys we trust, in a file of our own that only we write.
//!
//! A line reads `lw-<peer id> <type> <base64>`, one per peer. A key enters the file only when
//! pairing binds it to the peer. Any other line makes `pin` refuse, because OpenSSH reads
//! patterns, hashes, and markers that could widen which key it accepts for a peer.

use super::{destination::Destination, identity::DeviceId, ssh, write_atomic, Layout};
use std::{fs, sync::Mutex, time::Duration};

const KEY_TYPES: [&str; 2] = ["ssh-ed25519", "ecdsa-sha2-nistp256"];

/// Serializes the read, change, and write of the file inside this process.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// One host key: its type and its base64 body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKey {
    pub kind: String,
    pub key: String,
}

impl HostKey {
    pub fn new(kind: &str, key: &str) -> Result<Self, &'static str> {
        check_key(kind, key)?;
        Ok(Self {
            kind: kind.to_owned(),
            key: key.to_owned(),
        })
    }
}

/// The name under which ssh looks the peer up. It is passed as `HostKeyAlias`.
pub fn alias(peer: &DeviceId) -> String {
    format!("lw-{}", peer.as_str())
}

/// Checks the type, the base64, and that the key blob names the same type.
pub fn check_key(kind: &str, body: &str) -> Result<(), &'static str> {
    if !KEY_TYPES.contains(&kind) {
        return Err("hostKeyInvalid");
    }
    let blob = base64_decode(body).ok_or("hostKeyInvalid")?;
    // A blob starts with its type as a string of a 4-byte length and the bytes.
    let length = blob
        .get(..4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_be_bytes);
    let named = length.and_then(|length| blob.get(4..4 + length as usize));
    // The key data must follow the type name.
    if named != Some(kind.as_bytes()) || blob.len() <= 4 + kind.len() + 4 {
        return Err("hostKeyInvalid");
    }
    Ok(())
}

/// Strict standard base64 with padding. None for anything else.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 != 0 || bytes.len() > 2048 {
        return None;
    }
    let padding = bytes.iter().rev().take_while(|&&byte| byte == b'=').count();
    if padding > 2 {
        return None;
    }
    let (body, _) = bytes.split_at(bytes.len() - padding);
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let (mut accumulator, mut bits) = (0u32, 0u32);
    for &byte in body {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        accumulator = accumulator << 6 | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    // The unused bits of the last character must be zero.
    (accumulator == 0).then_some(out)
}

/// A line of the file. Only a line in our exact format is an entry.
#[derive(Debug, PartialEq)]
enum Line<'a> {
    Entry {
        alias: &'a str,
        key: HostKey,
    },
    /// A blank line or a comment.
    Inert(&'a str),
    Foreign(&'a str),
}

fn classify(line: &str) -> Line<'_> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Line::Inert(line);
    }
    let fields: Vec<&str> = line.split(' ').collect();
    if let [alias, kind, body] = fields[..] {
        let named = alias
            .strip_prefix("lw-")
            .is_some_and(|id| DeviceId::parse(id).is_ok());
        if named {
            if let Ok(key) = HostKey::new(kind, body) {
                return Line::Entry { alias, key };
            }
        }
    }
    Line::Foreign(line)
}

fn render(lines: &[String]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn read_lines(layout: &Layout) -> Result<Vec<String>, &'static str> {
    // The file holds one short line per peer, so a few MiB is far more than it ever needs.
    let path = layout.known_hosts_file();
    match fs::metadata(&path) {
        Ok(meta) if meta.len() > 4 * 1024 * 1024 => return Err("knownHostsInvalid"),
        _ => {}
    }
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return Err("remoteIo"),
    };
    Ok(text.lines().map(str::to_owned).collect())
}

/// The key pinned for a peer, if any.
pub fn pinned(layout: &Layout, peer: &DeviceId) -> Result<Option<HostKey>, &'static str> {
    let _guard = FILE_LOCK.lock().map_err(|_| "remoteIo")?;
    let wanted = alias(peer);
    Ok(read_lines(layout)?
        .iter()
        .find_map(|line| match classify(line) {
            Line::Entry { alias, key } if alias == wanted => Some(key),
            _ => None,
        }))
}

/// Pins the key for a peer and replaces the one it had. Refuses when the file holds a line
/// that we did not write.
pub fn pin(layout: &Layout, peer: &DeviceId, key: &HostKey) -> Result<(), &'static str> {
    check_key(&key.kind, &key.key)?;
    let _guard = FILE_LOCK.lock().map_err(|_| "remoteIo")?;
    let wanted = alias(peer);
    let new_line = format!("{wanted} {} {}", key.kind, key.key);
    let lines = read_lines(layout)?;
    if lines
        .iter()
        .any(|line| matches!(classify(line), Line::Foreign(_)))
    {
        return Err("knownHostsForeign");
    }
    let mut replaced = false;
    let mut out = Vec::with_capacity(lines.len() + 1);
    for line in lines {
        let ours = matches!(classify(&line), Line::Entry { alias, .. } if alias == wanted);
        if !ours {
            out.push(line);
        } else if !replaced {
            // One entry per peer: the first is replaced in place and any other is dropped.
            replaced = true;
            out.push(new_line.clone());
        }
    }
    if !replaced {
        out.push(new_line);
    }
    write_atomic(&layout.known_hosts_file(), render(&out).as_bytes())
}

/// Removes the entry of a peer. Every other line stays as it is.
pub fn unpin(layout: &Layout, peer: &DeviceId) -> Result<(), &'static str> {
    let _guard = FILE_LOCK.lock().map_err(|_| "remoteIo")?;
    let wanted = alias(peer);
    let lines = read_lines(layout)?;
    let kept: Vec<String> = lines
        .iter()
        .filter(|line| !matches!(classify(line), Line::Entry { alias, .. } if alias == wanted))
        .cloned()
        .collect();
    if kept.len() == lines.len() {
        return Ok(());
    }
    write_atomic(&layout.known_hosts_file(), render(&kept).as_bytes())
}

/// Asks the host for its ed25519 key with `ssh-keyscan`. The answer is untrusted: anyone on the
/// path can give it. It counts only after pairing binds it to the device that holds the key.
pub fn host_key(destination: &Destination) -> Result<HostKey, &'static str> {
    let mut command = ssh::tool("/usr/bin/ssh-keyscan");
    let port = destination.port().to_string();
    // The destination cannot start with a dash, and `--` ends the options before the host.
    command.args([
        "-T",
        "5",
        "-t",
        "ed25519",
        "-p",
        &port,
        "--",
        destination.host(),
    ]);
    let done = ssh::execute(command, &[], Duration::from_secs(10))?;
    if done.timed_out {
        return Err("sshTimeout");
    }
    parse_keyscan(&String::from_utf8_lossy(&done.stdout)).ok_or(if done.success {
        "sshFailed"
    } else {
        "sshUnreachable"
    })
}

/// Takes the first valid ed25519 line of `ssh-keyscan` output and ignores comments.
fn parse_keyscan(output: &str) -> Option<HostKey> {
    output
        .lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            match fields[..] {
                [_, "ssh-ed25519", body] => HostKey::new("ssh-ed25519", body).ok(),
                _ => None,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::testing::scratch;

    const ED: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIDiFbQEPp+zd0e6F/i0IEqDDrxYfKTFowPI3v8NSyB+I";
    const ED2: &str = "AAAAC3NzaC1lZDI1NTE5AAAAID8ZXmiaFqom1COnlczolZXMUWPTJ9+G8VXLl67Q9FJ9";

    fn peer(digit: char) -> DeviceId {
        DeviceId::parse(&digit.to_string().repeat(32)).unwrap()
    }

    fn layout() -> (std::path::PathBuf, Layout) {
        let base = scratch("known");
        (base.clone(), Layout::for_paths(base, None))
    }

    #[test]
    fn keys_are_checked_for_type_encoding_and_blob() {
        assert!(check_key("ssh-ed25519", ED).is_ok());
        assert!(check_key("ssh-rsa", ED).is_err());
        assert!(
            check_key("ecdsa-sha2-nistp256", ED).is_err(),
            "blob names another type"
        );
        assert!(check_key("ssh-ed25519", "").is_err());
        assert!(check_key("ssh-ed25519", &ED[..ED.len() - 1]).is_err());
        assert!(check_key("ssh-ed25519", &format!("{ED}\n")).is_err());
        assert!(
            check_key("ssh-ed25519", "AAAAC3NzaC1lZDI1NTE5").is_err(),
            "no key data"
        );
        assert!(check_key("ssh-ed25519", &ED.replace("IDiF", "ID=F")).is_err());
    }

    #[test]
    fn base64_matches_known_vectors_and_rejects_loose_input() {
        assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
        assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
        assert_eq!(base64_decode("TQ==").unwrap(), b"M");
        for bad in ["TQ=", "TQ", "T===", "TWF u", "TQ==TQ==", "TR==", "TWFu\n"] {
            assert!(base64_decode(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn pin_unpin_and_pinned_round_trip_one_entry_per_peer() {
        let (base, layout) = layout();
        let (a, b) = (peer('a'), peer('b'));
        assert_eq!(pinned(&layout, &a), Ok(None));
        unpin(&layout, &a).unwrap();
        pin(&layout, &a, &HostKey::new("ssh-ed25519", ED).unwrap()).unwrap();
        pin(&layout, &b, &HostKey::new("ssh-ed25519", ED2).unwrap()).unwrap();
        assert_eq!(pinned(&layout, &a).unwrap().unwrap().key, ED);
        pin(&layout, &a, &HostKey::new("ssh-ed25519", ED2).unwrap()).unwrap();
        assert_eq!(pinned(&layout, &a).unwrap().unwrap().key, ED2);
        let text = fs::read_to_string(layout.known_hosts_file()).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.starts_with(&format!("lw-{} ssh-ed25519 {ED2}\n", "a".repeat(32))));
        unpin(&layout, &a).unwrap();
        assert_eq!(pinned(&layout, &a), Ok(None));
        assert!(pinned(&layout, &b).unwrap().is_some());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn duplicates_collapse_and_comments_survive() {
        let (base, layout) = layout();
        let a = "a".repeat(32);
        let text = format!("# ours\nlw-{a} ssh-ed25519 {ED}\n\nlw-{a} ssh-ed25519 {ED2}\n");
        fs::write(layout.known_hosts_file(), text).unwrap();
        pin(
            &layout,
            &peer('a'),
            &HostKey::new("ssh-ed25519", ED2).unwrap(),
        )
        .unwrap();
        let after = fs::read_to_string(layout.known_hosts_file()).unwrap();
        assert_eq!(after, format!("# ours\nlw-{a} ssh-ed25519 {ED2}\n\n"));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn foreign_lines_block_pin_and_survive_unpin() {
        let (base, layout) = layout();
        let a = "a".repeat(32);
        let foreign = [
            format!("* ssh-ed25519 {ED}"),
            format!("lw-* ssh-ed25519 {ED}"),
            format!("@cert-authority lw-{a} ssh-ed25519 {ED}"),
            format!("lw-{a} ssh-rsa {ED}"),
            format!("lw-{a} ssh-ed25519 {ED} comment"),
            format!("lw-{a}  ssh-ed25519 {ED}"),
            "|1|abc|def ssh-ed25519 AAAA".to_owned(),
        ];
        for line in foreign {
            let text = format!("lw-{a} ssh-ed25519 {ED}\n{line}\n");
            fs::write(layout.known_hosts_file(), &text).unwrap();
            let key = HostKey::new("ssh-ed25519", ED2).unwrap();
            assert_eq!(
                pin(&layout, &peer('b'), &key),
                Err("knownHostsForeign"),
                "{line}"
            );
            assert_eq!(
                fs::read_to_string(layout.known_hosts_file()).unwrap(),
                text,
                "pin left the file"
            );
            unpin(&layout, &peer('a')).unwrap();
            assert_eq!(
                fs::read_to_string(layout.known_hosts_file()).unwrap(),
                format!("{line}\n")
            );
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn keyscan_refuses_a_closed_port_on_this_machine() {
        // Nothing listens on port 1, so this stays on the loopback interface.
        let destination = Destination::new("me", "127.0.0.1", Some(1)).unwrap();
        assert!(host_key(&destination).is_err());
    }

    #[test]
    fn keyscan_output_is_parsed_and_validated() {
        let good = format!("# 192.168.1.2:22 SSH-2.0-OpenSSH_9.8\n192.168.1.2 ssh-ed25519 {ED}\n");
        assert_eq!(
            parse_keyscan(&good),
            Some(HostKey::new("ssh-ed25519", ED).unwrap())
        );
        for bad in [
            "",
            "# only a comment\n",
            &format!("h ssh-rsa {ED}\n"),
            "h ssh-ed25519 AAAA\n",
            &format!("h ssh-ed25519 {ED} extra\n"),
        ] {
            assert_eq!(parse_keyscan(bad), None, "{bad:?}");
        }
    }
}
