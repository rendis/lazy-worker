//! What other devices asked of this one, and what happened to it.
//!
//! One JSON object per line, appended. An entry names the request, never its arguments.
//! Two files keep it. Security events (pairing, removal, shell grants, runs of actions, changes of the
//! key file) have a file of their own, so a flood of refused requests can never push them out. Every
//! other entry goes to the ordinary file. A refusal that repeats within a minute is one line with a
//! count. Each file holds the newest 1000 entries, and a rewrite drops the oldest. Nothing clears them.

use super::{check_private_file, now, write_atomic};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

/// The entries that reading returns from each file, and that a rewrite keeps.
pub const MAX_ENTRIES: usize = 1000;
/// The file may grow this far past the limit before a rewrite, so a rewrite is not on every request.
const SLACK: usize = 100;
const MAX_FILE: u64 = 4 * 1024 * 1024;
/// Seconds in which a repeated refusal is counted on one line.
const COALESCE: u64 = 60;
/// The most refusals that wait for their minute to end.
const MAX_PENDING: usize = 256;

/// The events that the security file keeps. Only this app writes them, through `append`. A request that a device
/// sends goes through `append_request`, so it can never take the name of one of them.
const SECURITY: &[&str] = &[
    "paired",
    "pair",
    "pairDevice",
    "forgetDevice",
    "removePeer",
    "cutAll",
    "shellGranted",
    "shellRevoked",
    "shellExpired",
    "shellStart",
    "shellEnd",
    "keysChanged",
    "actionStart",
    "actionEnd",
    "actionSkipped",
    "actionScheduleCleared",
    "peerPermissions",
];

fn default_count() -> u32 {
    1
}

fn one(count: &u32) -> bool {
    *count <= 1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Unix seconds.
    pub time: u64,
    /// Orders entries of the same second, across both files.
    #[serde(default)]
    pub seq: u64,
    /// The id of the device, or nothing for an event that names none.
    pub peer: Option<String>,
    /// The name the device had at that moment.
    pub name: Option<String>,
    /// The `command` tag of a request, or the event: `removePeer` or `cutAll`.
    pub request: String,
    /// `ok` or the error code.
    pub outcome: String,
    /// How many times this happened in the minute that began at `time`.
    #[serde(default = "default_count", skip_serializing_if = "one")]
    pub count: u32,
}

impl Entry {
    pub fn new(peer: Option<&str>, name: Option<&str>, request: &str, outcome: &str) -> Self {
        Self {
            time: now(),
            seq: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(0)),
            peer: peer.map(str::to_owned),
            name: name.map(str::to_owned),
            request: request.to_owned(),
            outcome: outcome.to_owned(),
            count: 1,
        }
    }

    /// Whether the entry belongs in the file that refused requests cannot fill.
    fn is_security(&self) -> bool {
        // A device that has no shell can ask for one as often as it likes.
        SECURITY.contains(&self.request.as_str()) && !(self.request == "shellStart" && self.outcome == "notPermitted")
    }
}

/// One file of entries.
struct Ring {
    path: PathBuf,
    /// The lines in the file.
    lines: Mutex<usize>,
}

impl Ring {
    /// Opens the file. One that is not private and ours, or too big to read, is refused.
    fn open(path: PathBuf) -> Result<Self, &'static str> {
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

    fn append(&self, entry: &Entry) -> Result<(), &'static str> {
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

    fn read(&self) -> Vec<Entry> {
        let _guard = self.lines.lock().unwrap_or_else(|error| error.into_inner());
        self.read_file()
    }
}

pub struct Log {
    ordinary: Ring,
    security: Ring,
    /// Refusals that wait for their minute to end, so repeats can be counted on one line.
    pending: Mutex<HashMap<(Option<String>, String, String), Entry>>,
}

