//! What other devices asked of this one, and what happened to it.
//!
//! One JSON object per line, appended. An entry names the request, never its arguments.
//! The file holds the newest 1000 entries, and a rewrite drops the oldest. Nothing clears it.

use super::{check_private_file, now, write_atomic};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::Mutex,
};

/// The entries that reading returns, and that a rewrite keeps.
pub const MAX_ENTRIES: usize = 1000;
/// The file may grow this far past the limit before a rewrite, so a rewrite is not on every request.
const SLACK: usize = 100;
const MAX_FILE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Unix seconds.
    pub time: u64,
    /// The id of the device, or nothing for an event that names none.
    pub peer: Option<String>,
    /// The name the device had at that moment.
    pub name: Option<String>,
    /// The `command` tag of a request, or the event: `removePeer` or `cutAll`.
    pub request: String,
    /// `ok` or the error code.
    pub outcome: String,
}

impl Entry {
    pub fn new(peer: Option<&str>, name: Option<&str>, request: &str, outcome: &str) -> Self {
        Self {
            time: now(),
            peer: peer.map(str::to_owned),
            name: name.map(str::to_owned),
            request: request.to_owned(),
            outcome: outcome.to_owned(),
        }
    }
}

pub struct Log {
    path: PathBuf,
    /// The lines in the file.
    lines: Mutex<usize>,
}

impl Log {
    /// Opens the file. One that is not private and ours, or too big to read, is refused.
    pub fn open(path: PathBuf) -> Result<Self, &'static str> {
        let lines = match fs::symlink_metadata(&path) {
            Ok(meta) => {
                check_private_file(&path)?;
                if meta.len() > MAX_FILE {
                    return Err("remoteFileInvalid");
                }
                fs::read(&path)
                    .map_err(|_| "remoteIo")?
                    .iter()
                    .filter(|&&byte| byte == b'\n')
                    .count()
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(_) => return Err("remoteIo"),
        };
        Ok(Self {
            path,
            lines: Mutex::new(lines),
        })
    }

    /// Adds one entry. A failure is returned, and the caller decides whether it matters.
    pub fn append(&self, entry: &Entry) -> Result<(), &'static str> {
        let mut line = serde_json::to_vec(entry).map_err(|_| "remoteIo")?;
        line.push(b'\n');
        let mut lines = self.lines.lock().unwrap_or_else(|error| error.into_inner());
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.path)
            .map_err(|_| "remoteIo")?;
        file.write_all(&line).map_err(|_| "remoteIo")?;
        *lines += 1;
        if *lines > MAX_ENTRIES + SLACK {
            let kept = self.read_file();
            let mut bytes = Vec::new();
            for kept in &kept {
                bytes.extend(serde_json::to_vec(kept).map_err(|_| "remoteIo")?);
                bytes.push(b'\n');
            }
            write_atomic(&self.path, &bytes)?;
            *lines = kept.len();
        }
        Ok(())
    }

    /// The newest `MAX_ENTRIES`, oldest first. A line that does not parse is skipped.
    fn read_file(&self) -> Vec<Entry> {
        let text = fs::read_to_string(&self.path).unwrap_or_default();
        let mut entries: Vec<Entry> = text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }
        entries
    }

    /// The entries, newest first.
    pub fn read(&self) -> Vec<Entry> {
        let _guard = self.lines.lock().unwrap_or_else(|error| error.into_inner());
        let mut entries = self.read_file();
        entries.reverse();
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::testing::scratch;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn entries_are_appended_and_read_newest_first() {
        let dir = scratch("log-basic");
        let log = Log::open(dir.join("log.jsonl")).unwrap();
        assert!(log.read().is_empty());
        log.append(&Entry::new(Some("a"), Some("Studio"), "getStatuses", "ok"))
            .unwrap();
        log.append(&Entry::new(
            Some("a"),
            Some("Studio"),
            "setTheme",
            "notPermitted",
        ))
        .unwrap();
        let entries = log.read();
        assert_eq!(entries[0].request, "setTheme");
        assert_eq!(entries[0].outcome, "notPermitted");
        assert_eq!(entries[1].name.as_deref(), Some("Studio"));
        assert_eq!(
            fs::metadata(dir.join("log.jsonl")).unwrap().mode() & 0o777,
            0o600
        );
        let raw = fs::read_to_string(dir.join("log.jsonl")).unwrap();
        assert!(raw.contains("\"request\":\"getStatuses\""));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_oldest_entries_are_dropped_past_the_limit() {
        let dir = scratch("log-limit");
        let path = dir.join("log.jsonl");
        let log = Log::open(path.clone()).unwrap();
        for index in 0..(MAX_ENTRIES + SLACK + 5) {
            log.append(&Entry::new(None, None, &format!("r{index}"), "ok"))
                .unwrap();
        }
        let entries = log.read();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].request, format!("r{}", MAX_ENTRIES + SLACK + 4));
        assert_eq!(entries.last().unwrap().request, format!("r{}", SLACK + 5));
        let on_disk = fs::read_to_string(&path).unwrap().lines().count();
        assert!(on_disk <= MAX_ENTRIES + SLACK);
        let reopened = Log::open(path).unwrap();
        assert_eq!(reopened.read().len(), MAX_ENTRIES);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_broken_line_is_skipped_and_an_unsafe_file_is_refused() {
        let dir = scratch("log-bad");
        let path = dir.join("log.jsonl");
        fs::write(&path, "garbage\n{\"time\":1,\"peer\":null,\"name\":null,\"request\":\"cutAll\",\"outcome\":\"ok\"}\n").unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let log = Log::open(path.clone()).unwrap();
        assert_eq!(log.read().len(), 1);
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        assert!(Log::open(path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
