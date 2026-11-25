//! The notifications of this device: what the bell in the panel lists.
//!
//! The list keeps the last `MAX_ENTRIES`, newest first, in `<config>/notifications.json`. Every
//! change is written before the memory copy changes, so the two never disagree. An entry holds a
//! message key that the webview translates, never text to show as it is.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};

pub const MAX_ENTRIES: usize = 100;
/// The event that tells the main window to read the list again.
pub const CHANGED_EVENT: &str = "notifications-changed";
const MAX_KEY: usize = 64;
const MAX_ROUTE: usize = 64;
const MAX_PARAMS: usize = 8;
const MAX_PARAM_NAME: usize = 32;
const MAX_PARAM_VALUE: usize = 200;
const MAX_ID: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Message,
    Error,
    Update,
    Info,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    /// Seconds since the Unix epoch.
    pub at: u64,
    pub kind: Kind,
    /// The message key of the title. `params.detail`, when present, is the key of the second line.
    pub key: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(default)]
    pub route: Option<String>,
    #[serde(default)]
    pub read: bool,
}

/// A notification before it has an id and a time.
#[derive(Clone, Debug)]
pub struct NewEntry {
    pub kind: Kind,
    pub key: String,
    pub params: BTreeMap<String, String>,
    pub route: Option<String>,
}

/// Whether a notification also sounds and shows a macOS banner, as the preferences allow.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Yes,
    No,
}

/// Whether an entry with the same kind, key and params replaces nothing and is dropped.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Allow,
    Once,
}

pub struct Inbox {
    path: PathBuf,
    entries: Mutex<Vec<Entry>>,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

fn new_id(at: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.subsec_nanos());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{at:x}-{nanos:x}-{count:x}")
}

/// Letters and digits, as a message key is.
fn is_key(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_KEY && text.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// A view name or a place in the panel: lowercase letters, digits, `:` and `-`.
fn is_route(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_ROUTE
        && text
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b':' | b'-'))
}

fn is_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_ID
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

/// Text for people from anywhere: one line, no invisible characters, and bounded.
fn clean_value(text: &str) -> String {
    #[cfg(unix)]
    let text = crate::remote::peers::clean_name(text);
    #[cfg(not(unix))]
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    text.chars().take(MAX_PARAM_VALUE).collect()
}

/// Checks a notification at the boundary and cleans its values.
pub fn validate(entry: NewEntry) -> Result<NewEntry, &'static str> {
    if !is_key(&entry.key)
        || entry.params.len() > MAX_PARAMS
        || entry.route.as_deref().is_some_and(|route| !is_route(route))
    {
        return Err("badRequest");
    }
    let mut params = BTreeMap::new();
    for (name, value) in entry.params {
        let valid_name = !name.is_empty()
            && name.len() <= MAX_PARAM_NAME
            && name.bytes().all(|byte| byte.is_ascii_alphanumeric());
        if !valid_name || value.chars().count() > MAX_PARAM_VALUE {
            return Err("badRequest");
        }
        // The second line is a message key too.
        if name == "detail" && !is_key(&value) {
            return Err("badRequest");
        }
        params.insert(name, clean_value(&value));
    }
    Ok(NewEntry { params, ..entry })
}

/// Checks the ids that a request names.
pub fn validate_ids(ids: &[String]) -> Result<(), &'static str> {
    if ids.len() > MAX_ENTRIES || ids.iter().any(|id| !is_id(id)) {
        return Err("badRequest");
    }
    Ok(())
}

impl Inbox {
    /// Reads the list. A missing or unreadable file starts an empty list.
    pub fn open(path: PathBuf) -> Self {
        let entries = fs::read(&path)
            .ok()
            .and_then(|data| serde_json::from_slice::<Vec<Entry>>(&data).ok())
            .map(|mut entries| {
                entries.truncate(MAX_ENTRIES);
                entries
            })
            .unwrap_or_default();
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    fn write(path: &Path, entries: &[Entry]) -> Result<(), &'static str> {
        let data = serde_json::to_vec_pretty(entries).map_err(|_| "saveFailed")?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, data).map_err(|_| "saveFailed")?;
        fs::rename(temporary, path).map_err(|_| "saveFailed")
    }

    /// Writes the changed list, then keeps it.
    fn change(
        &self,
        edit: impl FnOnce(&mut Vec<Entry>) -> bool,
    ) -> Result<bool, &'static str> {
        let mut entries = self.entries.lock().map_err(|_| "saveFailed")?;
        let mut next = entries.clone();
        if !edit(&mut next) {
            return Ok(false);
        }
        Self::write(&self.path, &next)?;
        *entries = next;
        Ok(true)
    }

    /// The entries, newest first.
    pub fn list(&self) -> Vec<Entry> {
        self.entries.lock().map(|entries| entries.clone()).unwrap_or_default()
    }

    pub fn unread(&self) -> usize {
        self.entries
            .lock()
            .map(|entries| entries.iter().filter(|entry| !entry.read).count())
            .unwrap_or(0)
    }

    /// Adds an entry first in the list and drops the oldest past `MAX_ENTRIES`. With `Repeat::Once`, an
    /// entry like one already listed is not added again.
    pub fn add(&self, new: NewEntry, repeat: Repeat) -> Result<Option<Entry>, &'static str> {
        let at = now();
        let entry = Entry {
            id: new_id(at),
            at,
            kind: new.kind,
            key: new.key,
            params: new.params,
            route: new.route,
            read: false,
        };
        let added = entry.clone();
        let changed = self.change(|entries| {
            let seen = entries.iter().any(|old| {
                old.kind == entry.kind && old.key == entry.key && old.params == entry.params
            });
            if repeat == Repeat::Once && seen {
                return false;
            }
            entries.insert(0, entry);
            entries.truncate(MAX_ENTRIES);
            true
        })?;
        Ok(changed.then_some(added))
    }

    /// Marks the named entries read, or all of them without names.
    pub fn mark_read(&self, ids: Option<&[String]>) -> Result<bool, &'static str> {
        self.change(|entries| {
            let mut changed = false;
            for entry in entries.iter_mut() {
                let named = ids.is_none_or(|ids| ids.contains(&entry.id));
                if named && !entry.read {
                    entry.read = true;
                    changed = true;
                }
            }
            changed
        })
    }

    pub fn clear(&self) -> Result<bool, &'static str> {
        self.change(|entries| {
            let changed = !entries.is_empty();
            entries.clear();
            changed
        })
    }
}

