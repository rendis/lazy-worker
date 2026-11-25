//! The notifications of this device: what the bell in the panel lists.
//!
//! The bell holds only what is still new. An entry goes when the person opens it, opens the place
//! it points to, or after `MAX_AGE`. `MAX_ENTRIES` is only a safety limit. The list lives in
//! `<config>/notifications.json`, newest first. Every change is written before the memory copy
//! changes, so the two never disagree. An entry holds a message key that the webview translates,
//! never text to show as it is.

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
/// An entry older than this goes, in seconds (7 days).
pub const MAX_AGE: u64 = 7 * 24 * 60 * 60;
/// How many dismissed updates the list remembers, so a version never comes back.
const MAX_TOMBSTONES: usize = 50;
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
}

impl Entry {
    /// What makes two entries the same news.
    fn fingerprint(&self) -> String {
        format!("{:?}:{}:{}", self.kind, self.key, serde_json::to_string(&self.params).unwrap_or_default())
    }
}

/// What the file holds. An older file holds only the list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Store {
    entries: Vec<Entry>,
    /// The fingerprints of updates that the person dismissed, newest first.
    #[serde(default)]
    dismissed: Vec<String>,
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

/// With `Once`, an entry like one listed, or like a dismissed update, is not added again.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Allow,
    Once,
}

pub struct Inbox {
    path: PathBuf,
    store: Mutex<Store>,
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

/// Checks a route that a request names.
pub fn validate_route(route: &str) -> Result<(), &'static str> {
    if is_route(route) { Ok(()) } else { Err("badRequest") }
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
        let store = fs::read(&path)
            .ok()
            .and_then(|data| {
                serde_json::from_slice::<Store>(&data).ok().or_else(|| {
                    serde_json::from_slice::<Vec<Entry>>(&data)
                        .ok()
                        .map(|entries| Store { entries, dismissed: Vec::new() })
                })
            })
            .map(|mut store| {
                store.entries.truncate(MAX_ENTRIES);
                store.dismissed.truncate(MAX_TOMBSTONES);
                store
            })
            .unwrap_or_default();
        Self { path, store: Mutex::new(store) }
    }

    fn write(path: &Path, store: &Store) -> Result<(), &'static str> {
        let data = serde_json::to_vec_pretty(store).map_err(|_| "saveFailed")?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, data).map_err(|_| "saveFailed")?;
        fs::rename(temporary, path).map_err(|_| "saveFailed")
    }

    /// Writes the changed list, then keeps it. Entries past `MAX_AGE` go with any change.
    fn change(&self, now: u64, edit: impl FnOnce(&mut Store) -> bool) -> Result<bool, &'static str> {
        let mut store = self.store.lock().map_err(|_| "saveFailed")?;
        let mut next = store.clone();
        let before = next.entries.len();
        next.entries.retain(|entry| now.saturating_sub(entry.at) < MAX_AGE);
        let expired = next.entries.len() != before;
        if !edit(&mut next) && !expired {
            return Ok(false);
        }
        Self::write(&self.path, &next)?;
        *store = next;
        Ok(true)
    }

    /// The entries that are still new, newest first.
    pub fn list(&self) -> Vec<Entry> {
        self.list_at(now())
    }

    fn list_at(&self, now: u64) -> Vec<Entry> {
        let _ = self.change(now, |_| false);
        self.store.lock().map(|store| store.entries.clone()).unwrap_or_default()
    }

    /// How many entries the bell shows.
    pub fn count(&self) -> usize {
        self.list().len()
    }

    /// Adds an entry first in the list and drops the oldest past `MAX_ENTRIES`.
    pub fn add(&self, new: NewEntry, repeat: Repeat) -> Result<Option<Entry>, &'static str> {
        self.add_at(new, repeat, now())
    }

    fn add_at(&self, new: NewEntry, repeat: Repeat, at: u64) -> Result<Option<Entry>, &'static str> {
        let entry = Entry {
            id: new_id(at),
            at,
            kind: new.kind,
            key: new.key,
            params: new.params,
            route: new.route,
        };
        let added = entry.clone();
        let fingerprint = entry.fingerprint();
        let mut inserted = false;
        self.change(now().max(at), |store| {
            let seen = store.entries.iter().any(|old| old.fingerprint() == fingerprint)
                || store.dismissed.contains(&fingerprint);
            if repeat == Repeat::Once && seen {
                return false;
            }
            store.entries.insert(0, entry);
            store.entries.truncate(MAX_ENTRIES);
            inserted = true;
            true
        })?;
        Ok(inserted.then_some(added))
    }

    /// Removes the entries that match. A dismissed update is remembered, so it never comes back.
    fn dismiss_where(&self, matches: impl Fn(&Entry) -> bool) -> Result<bool, &'static str> {
        self.change(now(), |store| {
            let (gone, kept): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut store.entries).into_iter().partition(&matches);
            store.entries = kept;
            for entry in gone.iter().filter(|entry| entry.kind == Kind::Update) {
                let fingerprint = entry.fingerprint();
                if !store.dismissed.contains(&fingerprint) {
                    store.dismissed.insert(0, fingerprint);
                }
            }
            store.dismissed.truncate(MAX_TOMBSTONES);
            !gone.is_empty()
        })
    }

    /// Removes the named entries.
    pub fn dismiss(&self, ids: &[String]) -> Result<bool, &'static str> {
        self.dismiss_where(|entry| ids.contains(&entry.id))
    }

    /// Removes the entries that point to a place, of one kind or of all kinds.
    pub fn dismiss_route(&self, route: &str, kind: Option<Kind>) -> Result<bool, &'static str> {
        self.dismiss_where(|entry| entry.route.as_deref() == Some(route) && kind.is_none_or(|kind| entry.kind == kind))
    }

    /// Removes every entry. Dismissed updates stay remembered.
    pub fn clear(&self) -> Result<bool, &'static str> {
        self.dismiss_where(|_| true)
    }
}