impl Log {
    /// Opens the files. One that is not private and ours, or too big to read, is refused.
    pub fn open(path: PathBuf) -> Result<Self, &'static str> {
        let security = path.with_extension("security.jsonl");
        Ok(Self {
            ordinary: Ring::open(path)?,
            security: Ring::open(security)?,
            pending: Mutex::default(),
        })
    }

    /// Adds an event of this app. A failure is returned, and the caller decides whether it matters.
    pub fn append(&self, entry: &Entry) -> Result<(), &'static str> {
        if entry.is_security() {
            return self.security.append(entry);
        }
        self.append_request(entry)
    }

    /// Adds a request that a device sent. Its name is the device's own word, so it never reaches the security file.
    pub fn append_request(&self, entry: &Entry) -> Result<(), &'static str> {
        if entry.outcome == "ok" {
            return self.ordinary.append(entry);
        }
        let mut pending = self.pending.lock().unwrap_or_else(|error| error.into_inner());
        let mut outcome = Self::write_due(&self.ordinary, &mut pending, entry.time, false);
        let key = (entry.peer.clone(), entry.request.clone(), entry.outcome.clone());
        match pending.get_mut(&key) {
            Some(first) => first.count = first.count.saturating_add(1),
            None => {
                pending.insert(key, entry.clone());
            }
        }
        if pending.len() >= MAX_PENDING {
            outcome = outcome.and(Self::write_due(&self.ordinary, &mut pending, entry.time, true));
        }
        outcome
    }

    /// Writes the refusals whose minute is over, or all of them.
    fn write_due(
        ring: &Ring,
        pending: &mut HashMap<(Option<String>, String, String), Entry>,
        at: u64,
        all: bool,
    ) -> Result<(), &'static str> {
        let mut due: Vec<Entry> = Vec::new();
        pending.retain(|_, entry| {
            let over = all || at.saturating_sub(entry.time) >= COALESCE;
            if over {
                due.push(entry.clone());
            }
            !over
        });
        due.sort_by_key(|entry| (entry.time, entry.seq));
        let mut outcome = Ok(());
        for entry in &due {
            outcome = outcome.and(ring.append(entry));
        }
        outcome
    }

    /// Writes what waits for its minute, when the app quits.
    pub fn flush(&self) {
        let mut pending = self.pending.lock().unwrap_or_else(|error| error.into_inner());
        let _ = Self::write_due(&self.ordinary, &mut pending, now(), true);
    }

    /// The entries of both files, newest first.
    pub fn read(&self) -> Vec<Entry> {
        self.flush();
        let mut entries = self.ordinary.read();
        entries.extend(self.security.read());
        entries.sort_by_key(|entry| std::cmp::Reverse((entry.time, entry.seq)));
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

    #[test]
    fn a_flood_of_refusals_never_pushes_a_security_event_out() {
        let dir = scratch("log-security");
        let log = Log::open(dir.join("log.jsonl")).unwrap();
        log.append(&Entry::new(Some("a"), Some("Studio"), "shellGranted", "ok")).unwrap();
        log.append(&Entry::new(Some("a"), Some("Studio"), "cutAll", "ok")).unwrap();
        for index in 0..2000 {
            // A device names its own request, so it can send the name of an event too.
            let name = if index % 2 == 0 { "probe" } else { "shellGranted" };
            log.append_request(&Entry::new(Some(&format!("peer{}", index % 50)), None, name, "notPermitted")).unwrap();
            log.append(&Entry::new(Some("b"), None, "shellStart", "notPermitted")).unwrap();
        }
        let entries = log.read();
        assert!(entries.iter().any(|entry| entry.request == "cutAll"));
        assert_eq!(entries.iter().filter(|entry| entry.request == "shellGranted" && entry.outcome == "ok").count(), 1);
        assert_eq!(
            entries.iter().filter(|entry| entry.request == "shellGranted" && entry.outcome == "notPermitted").count(),
            25,
            "a request that carries an event's name stays an ordinary entry, one line per device and name"
        );
        let on_disk = fs::read_to_string(dir.join("log.security.jsonl")).unwrap();
        assert!(on_disk.contains("cutAll") && !on_disk.contains("notPermitted"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_refusal_that_repeats_is_one_line_with_a_count() {
        let dir = scratch("log-coalesce");
        let log = Log::open(dir.join("log.jsonl")).unwrap();
        for _ in 0..7 {
            log.append_request(&Entry::new(Some("a"), Some("Studio"), "setTheme", "notPermitted")).unwrap();
        }
        log.append_request(&Entry::new(Some("a"), Some("Studio"), "setTheme", "badRequest")).unwrap();
        log.append_request(&Entry::new(Some("b"), Some("Mini"), "setTheme", "notPermitted")).unwrap();
        let entries = log.read();
        assert_eq!(entries.len(), 3);
        let counts: Vec<_> = entries.iter().map(|e| (e.peer.clone().unwrap(), e.outcome.clone(), e.count)).collect();
        assert!(counts.contains(&("a".into(), "notPermitted".into(), 7)));
        assert!(counts.contains(&("a".into(), "badRequest".into(), 1)));
        assert!(counts.contains(&("b".into(), "notPermitted".into(), 1)));
        let raw = fs::read_to_string(dir.join("log.jsonl")).unwrap();
        assert!(raw.contains("\"count\":7") && raw.lines().count() == 3);
        // Reading again adds nothing, and the minute after starts a new line.
        assert_eq!(log.read().len(), 3);
        let mut later = Entry::new(Some("a"), Some("Studio"), "setTheme", "notPermitted");
        later.time += COALESCE;
        log.append_request(&later).unwrap();
        assert_eq!(log.read().len(), 4);
        // A success is never merged.
        for _ in 0..3 {
            log.append_request(&Entry::new(Some("a"), None, "setTheme", "ok")).unwrap();
        }
        assert_eq!(log.read().iter().filter(|e| e.outcome == "ok").count(), 3);
        fs::remove_dir_all(dir).unwrap();
    }
}
