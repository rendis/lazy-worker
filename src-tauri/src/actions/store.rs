//! The saved actions of this device: named shell commands that only this device can change.
//!
//! The file is read once at start, and a file that is not valid is refused, never replaced.
//! Every change is written before the memory copy changes, so the two never disagree.

use super::schedule::Schedule;
use crate::remote::{check_private_file, write_atomic};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::ErrorKind,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

pub const MAX_NAME: usize = 60;
pub const MAX_COMMAND: usize = 4 * 1024;
pub const MAX_ACTIONS: usize = 64;
pub const DEFAULT_TIMEOUT: u32 = 300;
pub const MAX_TIMEOUT: u32 = 3600;
const MAX_FILE: u64 = 1024 * 1024;

/// Who set a schedule: the person here, or a paired device by its id.
pub const LOCAL: &str = "local";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LastRun {
    /// Unix seconds.
    pub start: u64,
    pub end: u64,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub ok: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Action {
    /// 32 lowercase hexadecimal characters.
    pub id: String,
    pub name: String,
    pub command: String,
    pub timeout_seconds: u32,
    pub schedule: Option<Schedule>,
    pub notify_on_failure: bool,
    /// `local` or the id of a paired device. Set when there is a schedule, and only then.
    pub scheduled_by: Option<String>,
    pub last_run: Option<LastRun>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Data {
    pub actions: Vec<Action>,
    /// The last time the scheduler looked, in unix seconds. A start after a stop uses it to find the runs it missed.
    pub checked_at: Option<u64>,
}

fn is_hex_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// A name for people: no control or invisible characters, trimmed, and at most `MAX_NAME` characters.
pub fn clean_name(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
                )
        })
        .collect();
    kept.trim().chars().take(MAX_NAME).collect::<String>().trim().to_owned()
}

pub fn validate_command(command: &str) -> Result<(), &'static str> {
    if command.trim().is_empty() || command.len() > MAX_COMMAND || command.contains('\0') {
        return Err("actionInvalid");
    }
    Ok(())
}

pub fn validate_timeout(seconds: u32) -> Result<(), &'static str> {
    if (1..=MAX_TIMEOUT).contains(&seconds) {
        Ok(())
    } else {
        Err("actionInvalid")
    }
}

impl Action {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !is_hex_id(&self.id) || self.name.is_empty() || clean_name(&self.name) != self.name {
            return Err("actionsFileInvalid");
        }
        validate_command(&self.command).map_err(|_| "actionsFileInvalid")?;
        validate_timeout(self.timeout_seconds).map_err(|_| "actionsFileInvalid")?;
        if let Some(schedule) = &self.schedule {
            schedule.validate().map_err(|_| "actionsFileInvalid")?;
        }
        match (&self.schedule, &self.scheduled_by) {
            (None, None) => {}
            (Some(_), Some(by)) if by == LOCAL || is_hex_id(by) => {}
            _ => return Err("actionsFileInvalid"),
        }
        if let Some(run) = &self.last_run {
            if run.end < run.start {
                return Err("actionsFileInvalid");
            }
        }
        Ok(())
    }
}

impl Data {
    fn validate(&self) -> Result<(), &'static str> {
        if self.actions.len() > MAX_ACTIONS {
            return Err("actionsFileInvalid");
        }
        let mut seen = HashSet::new();
        for action in &self.actions {
            action.validate()?;
            if !seen.insert(action.id.as_str()) {
                return Err("actionsFileInvalid");
            }
        }
        Ok(())
    }
}

pub struct Store {
    path: PathBuf,
    data: Mutex<Data>,
}