/// Removes the entries that point to a place the person just opened, and says so when any went.
pub fn dismiss_route(app: &AppHandle, route: &str, kind: Option<Kind>) {
    if let Some(inbox) = app.try_state::<Inbox>() {
        if inbox.dismiss_route(route, kind).unwrap_or(false) {
            changed(app);
        }
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
        NewEntry { kind: Kind::Info, key: key.into(), params: BTreeMap::new(), route: None }
    }

    fn routed(kind: Kind, key: &str, route: &str) -> NewEntry {
        NewEntry { kind, key: key.into(), params: BTreeMap::new(), route: Some(route.into()) }
    }

    fn update(version: &str) -> NewEntry {
        let mut new = routed(Kind::Update, "notifyUpdate", "settings");
        new.params.insert("version".into(), version.into());
        new
    }

    #[test]
    fn the_list_keeps_the_newest_first_and_drops_the_oldest_past_the_limit() {
        let inbox = Inbox::open(folder().join("notifications.json"));
        for index in 0..MAX_ENTRIES + 5 {
            inbox.add(entry(&format!("key{index}")), Repeat::Allow).unwrap();
        }
        let list = inbox.list();
        assert_eq!(list.len(), MAX_ENTRIES);
        assert_eq!(list[0].key, format!("key{}", MAX_ENTRIES + 4));
        assert_eq!(list[MAX_ENTRIES - 1].key, "key5");
        assert_eq!(inbox.count(), MAX_ENTRIES);
    }

    #[test]
    fn the_list_survives_a_restart_and_reads_the_older_file() {
        let path = folder().join("notifications.json");
        let inbox = Inbox::open(path.clone());
        let first = inbox.add(entry("first"), Repeat::Allow).unwrap().unwrap();
        inbox.add(entry("second"), Repeat::Allow).unwrap();
        inbox.dismiss(&[first.id.clone()]).unwrap();
        let again = Inbox::open(path.clone());
        assert_eq!(again.list(), inbox.list());
        assert_eq!(again.count(), 1);
        again.clear().unwrap();
        assert!(Inbox::open(path.clone()).list().is_empty());

        let older = vec![Entry { id: "1-2-3".into(), at: now(), kind: Kind::Info, key: "x".into(), params: BTreeMap::new(), route: None }];
        fs::write(&path, serde_json::to_vec(&older).unwrap()).unwrap();
        assert_eq!(Inbox::open(path).count(), 1);
    }

    #[test]
    fn opening_a_place_removes_what_points_there() {
        let inbox = Inbox::open(folder().join("notifications.json"));
        let mailbox = "mailbox:peer:0123456789abcdef0123456789abcdef";
        inbox.add(routed(Kind::Message, "mailboxText", mailbox), Repeat::Allow).unwrap();
        inbox.add(routed(Kind::Error, "mailboxSendFailed", mailbox), Repeat::Allow).unwrap();
        inbox.add(routed(Kind::Error, "caffeinate", "caffeinate"), Repeat::Allow).unwrap();
        inbox.add(update("0.7.1"), Repeat::Once).unwrap();
        assert!(inbox.dismiss_route(mailbox, None).unwrap());
        assert_eq!(inbox.count(), 2);
        // Settings removes its updates, and a view only its errors.
        assert!(!inbox.dismiss_route("caffeinate", Some(Kind::Update)).unwrap());
        assert!(inbox.dismiss_route("caffeinate", Some(Kind::Error)).unwrap());
        assert!(inbox.dismiss_route("settings", Some(Kind::Update)).unwrap());
        assert_eq!(inbox.count(), 0);
    }

    #[test]
    fn an_error_goes_when_its_problem_clears() {
        let inbox = Inbox::open(folder().join("notifications.json"));
        inbox.add(routed(Kind::Error, "devices", "devices"), Repeat::Allow).unwrap();
        inbox.add(routed(Kind::Info, "devices", "devices"), Repeat::Allow).unwrap();
        inbox.dismiss_route("devices", Some(Kind::Error)).unwrap();
        let left = inbox.list();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].kind, Kind::Info);
    }

    #[test]
    fn an_entry_goes_after_seven_days() {
        let inbox = Inbox::open(folder().join("notifications.json"));
        let start = now();
        inbox.add_at(entry("old"), Repeat::Allow, start - MAX_AGE - 1).unwrap();
        inbox.add_at(entry("recent"), Repeat::Allow, start - MAX_AGE + 60).unwrap();
        let left = inbox.list_at(start);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].key, "recent");
        assert!(inbox.list_at(start + 120).is_empty());
    }

    #[test]
    fn a_dismissed_update_never_comes_back() {
        let path = folder().join("notifications.json");
        let inbox = Inbox::open(path.clone());
        assert!(inbox.add(update("0.7.1"), Repeat::Once).unwrap().is_some());
        assert!(inbox.add(update("0.7.1"), Repeat::Once).unwrap().is_none());
        inbox.dismiss_route("settings", Some(Kind::Update)).unwrap();
        // Also after a restart, and after Clear all.
        let again = Inbox::open(path);
        assert!(again.add(update("0.7.1"), Repeat::Once).unwrap().is_none());
        assert!(again.add(update("0.7.2"), Repeat::Once).unwrap().is_some());
        again.clear().unwrap();
        assert!(again.add(update("0.7.2"), Repeat::Once).unwrap().is_none());
        assert!(again.list().is_empty());
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
        assert!(validate_route("Settings/../x").is_err());
        assert!(validate_route("settings").is_ok());

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