/// Tells the main window and the menu bar that the list changed.
pub fn changed(app: &AppHandle) {
    let _ = app.emit_to("main", CHANGED_EVENT, ());
    #[cfg(target_os = "macos")]
    crate::tray::refresh(app);
}

/// Adds a notification from this app. With `Alert::Yes` it also sounds and shows a macOS banner, as
/// the preferences allow. A notification that cannot be saved is dropped: it is never worth an error.
pub fn record(app: &AppHandle, new: NewEntry, alert: Alert, repeat: Repeat) {
    let Ok(new) = validate(new) else {
        return;
    };
    let Some(inbox) = app.try_state::<Inbox>() else {
        return;
    };
    let Ok(Some(entry)) = inbox.add(new, repeat) else {
        return;
    };
    changed(app);
    #[cfg(target_os = "macos")]
    if alert == Alert::Yes {
        crate::notifications::alert(app, &entry);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (alert, entry);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        let path = std::env::temp_dir().join(format!("lazy-worker-inbox-{}", new_id(now())));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn entry(key: &str) -> NewEntry {
        NewEntry {
            kind: Kind::Info,
            key: key.into(),
            params: BTreeMap::new(),
            route: None,
        }
    }

    #[test]
    fn the_list_keeps_the_newest_first_and_drops_the_oldest() {
        let inbox = Inbox::open(folder().join("notifications.json"));
        for index in 0..MAX_ENTRIES + 5 {
            inbox.add(entry(&format!("key{index}")), Repeat::Allow).unwrap();
        }
        let list = inbox.list();
        assert_eq!(list.len(), MAX_ENTRIES);
        assert_eq!(list[0].key, format!("key{}", MAX_ENTRIES + 4));
        assert_eq!(list[MAX_ENTRIES - 1].key, "key5");
        assert_eq!(inbox.unread(), MAX_ENTRIES);
    }

    #[test]
    fn the_list_survives_a_restart() {
        let path = folder().join("notifications.json");
        let inbox = Inbox::open(path.clone());
        let first = inbox.add(entry("first"), Repeat::Allow).unwrap().unwrap();
        inbox.add(entry("second"), Repeat::Allow).unwrap();
        inbox.mark_read(Some(&[first.id.clone()])).unwrap();
        let again = Inbox::open(path.clone());
        assert_eq!(again.list(), inbox.list());
        assert_eq!(again.unread(), 1);
        again.mark_read(None).unwrap();
        assert_eq!(Inbox::open(path.clone()).unread(), 0);
        again.clear().unwrap();
        assert!(Inbox::open(path).list().is_empty());
    }

    #[test]
    fn a_repeat_once_entry_is_added_only_once() {
        let inbox = Inbox::open(folder().join("notifications.json"));
        let mut update = entry("notifyUpdate");
        update.params.insert("version".into(), "0.7.1".into());
        assert!(inbox.add(update.clone(), Repeat::Once).unwrap().is_some());
        assert!(inbox.add(update.clone(), Repeat::Once).unwrap().is_none());
        update.params.insert("version".into(), "0.7.2".into());
        assert!(inbox.add(update, Repeat::Once).unwrap().is_some());
        assert_eq!(inbox.list().len(), 2);
    }

    #[test]
    fn an_unreadable_file_starts_an_empty_list() {
        let path = folder().join("notifications.json");
        fs::write(&path, b"not json").unwrap();
        assert!(Inbox::open(path).list().is_empty());
    }

    #[test]
    fn validation_rejects_bad_input_and_cleans_values() {
        let mut bad_key = entry("a b");
        assert!(validate(bad_key.clone()).is_err());
        bad_key.key = "x".repeat(MAX_KEY + 1);
        assert!(validate(bad_key).is_err());

        let mut bad_route = entry("ok");
        bad_route.route = Some("Settings/../x".into());
        assert!(validate(bad_route).is_err());

        let mut bad_detail = entry("ok");
        bad_detail.params.insert("detail".into(), "not a key".into());
        assert!(validate(bad_detail).is_err());

        let mut long = entry("ok");
        long.params.insert("name".into(), "x".repeat(MAX_PARAM_VALUE + 1));
        assert!(validate(long).is_err());

        let mut many = entry("ok");
        for index in 0..=MAX_PARAMS {
            many.params.insert(format!("p{index}"), "v".into());
        }
        assert!(validate(many).is_err());

        let mut fine = entry("ok");
        fine.route = Some("mailbox:0123abcd".into());
        fine.params.insert("name".into(), "Mac\u{202e}mini\n".into());
        let cleaned = validate(fine).unwrap();
        assert!(!cleaned.params["name"].contains('\u{202e}'));
        assert!(!cleaned.params["name"].contains('\n'));

        assert!(validate_ids(&["1a-2b-3".into()]).is_ok());
        assert!(validate_ids(&["../x".into()]).is_err());
    }
}