impl Store {
    /// Opens the file. One that is not private and ours, too big, or not valid is refused.
    pub fn open(path: PathBuf) -> Result<Self, &'static str> {
        let data = match fs::symlink_metadata(&path) {
            Ok(meta) => {
                check_private_file(&path).map_err(|_| "actionsFileUnsafe")?;
                if meta.len() > MAX_FILE {
                    return Err("actionsFileInvalid");
                }
                let text = fs::read_to_string(&path).map_err(|_| "actionsIo")?;
                let data: Data = serde_json::from_str(&text).map_err(|_| "actionsFileInvalid")?;
                data.validate()?;
                data
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Data::default(),
            Err(_) => return Err("actionsIo"),
        };
        Ok(Self {
            path,
            data: Mutex::new(data),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn snapshot(&self) -> Data {
        self.lock().clone()
    }

    pub fn get(&self, id: &str) -> Option<Action> {
        self.lock().actions.iter().find(|action| action.id == id).cloned()
    }

    /// Applies a change to a copy, checks it, writes it, and only then keeps it.
    /// The change returns an error code to refuse itself.
    pub fn change<T>(
        &self,
        change: impl FnOnce(&mut Data) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        let mut data = self.lock();
        let mut next = data.clone();
        let result = change(&mut next)?;
        if next == *data {
            return Ok(result);
        }
        next.validate()?;
        let mut bytes = serde_json::to_vec_pretty(&next).map_err(|_| "actionsIo")?;
        bytes.push(b'\n');
        write_atomic(&self.path, &bytes).map_err(|_| "actionsIo")?;
        *data = next;
        Ok(result)
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;

    pub fn action(id: &str, name: &str, command: &str) -> Action {
        Action {
            id: id.into(),
            name: name.into(),
            command: command.into(),
            timeout_seconds: DEFAULT_TIMEOUT,
            schedule: None,
            notify_on_failure: false,
            scheduled_by: None,
            last_run: None,
        }
    }

    pub const ID_A: &str = "0123456789abcdef0123456789abcdef";
    pub const ID_B: &str = "fedcba9876543210fedcba9876543210";
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};
    use crate::remote::testing::scratch;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn write_file(path: &std::path::Path, text: &str, mode: u32) {
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_change_is_written_privately_and_read_back() {
        let dir = scratch("actions-store");
        let path = dir.join("actions.json");
        let store = Store::open(path.clone()).unwrap();
        assert!(store.snapshot().actions.is_empty());
        store
            .change(|data| {
                data.actions.push(action(ID_A, "Backup", "echo hi"));
                Ok(())
            })
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        let again = Store::open(path).unwrap();
        assert_eq!(again.get(ID_A).unwrap().command, "echo hi");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_refused_change_leaves_file_and_memory_alone() {
        let dir = scratch("actions-refused");
        let store = Store::open(dir.join("actions.json")).unwrap();
        let refused = store.change(|data| {
            data.actions.push(action("short", "Backup", "echo hi"));
            Ok(())
        });
        assert_eq!(refused, Err("actionsFileInvalid"));
        assert!(store.snapshot().actions.is_empty());
        assert!(!dir.join("actions.json").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_valid_is_refused_and_left_as_it_is() {
        let dir = scratch("actions-bad");
        let path = dir.join("actions.json");
        let good = r#"{"actions":[{"id":"0123456789abcdef0123456789abcdef","name":"Backup","command":"echo hi","timeoutSeconds":300,"schedule":null,"notifyOnFailure":false,"scheduledBy":null,"lastRun":null}],"checkedAt":null}"#;
        write_file(&path, good, 0o600);
        assert!(Store::open(path.clone()).is_ok());
        let cases = [
            ("garbage", "actionsFileInvalid"),
            (r#"{"actions":[],"extra":1}"#, "actionsFileInvalid"),
            (&good.replace("0123456789abcdef0123456789abcdef", "ABC"), "actionsFileInvalid"),
            (&good.replace("\"timeoutSeconds\":300", "\"timeoutSeconds\":0"), "actionsFileInvalid"),
            (&good.replace("\"timeoutSeconds\":300", "\"timeoutSeconds\":3601"), "actionsFileInvalid"),
            (&good.replace("echo hi", "echo\\u0000hi"), "actionsFileInvalid"),
            (&good.replace("\"name\":\"Backup\"", "\"name\":\" Backup\""), "actionsFileInvalid"),
            // A schedule needs a name for who set it, and no schedule needs none.
            (&good.replace("\"schedule\":null", "\"schedule\":{\"kind\":\"daily\",\"time\":\"02:00\"}"), "actionsFileInvalid"),
            (&good.replace("\"scheduledBy\":null", "\"scheduledBy\":\"local\""), "actionsFileInvalid"),
            (&good.replace("\"schedule\":null,\"notifyOnFailure\":false,\"scheduledBy\":null", "\"schedule\":{\"kind\":\"daily\",\"time\":\"25:00\"},\"notifyOnFailure\":false,\"scheduledBy\":\"local\""), "actionsFileInvalid"),
            (&good.replace("\"schedule\":null,\"notifyOnFailure\":false,\"scheduledBy\":null", "\"schedule\":{\"kind\":\"weekly\",\"day\":7,\"time\":\"02:00\"},\"notifyOnFailure\":false,\"scheduledBy\":\"local\""), "actionsFileInvalid"),
            (&good.replace("\"scheduledBy\":null", "\"scheduledBy\":\"someone\""), "actionsFileInvalid"),
        ];
        for (text, code) in cases {
            write_file(&path, text, 0o600);
            assert_eq!(Store::open(path.clone()).err(), Some(code), "{text}");
            // The file is not replaced.
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
        // Two actions with the same id.
        let one = r#"{"id":"0123456789abcdef0123456789abcdef","name":"A","command":"x","timeoutSeconds":5,"schedule":null,"notifyOnFailure":false,"scheduledBy":null,"lastRun":null}"#;
        write_file(&path, &format!("{{\"actions\":[{one},{one}],\"checkedAt\":null}}"), 0o600);
        assert_eq!(Store::open(path.clone()).err(), Some("actionsFileInvalid"));
        // A file that others can read.
        write_file(&path, good, 0o644);
        assert_eq!(Store::open(path).err(), Some("actionsFileUnsafe"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn names_and_commands_are_cleaned_and_limited() {
        assert_eq!(clean_name("  Back\u{202E}up\n "), "Backup");
        assert_eq!(clean_name(&"é".repeat(100)).chars().count(), MAX_NAME);
        assert!(validate_command("echo hi").is_ok());
        assert!(validate_command("   ").is_err());
        assert!(validate_command("a\0b").is_err());
        assert!(validate_command(&"a".repeat(MAX_COMMAND)).is_ok());
        assert!(validate_command(&"a".repeat(MAX_COMMAND + 1)).is_err());
        assert!(validate_timeout(1).is_ok() && validate_timeout(3600).is_ok());
        assert!(validate_timeout(0).is_err() && validate_timeout(3601).is_err());
    }
}
