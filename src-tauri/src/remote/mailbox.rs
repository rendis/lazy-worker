//! The mailbox between this device and another one: texts and files, in both directions.
//!
//! The device that controls (A) reaches the controlled device (B) over its session, and B never
//! connects out. A delivers to B at once. B keeps what it sends to A in an outbox, and A asks for
//! it every minute. A conversation is a `Party`: a device that this one controls, or a peer that
//! controls this one. Each one keeps texts and file metadata only: the app keeps no copy of a file
//! that arrived, and an outbox copy goes once A has it.
//!
//! Text and names from the other device are kept as text. File data moves in chunks through the
//! transfer queue and never reaches the webview.

use super::{ensure_private_dir, files::{CHUNK, MAX_FILE_SIZE}, now, transfers::{Finish, State as JobState}};
use base64::Engine;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

/// The longest text of one message, in bytes.
pub const MAX_TEXT: usize = 8 * 1024;
/// The items that a conversation keeps, and for how long.
pub const KEEP_ITEMS: usize = 200;
pub const KEEP_SECONDS: u64 = 30 * 24 * 60 * 60;
/// The items that may wait in the outbox for one device.
pub const MAX_PENDING: usize = 20;
/// How long an item waits in the outbox before it expires.
pub const OUTBOX_SECONDS: u64 = 7 * 24 * 60 * 60;
/// How often the controller asks a device for its outbox.
pub const POLL_EVERY: Duration = Duration::from_secs(60);
/// The longest file name, in bytes, as macOS allows.
const MAX_NAME: usize = 255;
/// The files or folders of one send.
pub const MAX_PATHS: usize = 20;
/// The files that one device may be sending here at once.
const MAX_INCOMING_PER_PEER: usize = 4;
/// An incoming file that receives nothing for this long ends.
const INCOMING_IDLE: Duration = Duration::from_secs(120);
/// The items that one poll may bring.
const MAX_POLL: usize = 50;

/// The other side of a conversation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Party {
    /// A device that this one controls.
    Device(String),
    /// A paired device that controls this one.
    Peer(String),
}

impl Party {
    pub fn id(&self) -> &str {
        match self {
            Self::Device(id) | Self::Peer(id) => id,
        }
    }

    pub fn valid(&self) -> bool {
        is_id(self.id())
    }

    fn file_name(&self) -> String {
        match self {
            Self::Device(id) => format!("device-{id}.json"),
            Self::Peer(id) => format!("peer-{id}.json"),
        }
    }

    fn from_file_name(name: &str) -> Option<Self> {
        let stem = name.strip_suffix(".json")?;
        let party = if let Some(id) = stem.strip_prefix("device-") {
            Self::Device(id.to_owned())
        } else {
            Self::Peer(stem.strip_prefix("peer-")?.to_owned())
        };
        party.valid().then_some(party)
    }

    /// The place in the panel that a notification of this conversation opens.
    pub fn route(&self) -> String {
        match self {
            Self::Device(id) => format!("mailbox:device:{id}"),
            Self::Peer(id) => format!("mailbox:peer:{id}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// A folder becomes a zip before it goes.
    Compressing,
    /// Waits for the transfer queue.
    Queued,
    Sending,
    Delivered,
    /// In the outbox, until the controller picks it up.
    Waiting,
    /// The controller is reading it from the outbox.
    PickingUp,
    /// A file that is coming in.
    Arriving,
    /// A file that came in and lies in the receive folder.
    Saved,
    /// A text that came in.
    Received,
    Expired,
    Failed,
    Stopped,
}

impl Status {
    fn active(self) -> bool {
        matches!(self, Self::Compressing | Self::Queued | Self::Sending | Self::Arriving | Self::PickingUp)
    }
}

/// One message, kept on disk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    /// The id that the other device gave it, for an item that came from there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    pub at: u64,
    pub mine: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub size: u64,
    /// The file is a zip of a folder.
    #[serde(default)]
    pub folder: bool,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub read: bool,
    /// The file on this device: what was sent, the outbox copy, or where an arrived file was saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// True when this app made `path` (an outbox copy or a zip) and removes it.
    #[serde(default)]
    pub owned: bool,
}

impl Item {
    fn new(mine: bool, status: Status) -> Self {
        Self {
            id: new_id(),
            remote: None,
            at: now(),
            mine,
            text: None,
            name: None,
            size: 0,
            folder: false,
            status,
            error: None,
            read: mine,
            path: None,
            owned: false,
        }
    }
}

/// One message as the webview reads it. No path of this device leaves except the folder it lies in.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    pub id: String,
    pub at: u64,
    pub mine: bool,
    pub text: Option<String>,
    pub name: Option<String>,
    pub size: u64,
    pub folder: bool,
    pub status: Status,
    pub error: Option<String>,
    pub read: bool,
    pub done: u64,
    /// A saved file that is not where it was saved any more.
    pub missing: bool,
    /// The folder that a saved file lies in, with `~` for the home folder.
    pub place: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub items: Vec<ItemView>,
    /// Where files from this device are saved, with `~` for the home folder.
    pub folder: String,
    /// For a device that this one controls: whether it allows the mailbox, as its last clear answer said.
    pub granted: Option<bool>,
    /// For a device that this one controls: the last time this device asked it, and what came back.
    pub check: Option<Check>,
}

/// What the last mailbox check of a device found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// The device answered and allows the mailbox.
    Allowed,
    /// The device answered and refused: its mailbox is off for this device.
    Off,
    /// The device did not answer. `reason` says why, as an error word.
    Unreachable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub at: u64,
    pub outcome: Outcome,
    pub reason: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub party: Party,
    pub unread: usize,
    /// What turning the mailbox off would delete: items that wait for the device or are on their way.
    pub pending: usize,
    pub granted: Option<bool>,
    pub check: Option<Check>,
}

/// What the mailbox needs from the app. A test gives its own.
pub trait Host: Send + Sync + 'static {
    /// Calls a device that this one controls, and returns its answer.
    fn call(&self, device: &str, request: &Value) -> Result<Value, &'static str>;
    fn send_file(&self, device: &str, item: &str, name: &str, local: PathBuf, finish: Finish) -> Option<u64>;
    fn fetch_file(&self, device: &str, item: &str, name: &str, size: u64, part: PathBuf, finish: Finish) -> Option<u64>;
    fn stop_job(&self, job: u64);
    fn job(&self, job: u64) -> Option<(u64, u64, JobState)>;
    /// A conversation changed.
    fn changed(&self, party: &Party);
    /// A text or a whole file arrived.
    fn arrived(&self, party: &Party, item: &Item);
    /// The name of the other side, for the folder of its files.
    fn name_of(&self, party: &Party) -> Option<String>;
    /// Whether this device may write to the other side now: a device it controls, or a peer that has the mailbox.
    fn allowed(&self, party: &Party) -> bool;
    /// Whether a device that this one controls answered lately, when that is known.
    fn online(&self, _party: &Party) -> Option<bool> {
        None
    }
    /// A send to a device failed. The item says why.
    fn failed(&self, _party: &Party, _item: &Item) {}
    /// Runs work off the caller's thread.
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) {
        let _ = std::thread::Builder::new().name("mailbox".into()).spawn(work);
    }
}

/// A file that a device is sending here.
struct Incoming {
    peer: String,
    item: String,
    part: PathBuf,
    file: File,
    size: u64,
    written: u64,
    last: Instant,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    folder: Option<PathBuf>,
}

#[derive(Default)]
struct Inner {
    conversations: HashMap<Party, Vec<Item>>,
    settings: Settings,
    /// Bytes done of an item that moves, by item id.
    done: HashMap<String, u64>,
    /// The transfer job of an item, by item id.
    jobs: HashMap<String, u64>,
    /// Items whose folder is being compressed and that were stopped meanwhile.
    cancelled: HashSet<String>,
    incoming: HashMap<String, Incoming>,
    granted: HashMap<String, bool>,
    /// The last mailbox check of each device that this one controls.
    checks: HashMap<String, Check>,
}

pub struct Mailbox {
    dir: PathBuf,
    home: Option<PathBuf>,
    downloads: Option<PathBuf>,
    host: Box<dyn Host>,
    inner: Mutex<Inner>,
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 32 lowercase hexadecimal characters.
pub fn is_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The text of a message: not empty, at most `MAX_TEXT` bytes, and with no control or invisible
/// characters other than line breaks and tabs.
pub fn clean_text(text: &str) -> Result<String, &'static str> {
    if text.len() > MAX_TEXT {
        return Err("mailboxTextTooLong");
    }
    let kept: String = text
        .chars()
        .filter(|c| matches!(c, '\n' | '\t') || (!c.is_control() && !super::peers::is_format(*c)))
        .collect();
    let kept = kept.trim().to_owned();
    if kept.is_empty() {
        return Err("mailboxTextEmpty");
    }
    Ok(kept)
}

/// A file name from anywhere: one name, with no control or invisible characters, at most 255
/// bytes, and never `.` or `..`. Nothing usable becomes `file`.
pub fn clean_file_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| !c.is_control() && !super::peers::is_format(*c))
        .map(|c| if matches!(c, '/' | ':') { '-' } else { c })
        .collect();
    let kept = kept.trim();
    let mut out = String::new();
    for c in kept.chars() {
        if out.len() + c.len_utf8() > MAX_NAME {
            break;
        }
        out.push(c);
    }
    let out = out.trim().to_owned();
    if out.is_empty() || out == "." || out == ".." {
        "file".to_owned()
    } else {
        out
    }
}

/// The reason that a failed send shows: the mailbox is off there, the device is away, or the code itself.
fn failure_reason(code: &'static str) -> &'static str {
    match code {
        "notPermitted" => "mailboxOffThere",
        "sshUnreachable" | "sshDenied" | "sshTimeout" | "sshFailed" | "sshHostKeyChanged" | "sshHostKeyUnknown"
        | "appNotRunning" | "remoteUnavailable" | "notPaired" | "busy" | "remoteFailed" | "remoteTimeout" => "mailboxOffline",
        code => code,
    }
}

/// A device name as a folder name.
fn folder_name(name: &str) -> String {
    let name = super::peers::clean_name(name).replace(['/', ':'], "-");
    let name = name.trim_start_matches('.').trim();
    if name.is_empty() { "Device".to_owned() } else { name.to_owned() }
}

/// A name that is free in a folder: `name`, then `name 2`, `name 3`, and so on before the extension.
pub fn free_name(folder: &Path, name: &str) -> PathBuf {
    super::transfers::unique(folder, name)
}

/// Marks a file that came from another device, so macOS checks it when it opens.
fn quarantine(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return };
        let value = format!("0081;{:08x};Lazy Worker;", now());
        let name = c"com.apple.quarantine";
        // SAFETY: both strings end with NUL and live for the call, and the value is `len` bytes.
        unsafe {
            libc::setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

/// A copy that shares the blocks of the original on APFS, or a plain copy elsewhere.
fn snapshot(from: &Path, to: &Path) -> Result<(), &'static str> {
    let cloned = Command::new("/bin/cp")
        .arg("-c")
        .arg(from)
        .arg(to)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if cloned {
        return Ok(());
    }
    let _ = fs::remove_file(to);
    fs::copy(from, to).map(|_| ()).map_err(|_| "mailboxCopyFailed")
}

/// Compresses a folder into `zip`, keeping the folder itself, as Finder does.
fn compress(folder: &Path, zip: &Path) -> Result<(), &'static str> {
    let done = Command::new("/usr/bin/ditto")
        .args(["-c", "-k", "--keepParent"])
        .arg(folder)
        .arg(zip)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if done { Ok(()) } else { Err("mailboxCompressFailed") }
}

/// Moves a file into place, also across volumes.
fn move_file(from: &Path, to: &Path) -> Result<(), &'static str> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to).map_err(|_| "filesDiskFull")?;
    let _ = fs::remove_file(from);
    Ok(())
}

impl Mailbox {
    /// Opens the mailbox in `dir` and reads every conversation. Work that a restart cut short ends as failed.
    pub fn open(dir: PathBuf, home: Option<PathBuf>, downloads: Option<PathBuf>, host: impl Host) -> Result<Arc<Self>, &'static str> {
        ensure_private_dir(&dir)?;
        let mut inner = Inner::default();
        if let Ok(data) = fs::read(dir.join("settings.json")) {
            inner.settings = serde_json::from_slice(&data).unwrap_or_default();
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(party) = name.to_str().and_then(Party::from_file_name) else { continue };
                let Ok(data) = fs::read(entry.path()) else { continue };
                let Ok(mut items) = serde_json::from_slice::<Vec<Item>>(&data) else { continue };
                for item in &mut items {
                    match item.status {
                        Status::PickingUp => item.status = Status::Waiting,
                        status if status.active() => {
                            item.status = Status::Failed;
                            item.error = Some("mailboxInterrupted".into());
                        }
                        _ => {}
                    }
                }
                inner.conversations.insert(party, items);
            }
        }
        // Partial files of a run that ended go.
        let _ = fs::remove_dir_all(dir.join("incoming"));
        let _ = fs::remove_dir_all(dir.join("zips"));
        Ok(Arc::new(Self { dir, home, downloads, host: Box::new(host), inner: Mutex::new(inner) }))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn save(&self, inner: &Inner, party: &Party) {
        let path = self.dir.join(party.file_name());
        let Some(items) = inner.conversations.get(party) else {
            let _ = fs::remove_file(path);
            return;
        };
        let Ok(data) = serde_json::to_vec(items) else { return };
        let temp = self.dir.join(format!(".{}.tmp", party.file_name()));
        let written = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)
            .and_then(|mut file| file.write_all(&data).and_then(|()| file.sync_all()));
        if written.is_ok() {
            let _ = fs::rename(&temp, &path);
        } else {
            let _ = fs::remove_file(&temp);
        }
    }

    /// Keeps the last `KEEP_ITEMS` items of the last `KEEP_SECONDS`. Work in progress and the outbox stay.
    fn trim(items: &mut Vec<Item>, at: u64) {
        let keep = |item: &Item| item.status.active() || item.status == Status::Waiting;
        items.retain(|item| keep(item) || at.saturating_sub(item.at) <= KEEP_SECONDS);
        while items.len() > KEEP_ITEMS {
            let Some(index) = items.iter().position(|item| !keep(item)) else { break };
            let gone = items.remove(index);
            Self::drop_owned(&gone);
        }
    }

    fn drop_owned(item: &Item) {
        if item.owned {
            if let Some(path) = &item.path {
                let _ = fs::remove_file(path);
                if let Some(parent) = path.parent() {
                    let _ = fs::remove_dir(parent);
                }
            }
        }
    }

    /// Changes an item and saves the conversation. Returns the item as it is now.
    fn change(&self, party: &Party, id: &str, edit: impl FnOnce(&mut Item)) -> Option<Item> {
        let mut inner = self.lock();
        let item = inner.conversations.get_mut(party)?.iter_mut().find(|item| item.id == id)?;
        edit(item);
        let item = item.clone();
        self.save(&inner, party);
        drop(inner);
        self.host.changed(party);
        Some(item)
    }

    fn add(&self, party: &Party, item: Item) {
        let mut inner = self.lock();
        let items = inner.conversations.entry(party.clone()).or_default();
        items.push(item);
        Self::trim(items, now());
        self.save(&inner, party);
        drop(inner);
        self.host.changed(party);
    }

    /// Where files from a device are saved.
    fn receive_folder(&self, inner: &Inner, party: &Party) -> Option<PathBuf> {
        if let Some(folder) = &inner.settings.folder {
            return Some(folder.clone());
        }
        let name = self.host.name_of(party).unwrap_or_default();
        Some(self.downloads.as_ref()?.join("Lazy Worker").join(folder_name(&name)))
    }

    fn shown(&self, path: &Path) -> String {
        match &self.home {
            Some(home) => match path.strip_prefix(home) {
                Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => path.display().to_string(),
            },
            None => path.display().to_string(),
        }
    }

    // ----- What the webview asks -----

    pub fn conversation(&self, party: &Party) -> Conversation {
        let inner = self.lock();
        let items = inner.conversations.get(party).map(Vec::as_slice).unwrap_or_default();
        let views = items
            .iter()
            .map(|item| {
                let job = inner.jobs.get(&item.id).and_then(|job| self.host.job(*job));
                let done = job.map(|(done, _, _)| done).or_else(|| inner.done.get(&item.id).copied()).unwrap_or(0);
                // A send that waits for the queue says so.
                let status = match (item.status, job) {
                    (Status::Sending, Some((_, _, JobState::Queued))) => Status::Queued,
                    (status, _) => status,
                };
                let saved = item.status == Status::Saved;
                let missing = saved && item.path.as_ref().is_none_or(|path| fs::symlink_metadata(path).is_err());
                let place = if saved { item.path.as_ref().and_then(|path| path.parent()).map(|parent| self.shown(parent)) } else { None };
                ItemView {
                    id: item.id.clone(),
                    at: item.at,
                    mine: item.mine,
                    text: item.text.clone(),
                    name: item.name.clone(),
                    size: item.size,
                    folder: item.folder,
                    status,
                    error: item.error.clone(),
                    read: item.read,
                    done,
                    missing,
                    place,
                }
            })
            .collect();
        let folder = self.receive_folder(&inner, party).map(|folder| self.shown(&folder)).unwrap_or_default();
        let check = match party {
            Party::Device(id) => inner.checks.get(id).copied(),
            Party::Peer(_) => None,
        };
        let granted = match party {
            Party::Device(id) => inner.granted.get(id).copied(),
            Party::Peer(_) => None,
        };
        Conversation { items: views, folder, granted, check }
    }

    pub fn summary(&self) -> Vec<Summary> {
        let inner = self.lock();
        let mut parties: Vec<Party> = inner.conversations.keys().cloned().collect();
        for id in inner.granted.keys().chain(inner.checks.keys()) {
            let party = Party::Device(id.clone());
            if !parties.contains(&party) {
                parties.push(party);
            }
        }
        parties
            .into_iter()
            .map(|party| Summary {
                unread: inner.conversations.get(&party).map_or(0, |items| items.iter().filter(|item| !item.read).count()),
                pending: inner.conversations.get(&party).map_or(0, |items| {
                    items.iter().filter(|item| item.status == Status::Waiting || item.status.active()).count()
                }),
                granted: match &party {
                    Party::Device(id) => inner.granted.get(id).copied(),
                    Party::Peer(_) => None,
                },
                check: match &party {
                    Party::Device(id) => inner.checks.get(id).copied(),
                    Party::Peer(_) => None,
                },
                party,
            })
            .collect()
    }

    pub fn mark_read(&self, party: &Party) {
        let mut inner = self.lock();
        let Some(items) = inner.conversations.get_mut(party) else { return };
        let mut changed = false;
        for item in items.iter_mut().filter(|item| !item.read) {
            item.read = true;
            changed = true;
        }
        if changed {
            self.save(&inner, party);
            drop(inner);
            self.host.changed(party);
        }
    }

    pub fn set_folder(&self, folder: Option<String>) -> Result<(), &'static str> {
        let folder = match folder {
            Some(text) => {
                let path = PathBuf::from(&text);
                if !path.is_absolute() || text.contains('\0') || !path.is_dir() {
                    return Err("filesNotFolder");
                }
                Some(path)
            }
            None => None,
        };
        let mut inner = self.lock();
        inner.settings.folder = folder;
        let data = serde_json::to_vec(&inner.settings).map_err(|_| "saveFailed")?;
        let path = self.dir.join("settings.json");
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut file| file.write_all(&data))
            .map_err(|_| "saveFailed")?;
        let parties: Vec<Party> = inner.conversations.keys().cloned().collect();
        drop(inner);
        for party in &parties {
            self.host.changed(party);
        }
        Ok(())
    }

    /// The file of an item, for open and reveal: a saved file that is still there.
    pub fn file_of(&self, party: &Party, id: &str) -> Result<PathBuf, &'static str> {
        let inner = self.lock();
        let item = inner.conversations.get(party).and_then(|items| items.iter().find(|item| item.id == id)).ok_or("mailboxUnknown")?;
        let path = item.path.clone().filter(|_| item.status == Status::Saved).ok_or("mailboxUnknown")?;
        if !path.is_absolute() || fs::symlink_metadata(&path).is_err() {
            return Err("mailboxMissing");
        }
        Ok(path)
    }

    /// Why a send to a device would fail now, from what this device last heard: the mailbox is off there, or the device is away.
    fn blocked(&self, party: &Party) -> Option<&'static str> {
        let Party::Device(device) = party else { return None };
        if self.lock().granted.get(device) == Some(&false) {
            return Some("notPermitted");
        }
        (self.host.online(party) == Some(false)).then_some("sshUnreachable")
    }

    /// Notes what a device said about its mailbox, and tells the window when it changed.
    fn set_granted(&self, device: &str, granted: bool) {
        self.note_check(device, if granted { Outcome::Allowed } else { Outcome::Off }, None);
    }

    /// Keeps the last check of a device. Only a clear answer (allowed or off) changes whether it allows the
    /// mailbox: a device that did not answer keeps what it said last, and the check says it was not reached.
    fn note_check(&self, device: &str, outcome: Outcome, reason: Option<&'static str>) {
        {
            let mut inner = self.lock();
            match outcome {
                Outcome::Allowed => {
                    inner.granted.insert(device.to_owned(), true);
                }
                Outcome::Off => {
                    inner.granted.insert(device.to_owned(), false);
                }
                Outcome::Unreachable => {}
            }
            inner.checks.insert(device.to_owned(), Check { at: now(), outcome, reason });
        }
        // The time of the check moves on too, so the window and the row read it again.
        self.host.changed(&Party::Device(device.to_owned()));
    }

    /// A send to a device ended badly. A refusal means the mailbox is off there; a link error means the device is away.
    fn failed(&self, party: &Party, id: &str, code: &'static str) {
        let reason = failure_reason(code);
        if let Party::Device(device) = party {
            match reason {
                "mailboxOffThere" => self.set_granted(device, false),
                "mailboxOffline" => self.note_check(device, Outcome::Unreachable, Some(code)),
                _ => {}
            }
        }
        if let Some(item) = self.change(party, id, |item| {
            item.status = Status::Failed;
            item.error = Some(reason.to_owned());
        }) {
            self.host.failed(party, &item);
        }
    }

    pub fn send_text(self: &Arc<Self>, party: &Party, text: &str) -> Result<(), &'static str> {
        if !self.host.allowed(party) {
            return Err("notPermitted");
        }
        let text = clean_text(text)?;
        match party {
            Party::Device(device) => {
                let mut item = Item::new(true, Status::Sending);
                item.text = Some(text);
                let id = item.id.clone();
                self.add(party, item);
                // What this device knows already decides at once, so the item never looks pending for nothing.
                match self.blocked(party) {
                    Some(code) => self.failed(party, &id, code),
                    None => self.deliver_text(device.clone(), id),
                }
            }
            Party::Peer(_) => {
                self.check_room(party)?;
                let mut item = Item::new(true, Status::Waiting);
                item.text = Some(text);
                self.add(party, item);
            }
        }
        Ok(())
    }

    fn deliver_text(self: &Arc<Self>, device: String, id: String) {
        let this = self.clone();
        self.host.spawn(Box::new(move || {
            let party = Party::Device(device.clone());
            let text = this.lock().conversations.get(&party).and_then(|items| items.iter().find(|item| item.id == id)).and_then(|item| item.text.clone());
            let Some(text) = text else { return };
            let outcome = this.host.call(&device, &json!({ "command": "mailboxPut", "args": { "id": id, "text": text } }));
            match outcome {
                Ok(_) | Err("mailboxDuplicate") => {
                    this.set_granted(&device, true);
                    this.change(&party, &id, |item| {
                        item.status = Status::Delivered;
                        item.error = None;
                    });
                }
                Err(code) => this.failed(&party, &id, code),
            }
        }));
    }

    fn check_room(&self, party: &Party) -> Result<(), &'static str> {
        let inner = self.lock();
        let pending = inner
            .conversations
            .get(party)
            .map_or(0, |items| items.iter().filter(|item| item.mine && (item.status == Status::Waiting || item.status.active())).count());
        if pending >= MAX_PENDING { Err("mailboxFull") } else { Ok(()) }
    }

    /// Sends files and folders that the person chose here. A folder is compressed first.
    pub fn send_paths(self: &Arc<Self>, party: &Party, paths: &[String]) -> Result<(), &'static str> {
        if !self.host.allowed(party) {
            return Err("notPermitted");
        }
        if paths.is_empty() || paths.len() > MAX_PATHS {
            return Err("filesTooMany");
        }
        let mut chosen = Vec::new();
        for text in paths {
            let path = PathBuf::from(text);
            if !path.is_absolute() || text.contains('\0') {
                return Err("filesPathInvalid");
            }
            let meta = fs::metadata(&path).map_err(|_| "filesNotFound")?;
            if meta.is_file() && meta.len() > MAX_FILE_SIZE {
                return Err("filesTooLarge");
            }
            if !meta.is_file() && !meta.is_dir() {
                return Err("filesNotRegular");
            }
            chosen.push((path, meta));
        }
        if let Party::Peer(_) = party {
            let inner = self.lock();
            let pending = inner
                .conversations
                .get(party)
                .map_or(0, |items| items.iter().filter(|item| item.mine && (item.status == Status::Waiting || item.status.active())).count());
            if pending + chosen.len() > MAX_PENDING {
                return Err("mailboxFull");
            }
        }
        let blocked = self.blocked(party);
        for (path, meta) in chosen {
            let base = path.file_name().map(|name| clean_file_name(&name.to_string_lossy())).unwrap_or_else(|| "file".into());
            let mut item = Item::new(true, Status::Queued);
            if let Some(code) = blocked {
                // Nothing goes, and nothing is compressed for nothing. Try again starts from the chosen file or folder.
                item.folder = meta.is_dir();
                item.name = Some(if meta.is_dir() { clean_file_name(&format!("{base}.zip")) } else { base });
                item.size = if meta.is_dir() { 0 } else { meta.len() };
                item.path = Some(path);
                let id = item.id.clone();
                self.add(party, item);
                self.failed(party, &id, code);
                continue;
            }
            if meta.is_dir() {
                item.folder = true;
                item.status = Status::Compressing;
                item.name = Some(clean_file_name(&format!("{base}.zip")));
                let id = item.id.clone();
                self.add(party, item);
                self.compress_then(party.clone(), id, path);
            } else {
                item.name = Some(base);
                item.size = meta.len();
                let id = item.id.clone();
                self.add(party, item);
                self.ready(party.clone(), id, path, false);
            }
        }
        Ok(())
    }

    fn compress_then(self: &Arc<Self>, party: Party, id: String, folder: PathBuf) {
        let this = self.clone();
        self.host.spawn(Box::new(move || {
            let outcome = (|| {
                let dir = this.dir.join("zips").join(&id);
                ensure_private_dir(&this.dir.join("zips"))?;
                ensure_private_dir(&dir)?;
                let name = this.lock().conversations.get(&party).and_then(|items| items.iter().find(|item| item.id == id)).and_then(|item| item.name.clone()).unwrap_or_else(|| "folder.zip".into());
                let zip = dir.join(&name);
                compress(&folder, &zip)?;
                let size = fs::metadata(&zip).map_err(|_| "mailboxCompressFailed")?.len();
                if size > MAX_FILE_SIZE {
                    let _ = fs::remove_dir_all(&dir);
                    return Err("filesTooLarge");
                }
                Ok((zip, size))
            })();
            let stopped = this.lock().cancelled.remove(&id);
            match outcome {
                Ok((zip, _)) if stopped => {
                    if let Some(parent) = zip.parent() {
                        let _ = fs::remove_dir_all(parent);
                    }
                }
                Ok((zip, size)) => {
                    this.change(&party, &id, |item| item.size = size);
                    this.ready(party, id, zip, true);
                }
                Err(code) => {
                    this.change(&party, &id, |item| {
                        item.status = Status::Failed;
                        item.error = Some(code.to_owned());
                    });
                }
            }
        }));
    }

    /// A file is ready to go: to the transfer queue for a device, or into the outbox for a peer.
    fn ready(self: &Arc<Self>, party: Party, id: String, path: PathBuf, owned: bool) {
        match &party {
            Party::Device(device) => {
                // The status is set before the job is queued, so a job that ends at once is not overwritten.
                self.change(&party, &id, |item| {
                    item.path = Some(path.clone());
                    item.owned = owned;
                    item.status = Status::Sending;
                    item.error = None;
                });
                let name = self.lock().conversations.get(&party).and_then(|items| items.iter().find(|item| item.id == id)).and_then(|item| item.name.clone()).unwrap_or_default();
                let this = self.clone();
                let (finish_party, finish_id) = (party.clone(), id.clone());
                let finish: Finish = Box::new(move |outcome| this.sent(&finish_party, &finish_id, outcome));
                match self.host.send_file(device, &id, &name, path, finish) {
                    Some(job) => {
                        let mut inner = self.lock();
                        // A job that ended already left nothing to follow.
                        let moving = inner.conversations.get(&party).and_then(|items| items.iter().find(|item| item.id == id)).is_some_and(|item| item.status == Status::Sending);
                        if moving {
                            inner.jobs.insert(id.clone(), job);
                        }
                    }
                    None => {
                        self.change(&party, &id, |item| {
                            item.status = Status::Failed;
                            item.error = Some("workerUnavailable".into());
                        });
                    }
                }
            }
            Party::Peer(peer) => {
                let outcome = (|| -> Result<(PathBuf, u64), &'static str> {
                    let outbox = self.dir.join("outbox");
                    ensure_private_dir(&outbox)?;
                    ensure_private_dir(&outbox.join(peer))?;
                    let folder = outbox.join(peer).join(&id);
                    ensure_private_dir(&folder)?;
                    let name = path.file_name().map(|name| clean_file_name(&name.to_string_lossy())).unwrap_or_else(|| "file".into());
                    let copy = folder.join(name);
                    if owned {
                        move_file(&path, &copy)?;
                        if let Some(parent) = path.parent() {
                            let _ = fs::remove_dir(parent);
                        }
                    } else {
                        snapshot(&path, &copy)?;
                    }
                    let _ = fs::set_permissions(&copy, fs::Permissions::from_mode(0o600));
                    let size = fs::metadata(&copy).map_err(|_| "mailboxCopyFailed")?.len();
                    Ok((copy, size))
                })();
                self.change(&party, &id, |item| match outcome {
                    Ok((copy, size)) => {
                        item.path = Some(copy);
                        item.owned = true;
                        item.size = size;
                        item.status = Status::Waiting;
                        item.at = now();
                    }
                    Err(code) => {
                        item.status = Status::Failed;
                        item.error = Some(code.to_owned());
                    }
                });
            }
        }
    }

    /// A send to a device ended.
    fn sent(self: &Arc<Self>, party: &Party, id: &str, outcome: Result<(), &'static str>) {
        self.lock().jobs.remove(id);
        let delivered = matches!(outcome, Ok(()) | Err("mailboxDuplicate"));
        // A zip that went is not needed here any more. It goes before the item says delivered.
        if delivered {
            let owned = self.lock().conversations.get(party).and_then(|items| items.iter().find(|item| item.id == id)).filter(|item| item.owned).cloned();
            if let Some(item) = owned {
                Self::drop_owned(&item);
            }
        }
        match outcome {
            Ok(()) | Err("mailboxDuplicate") => {
                if let Party::Device(device) = party {
                    self.set_granted(device, true);
                }
                self.change(party, id, |item| {
                    item.status = Status::Delivered;
                    item.error = None;
                    if item.owned {
                        item.path = None;
                        item.owned = false;
                    }
                });
            }
            Err("stopped") => {
                self.change(party, id, |item| item.status = Status::Stopped);
            }
            Err(code) => self.failed(party, id, code),
        }
    }

    /// Stops an item that moves, or takes one back from the outbox.
    pub fn cancel(&self, party: &Party, id: &str) -> Result<(), &'static str> {
        let (status, job) = {
            let inner = self.lock();
            let item = inner.conversations.get(party).and_then(|items| items.iter().find(|item| item.id == id)).ok_or("mailboxUnknown")?;
            (item.status, inner.jobs.get(id).copied())
        };
        match status {
            Status::Compressing => {
                self.lock().cancelled.insert(id.to_owned());
                self.change(party, id, |item| item.status = Status::Stopped);
            }
            Status::Waiting | Status::PickingUp => {
                let item = self.change(party, id, |item| item.status = Status::Stopped);
                if let Some(item) = item {
                    Self::drop_owned(&item);
                }
            }
            Status::Arriving if matches!(party, Party::Peer(_)) => {
                let taken: Vec<Incoming> = {
                    let mut inner = self.lock();
                    let ids: Vec<String> = inner.incoming.iter().filter(|(_, up)| up.item == id).map(|(key, _)| key.clone()).collect();
                    ids.iter().filter_map(|key| inner.incoming.remove(key)).collect()
                };
                for upload in taken {
                    let _ = fs::remove_file(&upload.part);
                }
                self.change(party, id, |item| item.status = Status::Stopped);
            }
            _ => match job {
                Some(job) => self.host.stop_job(job),
                None if status.active() => {
                    self.change(party, id, |item| item.status = Status::Stopped);
                }
                None => {}
            },
        }
        Ok(())
    }

    /// Tries an item again: a text or file of mine that did not go, or a file that did not arrive.
    pub fn retry(self: &Arc<Self>, party: &Party, id: &str) -> Result<(), &'static str> {
        let item = {
            let inner = self.lock();
            inner.conversations.get(party).and_then(|items| items.iter().find(|item| item.id == id)).cloned().ok_or("mailboxUnknown")?
        };
        if !matches!(item.status, Status::Failed | Status::Stopped) {
            return Err("mailboxBusy");
        }
        let Party::Device(device) = party else {
            return Err("mailboxBusy");
        };
        if item.mine {
            if item.text.is_some() {
                self.change(party, id, |item| {
                    item.status = Status::Sending;
                    item.error = None;
                });
                self.deliver_text(device.clone(), id.to_owned());
                return Ok(());
            }
            let path = item.path.clone().filter(|path| path.exists()).ok_or("filesNotFound")?;
            // A folder that never went was never compressed.
            if path.is_dir() {
                self.change(party, id, |item| {
                    item.status = Status::Compressing;
                    item.error = None;
                });
                self.compress_then(party.clone(), id.to_owned(), path);
            } else {
                self.ready(party.clone(), id.to_owned(), path, item.owned);
            }
            Ok(())
        } else {
            let remote = item.remote.clone().ok_or("mailboxUnknown")?;
            self.fetch(device, &item.id, &remote, item.name.as_deref().unwrap_or("file"), item.size)
        }
    }

    pub fn remove(&self, party: &Party, id: &str) -> Result<(), &'static str> {
        let _ = self.cancel(party, id);
        let mut inner = self.lock();
        let items = inner.conversations.get_mut(party).ok_or("mailboxUnknown")?;
        let index = items.iter().position(|item| item.id == id).ok_or("mailboxUnknown")?;
        let gone = items.remove(index);
        Self::drop_owned(&gone);
        self.save(&inner, party);
        drop(inner);
        self.host.changed(party);
        Ok(())
    }

    /// Empties a conversation. Work in progress stops, and the outbox copies go.
    pub fn clear(&self, party: &Party) {
        let ids: Vec<String> = self.lock().conversations.get(party).map(|items| items.iter().map(|item| item.id.clone()).collect()).unwrap_or_default();
        for id in &ids {
            let _ = self.cancel(party, id);
        }
        let mut inner = self.lock();
        if let Some(items) = inner.conversations.remove(party) {
            for item in &items {
                Self::drop_owned(item);
            }
        }
        self.save(&inner, party);
        drop(inner);
        self.host.changed(party);
    }

    /// A peer lost the mailbox: what waits for it, and what it is sending here, goes.
    pub fn revoke_peer(&self, peer: &str) {
        let party = Party::Peer(peer.to_owned());
        let taken: Vec<Incoming> = {
            let mut inner = self.lock();
            let ids: Vec<String> = inner.incoming.iter().filter(|(_, up)| up.peer == peer).map(|(key, _)| key.clone()).collect();
            ids.iter().filter_map(|key| inner.incoming.remove(key)).collect()
        };
        for upload in &taken {
            let _ = fs::remove_file(&upload.part);
        }
        let mut inner = self.lock();
        if let Some(items) = inner.conversations.get_mut(&party) {
            items.retain(|item| {
                let pending = item.mine && (item.status == Status::Waiting || item.status == Status::PickingUp || item.status.active());
                if pending {
                    Self::drop_owned(item);
                }
                !pending
            });
            for item in items.iter_mut().filter(|item| item.status == Status::Arriving) {
                item.status = Status::Failed;
                item.error = Some("notPermitted".into());
            }
        }
        self.save(&inner, &party);
        drop(inner);
        let _ = fs::remove_dir_all(self.dir.join("outbox").join(peer));
        self.host.changed(&party);
    }

    /// A conversation ends with its device: everything of it goes.
    pub fn forget(&self, party: &Party) {
        if let Party::Peer(peer) = party {
            self.revoke_peer(peer);
        }
        let jobs: Vec<u64> = {
            let inner = self.lock();
            inner.conversations.get(party).map(|items| items.iter().filter_map(|item| inner.jobs.get(&item.id).copied()).collect()).unwrap_or_default()
        };
        for job in jobs {
            self.host.stop_job(job);
        }
        self.clear(party);
        if let Party::Device(id) = party {
            let mut inner = self.lock();
            inner.granted.remove(id);
            inner.checks.remove(id);
        }
    }

    // ----- What a device that controls this one asks -----

    /// A text from a peer.
    pub fn put(&self, peer: &str, id: &str, text: &str) -> Result<Value, &'static str> {
        if !is_id(id) {
            return Err("badRequest");
        }
        let text = clean_text(text)?;
        let party = Party::Peer(peer.to_owned());
        if self.known(&party, id) {
            return Err("mailboxDuplicate");
        }
        let mut item = Item::new(false, Status::Received);
        item.remote = Some(id.to_owned());
        item.text = Some(text);
        self.add(&party, item.clone());
        self.host.arrived(&party, &item);
        Ok(Value::Null)
    }

    /// True when an item from the other side with this id is here already.
    fn known(&self, party: &Party, remote: &str) -> bool {
        self.lock().conversations.get(party).is_some_and(|items| {
            items.iter().any(|item| item.remote.as_deref() == Some(remote) && matches!(item.status, Status::Saved | Status::Received))
        })
    }

    /// A peer begins to send a file.
    pub fn begin(&self, peer: &str, id: &str, name: &str, size: u64) -> Result<Value, &'static str> {
        if !is_id(id) || name.len() > 4 * MAX_NAME {
            return Err("badRequest");
        }
        if size > MAX_FILE_SIZE {
            return Err("filesTooLarge");
        }
        let party = Party::Peer(peer.to_owned());
        if self.known(&party, id) {
            return Err("mailboxDuplicate");
        }
        let incoming = self.dir.join("incoming");
        ensure_private_dir(&incoming)?;
        let upload = new_id();
        let part = incoming.join(format!("{upload}.part"));
        {
            let inner = self.lock();
            if inner.incoming.values().filter(|up| up.peer == peer).count() >= MAX_INCOMING_PER_PEER {
                return Err("mailboxBusy");
            }
        }
        let file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&part).map_err(|_| "filesDenied")?;
        // A send that broke and begins again replaces its old partial file.
        let stale: Vec<Incoming> = {
            let mut inner = self.lock();
            let items = inner.conversations.get(&party).map(|items| items.iter().filter(|item| item.remote.as_deref() == Some(id)).map(|item| item.id.clone()).collect::<Vec<_>>()).unwrap_or_default();
            let keys: Vec<String> = inner.incoming.iter().filter(|(_, up)| up.peer == peer && items.contains(&up.item)).map(|(key, _)| key.clone()).collect();
            keys.iter().filter_map(|key| inner.incoming.remove(key)).collect()
        };
        for upload in stale {
            let _ = fs::remove_file(&upload.part);
        }
        // An item that failed before comes back as the same item.
        let existing = self.lock().conversations.get(&party).and_then(|items| items.iter().find(|item| item.remote.as_deref() == Some(id)).map(|item| item.id.clone()));
        let item_id = match existing {
            Some(item_id) => {
                self.change(&party, &item_id, |item| {
                    item.status = Status::Arriving;
                    item.error = None;
                    item.size = size;
                });
                item_id
            }
            None => {
                let mut item = Item::new(false, Status::Arriving);
                item.remote = Some(id.to_owned());
                item.name = Some(clean_file_name(name));
                item.size = size;
                let item_id = item.id.clone();
                self.add(&party, item);
                item_id
            }
        };
        let mut inner = self.lock();
        inner.done.insert(item_id.clone(), 0);
        inner.incoming.insert(upload.clone(), Incoming { peer: peer.to_owned(), item: item_id, part, file, size, written: 0, last: Instant::now() });
        Ok(json!({ "uploadId": upload }))
    }

    pub fn write(&self, peer: &str, upload: &str, offset: u64, data: &str) -> Result<Value, &'static str> {
        if data.len() > 4 * CHUNK / 3 + 8 {
            return Err("badRequest");
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| "badRequest")?;
        let mut inner = self.lock();
        let incoming = inner.incoming.get_mut(upload).filter(|up| up.peer == peer).ok_or("mailboxUnknown")?;
        if offset != incoming.written || bytes.len() > CHUNK || incoming.written + bytes.len() as u64 > incoming.size {
            return Err("badRequest");
        }
        incoming.file.write_all(&bytes).map_err(|_| "filesDiskFull")?;
        incoming.written += bytes.len() as u64;
        incoming.last = Instant::now();
        let (item, written) = (incoming.item.clone(), incoming.written);
        inner.done.insert(item, written);
        Ok(Value::Null)
    }

    pub fn finish(&self, peer: &str, upload: &str) -> Result<Value, &'static str> {
        let incoming = {
            let mut inner = self.lock();
            match inner.incoming.get(upload) {
                Some(up) if up.peer == peer => inner.incoming.remove(upload),
                _ => None,
            }
        }
        .ok_or("mailboxUnknown")?;
        let party = Party::Peer(peer.to_owned());
        if incoming.written != incoming.size {
            let _ = fs::remove_file(&incoming.part);
            self.change(&party, &incoming.item, |item| {
                item.status = Status::Failed;
                item.error = Some("filesChanged".into());
            });
            return Err("badRequest");
        }
        let _ = incoming.file.sync_all();
        self.store_arrived(&party, &incoming.item, &incoming.part)?;
        Ok(Value::Null)
    }

    /// Moves a whole file into the receive folder and says that it arrived.
    fn store_arrived(&self, party: &Party, id: &str, part: &Path) -> Result<(), &'static str> {
        let (folder, name) = {
            let inner = self.lock();
            let name = inner.conversations.get(party).and_then(|items| items.iter().find(|item| item.id == id)).and_then(|item| item.name.clone()).unwrap_or_else(|| "file".into());
            (self.receive_folder(&inner, party), name)
        };
        let outcome = (|| -> Result<PathBuf, &'static str> {
            let folder = folder.ok_or("filesDenied")?;
            fs::DirBuilder::new().recursive(true).mode(0o755).create(&folder).map_err(|_| "filesDenied")?;
            let target = free_name(&folder, &name);
            move_file(part, &target)?;
            quarantine(&target);
            Ok(target)
        })();
        self.lock().done.remove(id);
        match outcome {
            Ok(target) => {
                if let Some(item) = self.change(party, id, |item| {
                    item.status = Status::Saved;
                    item.error = None;
                    item.read = false;
                    item.path = Some(target);
                    item.at = now();
                }) {
                    self.host.arrived(party, &item);
                }
                Ok(())
            }
            Err(code) => {
                let _ = fs::remove_file(part);
                self.change(party, id, |item| {
                    item.status = Status::Failed;
                    item.error = Some(code.to_owned());
                });
                Err(code)
            }
        }
    }

    pub fn abort(&self, peer: &str, upload: &str) -> Result<Value, &'static str> {
        let incoming = {
            let mut inner = self.lock();
            match inner.incoming.get(upload) {
                Some(up) if up.peer == peer => inner.incoming.remove(upload),
                _ => None,
            }
        };
        if let Some(incoming) = incoming {
            let _ = fs::remove_file(&incoming.part);
            self.lock().done.remove(&incoming.item);
            self.change(&Party::Peer(peer.to_owned()), &incoming.item, |item| item.status = Status::Stopped);
        }
        Ok(Value::Null)
    }

    /// What waits in the outbox for a peer.
    pub fn outbox(&self, peer: &str) -> Value {
        self.expire(now());
        let inner = self.lock();
        let items = inner.conversations.get(&Party::Peer(peer.to_owned())).map(Vec::as_slice).unwrap_or_default();
        let list: Vec<Value> = items
            .iter()
            .filter(|item| item.mine && matches!(item.status, Status::Waiting | Status::PickingUp))
            .take(MAX_POLL)
            .map(|item| match &item.text {
                Some(text) => json!({ "id": item.id, "at": item.at, "text": text }),
                None => json!({ "id": item.id, "at": item.at, "name": item.name, "size": item.size, "folder": item.folder }),
            })
            .collect();
        Value::Array(list)
    }

    /// A chunk of a file of the outbox.
    pub fn read(&self, peer: &str, id: &str, offset: u64) -> Result<Value, &'static str> {
        if !is_id(id) {
            return Err("badRequest");
        }
        let party = Party::Peer(peer.to_owned());
        let (path, status) = {
            let inner = self.lock();
            let item = inner.conversations.get(&party).and_then(|items| items.iter().find(|item| item.id == id && item.mine)).ok_or("mailboxUnknown")?;
            (item.path.clone().filter(|_| item.owned).ok_or("mailboxUnknown")?, item.status)
        };
        if !matches!(status, Status::Waiting | Status::PickingUp) {
            return Err("mailboxUnknown");
        }
        // Only a copy that this app made in the outbox of this peer is read.
        if !path.starts_with(self.dir.join("outbox").join(peer)) {
            return Err("mailboxUnknown");
        }
        let mut file = File::open(&path).map_err(|_| "mailboxMissing")?;
        let size = file.metadata().map_err(|_| "filesIo")?.len();
        if offset > size {
            return Err("badRequest");
        }
        file.seek(SeekFrom::Start(offset)).map_err(|_| "filesIo")?;
        let want = usize::try_from((size - offset).min(CHUNK as u64)).unwrap_or(CHUNK);
        let mut buffer = vec![0u8; want];
        file.read_exact(&mut buffer).map_err(|_| "filesChanged")?;
        let end = offset + want as u64;
        self.lock().done.insert(id.to_owned(), end);
        if status == Status::Waiting {
            self.change(&party, id, |item| item.status = Status::PickingUp);
        }
        Ok(json!({ "data": base64::engine::general_purpose::STANDARD.encode(&buffer), "size": size, "eof": end >= size }))
    }

    /// The peer has these items now: they are delivered, and their copies go.
    pub fn ack(&self, peer: &str, ids: &[String]) -> Result<Value, &'static str> {
        if ids.len() > MAX_POLL || ids.iter().any(|id| !is_id(id)) {
            return Err("badRequest");
        }
        let party = Party::Peer(peer.to_owned());
        for id in ids {
            self.lock().done.remove(id);
            let waiting = self
                .lock()
                .conversations
                .get(&party)
                .and_then(|items| items.iter().find(|item| item.id == *id))
                .filter(|item| item.mine && matches!(item.status, Status::Waiting | Status::PickingUp))
                .cloned();
            let Some(item) = waiting else { continue };
            // The copy goes before the item says delivered.
            Self::drop_owned(&item);
            self.change(&party, id, |item| {
                item.status = Status::Delivered;
                item.path = None;
                item.owned = false;
            });
        }
        Ok(Value::Null)
    }

    // ----- What this device does on its own -----

    /// Asks a device that this one controls for its outbox, and takes what waits there.
    pub fn poll(self: &Arc<Self>, device: &str) -> Result<(), &'static str> {
        let party = Party::Device(device.to_owned());
        let answer = match self.host.call(device, &json!({ "command": "mailboxPoll" })) {
            Ok(answer) => answer,
            Err(code) => {
                // Only a refusal says that the mailbox is off there. Any other error says nothing about it:
                // the device was not reached, or it did not understand, so it keeps what it said last.
                if code == "notPermitted" {
                    self.set_granted(device, false);
                } else {
                    self.note_check(device, Outcome::Unreachable, Some(code));
                }
                return Err(code);
            }
        };
        self.set_granted(device, true);
        let list = answer.as_array().ok_or("remoteFailed")?;
        let mut acks = Vec::new();
        for entry in list.iter().take(MAX_POLL) {
            let Some(remote) = entry.get("id").and_then(Value::as_str).filter(|id| is_id(id)) else { continue };
            if let Some(text) = entry.get("text").and_then(Value::as_str) {
                if !self.known(&party, remote) {
                    let Ok(text) = clean_text(text) else { continue };
                    let mut item = Item::new(false, Status::Received);
                    item.remote = Some(remote.to_owned());
                    item.text = Some(text);
                    self.add(&party, item.clone());
                    self.host.arrived(&party, &item);
                }
                acks.push(remote.to_owned());
                continue;
            }
            let Some(size) = entry.get("size").and_then(Value::as_u64).filter(|size| *size <= MAX_FILE_SIZE) else { continue };
            let name = clean_file_name(entry.get("name").and_then(Value::as_str).unwrap_or("file"));
            let folder = entry.get("folder").and_then(Value::as_bool).unwrap_or(false);
            // An item that is here already is fetched again only when it failed on its own.
            let existing = self.lock().conversations.get(&party).and_then(|items| items.iter().find(|item| item.remote.as_deref() == Some(remote)).cloned());
            match existing {
                Some(item) if item.status == Status::Failed => {
                    let _ = self.fetch(device, &item.id, remote, &name, size);
                }
                Some(item) if item.status == Status::Saved => acks.push(remote.to_owned()),
                Some(_) => {}
                None => {
                    let mut item = Item::new(false, Status::Arriving);
                    item.remote = Some(remote.to_owned());
                    item.name = Some(name.clone());
                    item.size = size;
                    item.folder = folder;
                    let id = item.id.clone();
                    self.add(&party, item);
                    let _ = self.fetch(device, &id, remote, &name, size);
                }
            }
        }
        if !acks.is_empty() {
            let _ = self.host.call(device, &json!({ "command": "mailboxAck", "args": { "ids": acks } }));
        }
        Ok(())
    }

    /// Queues the fetch of a file of a device's outbox.
    fn fetch(self: &Arc<Self>, device: &str, id: &str, remote: &str, name: &str, size: u64) -> Result<(), &'static str> {
        let incoming = self.dir.join("incoming");
        ensure_private_dir(&incoming)?;
        let part = incoming.join(format!("{}.part", new_id()));
        let party = Party::Device(device.to_owned());
        self.change(&party, id, |item| {
            item.status = Status::Arriving;
            item.error = None;
        });
        let this = self.clone();
        let (finish_device, finish_id, finish_remote, finish_part) = (device.to_owned(), id.to_owned(), remote.to_owned(), part.clone());
        let finish: Finish = Box::new(move |outcome| this.fetched(&finish_device, &finish_id, &finish_remote, &finish_part, outcome));
        let Some(job) = self.host.fetch_file(device, remote, name, size, part, finish) else {
            self.change(&party, id, |item| {
                item.status = Status::Failed;
                item.error = Some("workerUnavailable".into());
            });
            return Err("workerUnavailable");
        };
        let mut inner = self.lock();
        let moving = inner.conversations.get(&party).and_then(|items| items.iter().find(|item| item.id == id)).is_some_and(|item| item.status == Status::Arriving);
        if moving {
            inner.jobs.insert(id.to_owned(), job);
        }
        Ok(())
    }

    fn fetched(self: &Arc<Self>, device: &str, id: &str, remote: &str, part: &Path, outcome: Result<(), &'static str>) {
        self.lock().jobs.remove(id);
        let party = Party::Device(device.to_owned());
        match outcome {
            Ok(()) => {
                if self.store_arrived(&party, id, part).is_ok() {
                    let _ = self.host.call(device, &json!({ "command": "mailboxAck", "args": { "ids": [remote] } }));
                }
            }
            Err(code) => {
                let _ = fs::remove_file(part);
                self.change(&party, id, |item| {
                    item.status = if code == "stopped" { Status::Stopped } else { Status::Failed };
                    item.error = (code != "stopped").then(|| code.to_owned());
                });
            }
        }
    }

    /// Ends outbox items that waited too long, and incoming files that stopped moving.
    pub fn expire(&self, at: u64) {
        let mut changed = Vec::new();
        {
            let mut inner = self.lock();
            let parties: Vec<Party> = inner.conversations.keys().filter(|party| matches!(party, Party::Peer(_))).cloned().collect();
            for party in parties {
                let Some(items) = inner.conversations.get_mut(&party) else { continue };
                let mut touched = false;
                for item in items.iter_mut().filter(|item| item.mine && item.status == Status::Waiting && at.saturating_sub(item.at) > OUTBOX_SECONDS) {
                    Self::drop_owned(item);
                    item.status = Status::Expired;
                    item.path = None;
                    item.owned = false;
                    touched = true;
                }
                if touched {
                    self.save(&inner, &party);
                    changed.push(party);
                }
            }
        }
        let idle: Vec<Incoming> = {
            let mut inner = self.lock();
            let ids: Vec<String> = inner.incoming.iter().filter(|(_, up)| up.last.elapsed() >= INCOMING_IDLE).map(|(key, _)| key.clone()).collect();
            ids.iter().filter_map(|key| inner.incoming.remove(key)).collect()
        };
        for upload in idle {
            let _ = fs::remove_file(&upload.part);
            self.change(&Party::Peer(upload.peer.clone()), &upload.item, |item| {
                item.status = Status::Failed;
                item.error = Some("remoteTimeout".into());
            });
        }
        for party in &changed {
            self.host.changed(party);
        }
    }

    /// Trims every conversation to its limits.
    pub fn trim_all(&self, at: u64) {
        let mut inner = self.lock();
        let parties: Vec<Party> = inner.conversations.keys().cloned().collect();
        for party in parties {
            if let Some(items) = inner.conversations.get_mut(&party) {
                let before = items.len();
                Self::trim(items, at);
                if items.len() != before {
                    self.save(&inner, &party);
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn items(&self, party: &Party) -> Vec<Item> {
        self.lock().conversations.get(party).cloned().unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn set_items(&self, party: &Party, items: Vec<Item>) {
        let mut inner = self.lock();
        inner.conversations.insert(party.clone(), items);
        self.save(&inner, party);
    }
}

// ----- The app around the mailbox -----

/// The mailbox of the app, which it manages.
pub type Shared = Arc<Mailbox>;

fn party_of(party: crate::dispatch::MailboxParty) -> Result<Party, &'static str> {
    let party = match party {
        crate::dispatch::MailboxParty::Device(id) => Party::Device(id),
        crate::dispatch::MailboxParty::Peer(id) => Party::Peer(id),
    };
    if party.valid() { Ok(party) } else { Err("badRequest") }
}

/// Runs one mailbox request. It can wait for a device or a disk, so call it off the main thread.
pub fn run(app: &tauri::AppHandle, origin: crate::dispatch::Origin, request: crate::dispatch::Request) -> Result<Value, &'static str> {
    use crate::dispatch::Request;
    use tauri::Manager;
    let mailbox = app.try_state::<Shared>().map(|state| state.inner().clone()).ok_or("unsupported")?;
    let peer = || origin.peer().map(str::to_owned).ok_or("notPermitted");
    match request {
        Request::SetPeerMailbox { id, on } => {
            if !is_id(&id) {
                return Err("badRequest");
            }
            let service = app.state::<super::service::Service>();
            let was = service.set_mailbox(&id, on)?;
            if was && !on {
                mailbox.revoke_peer(&id);
            }
            Ok(Value::Null)
        }
        Request::MailboxPut { id, text } => mailbox.put(&peer()?, &id, &text),
        Request::MailboxBegin { id, name, size } => mailbox.begin(&peer()?, &id, &name, size),
        Request::MailboxWrite { upload_id, offset, data } => mailbox.write(&peer()?, &upload_id, offset, &data),
        Request::MailboxFinish { upload_id } => mailbox.finish(&peer()?, &upload_id),
        Request::MailboxAbort { upload_id } => mailbox.abort(&peer()?, &upload_id),
        Request::MailboxPoll => Ok(mailbox.outbox(&peer()?)),
        Request::MailboxRead { id, offset } => mailbox.read(&peer()?, &id, offset),
        Request::MailboxAck { ids } => mailbox.ack(&peer()?, &ids),
        Request::MailboxList { party } => serde_json::to_value(mailbox.conversation(&party_of(party)?)).map_err(|_| "workerUnavailable"),
        Request::MailboxSummary => serde_json::to_value(mailbox.summary()).map_err(|_| "workerUnavailable"),
        Request::MailboxCheck { party } => match party_of(party)? {
            Party::Device(id) => mailbox.poll(&id).map(|()| Value::Null).or_else(|code| if code == "notPermitted" { Ok(Value::Null) } else { Err(code) }),
            Party::Peer(_) => Ok(Value::Null),
        },
        Request::MailboxSendText { party, text } => mailbox.send_text(&party_of(party)?, &text).map(|()| Value::Null),
        Request::MailboxSendPaths { party, paths } => mailbox.send_paths(&party_of(party)?, &paths).map(|()| Value::Null),
        Request::MailboxCancel { party, id } => mailbox.cancel(&party_of(party)?, &id).map(|()| Value::Null),
        Request::MailboxRetry { party, id } => mailbox.retry(&party_of(party)?, &id).map(|()| Value::Null),
        Request::MailboxRemove { party, id } => mailbox.remove(&party_of(party)?, &id).map(|()| Value::Null),
        Request::MailboxClear { party } => {
            mailbox.clear(&party_of(party)?);
            Ok(Value::Null)
        }
        Request::MailboxMarkRead { party } => {
            let party = party_of(party)?;
            mailbox.mark_read(&party);
            // The person here opened the conversation, so the bell no longer needs to point to it.
            if matches!(origin, crate::dispatch::Origin::Local) {
                crate::inbox::dismiss_route(app, &party.route(), None);
            }
            Ok(Value::Null)
        }
        Request::MailboxReveal { party, id } => open_path(&mailbox.file_of(&party_of(party)?, &id)?, true).map(|()| Value::Null),
        Request::MailboxOpen { party, id } => open_path(&mailbox.file_of(&party_of(party)?, &id)?, false).map(|()| Value::Null),
        Request::MailboxSetFolder { folder } => mailbox.set_folder(folder).map(|()| Value::Null),
        _ => Err("badRequest"),
    }
}

/// Opens a file with its default application, or shows it in Finder. The path is absolute, so
/// `open` reads it as a file and never as an option.
fn open_path(path: &Path, reveal: bool) -> Result<(), &'static str> {
    let mut command = Command::new("/usr/bin/open");
    if reveal {
        command.arg("-R");
    }
    let done = command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if done { Ok(()) } else { Err("filesOpenFailed") }
}

/// The app as the mailbox sees it.
pub struct AppHost(pub tauri::AppHandle);

/// The event that tells the webview a conversation changed.
pub const CHANGED_EVENT: &str = "mailbox-changed";

impl Host for AppHost {
    fn call(&self, device: &str, request: &Value) -> Result<Value, &'static str> {
        use tauri::Manager;
        self.0.state::<super::service::Service>().forward(device, request)
    }

    fn send_file(&self, device: &str, item: &str, name: &str, local: PathBuf, finish: Finish) -> Option<u64> {
        use tauri::Manager;
        let transfers = self.0.try_state::<super::transfers::Transfers>()?;
        Some(transfers.queue_mailbox_send(device, item, name, local, finish))
    }

    fn fetch_file(&self, device: &str, item: &str, name: &str, size: u64, part: PathBuf, finish: Finish) -> Option<u64> {
        use tauri::Manager;
        let transfers = self.0.try_state::<super::transfers::Transfers>()?;
        Some(transfers.queue_mailbox_fetch(device, item, name, size, part, finish))
    }

    fn stop_job(&self, job: u64) {
        use tauri::Manager;
        if let Some(transfers) = self.0.try_state::<super::transfers::Transfers>() {
            transfers.stop(job);
        }
    }

    fn job(&self, job: u64) -> Option<(u64, u64, JobState)> {
        use tauri::Manager;
        self.0.try_state::<super::transfers::Transfers>()?.progress_of(job)
    }

    fn changed(&self, party: &Party) {
        use tauri::Emitter;
        let _ = self.0.emit_to("main", CHANGED_EVENT, party);
    }

    fn arrived(&self, party: &Party, item: &Item) {
        let device = self.name_of(party).unwrap_or_default();
        let mut params = std::collections::BTreeMap::new();
        params.insert("device".to_owned(), device);
        let key = match (&item.text, &item.name) {
            (Some(text), _) => {
                params.insert("item".to_owned(), text.clone());
                "mailboxText"
            }
            (None, name) => {
                params.insert("item".to_owned(), name.clone().unwrap_or_default());
                "mailboxFile"
            }
        };
        params.insert("detail".to_owned(), "mailboxOpen".to_owned());
        crate::inbox::record(
            &self.0,
            crate::inbox::NewEntry { kind: crate::inbox::Kind::Message, key: key.to_owned(), params, route: Some(party.route()) },
            crate::inbox::Alert::Yes,
            crate::inbox::Repeat::Allow,
        );
    }

    fn online(&self, party: &Party) -> Option<bool> {
        use tauri::Manager;
        let Party::Device(id) = party else { return None };
        let service = self.0.try_state::<super::service::Service>()?;
        service.remote_devices().into_iter().find(|device| device.id == *id)?.online
    }

    fn failed(&self, party: &Party, item: &Item) {
        let device = self.name_of(party).unwrap_or_default();
        let mut params = std::collections::BTreeMap::new();
        params.insert("device".to_owned(), device.clone());
        params.insert("name".to_owned(), device);
        params.insert("item".to_owned(), item.text.clone().or_else(|| item.name.clone()).unwrap_or_default());
        if let Some(reason) = &item.error {
            params.insert("detail".to_owned(), reason.clone());
        }
        crate::inbox::record(
            &self.0,
            crate::inbox::NewEntry { kind: crate::inbox::Kind::Error, key: "mailboxSendFailed".to_owned(), params, route: Some(party.route()) },
            crate::inbox::Alert::No,
            crate::inbox::Repeat::Allow,
        );
    }

    fn name_of(&self, party: &Party) -> Option<String> {
        use tauri::Manager;
        let service = self.0.try_state::<super::service::Service>()?;
        match party {
            Party::Device(id) => service.device(id).map(|device| device.name),
            Party::Peer(id) => service.peer_name(id),
        }
    }

    fn allowed(&self, party: &Party) -> bool {
        use tauri::Manager;
        let Some(service) = self.0.try_state::<super::service::Service>() else { return false };
        match party {
            Party::Device(id) => service.device(id).is_some(),
            Party::Peer(id) => service.peer_may_use_mailbox(id),
        }
    }
}

/// Removes the conversation of a device that is gone.
pub fn forget(app: &tauri::AppHandle, party: &Party) {
    use tauri::Manager;
    if let Some(mailbox) = app.try_state::<Shared>() {
        mailbox.forget(party);
    }
}

/// Asks every device that this one controls for its outbox, once a minute, for as long as the app lives.
pub fn start(app: &tauri::AppHandle) {
    let handle = app.clone();
    let _ = std::thread::Builder::new().name("mailbox-poll".into()).spawn(move || loop {
        use tauri::Manager;
        let Some(mailbox) = handle.try_state::<Shared>().map(|state| state.inner().clone()) else { return };
        let at = now();
        mailbox.expire(at);
        mailbox.trim_all(at);
        if let Some(service) = handle.try_state::<super::service::Service>() {
            for device in service.remote_devices() {
                // A device that was not reached lately is skipped, so a poll never waits on it.
                if device.online == Some(false) {
                    continue;
                }
                let _ = mailbox.poll(&device.id);
            }
        }
        std::thread::sleep(POLL_EVERY);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{
        testing::scratch,
        transfers::{Link, Transfers},
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    };

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// Hands each request of A to the mailbox of B, as B's dispatcher would for the peer A.
    /// B's mailbox, and whether B refuses A, as B's dispatcher does when the mailbox is off there.
    struct ToB(Arc<OnceLock<Arc<Mailbox>>>, Arc<AtomicBool>, Arc<Mutex<Option<&'static str>>>);

    impl ToB {
        fn answer(&self, request: &Value) -> Result<Value, &'static str> {
            if self.1.load(Ordering::Relaxed) {
                return Err("notPermitted");
            }
            if let Some(code) = *self.2.lock().unwrap() {
                return Err(code);
            }
            let b = self.0.get().ok_or("remoteFailed")?;
            let args = request.get("args").cloned().unwrap_or(Value::Null);
            let text = |name: &str| args.get(name).and_then(Value::as_str).unwrap_or_default().to_owned();
            let number = |name: &str| args.get(name).and_then(Value::as_u64).unwrap_or_default();
            match request.get("command").and_then(Value::as_str).unwrap_or_default() {
                "mailboxPut" => b.put(A, &text("id"), &text("text")),
                "mailboxBegin" => b.begin(A, &text("id"), &text("name"), number("size")),
                "mailboxWrite" => b.write(A, &text("uploadId"), number("offset"), &text("data")),
                "mailboxFinish" => b.finish(A, &text("uploadId")),
                "mailboxAbort" => b.abort(A, &text("uploadId")),
                "mailboxPoll" => Ok(b.outbox(A)),
                "mailboxRead" => b.read(A, &text("id"), number("offset")),
                "mailboxAck" => {
                    let ids: Vec<String> = serde_json::from_value(args.get("ids").cloned().unwrap_or_default()).map_err(|_| "badRequest")?;
                    b.ack(A, &ids)
                }
                _ => Err("badRequest"),
            }
        }
    }

    impl Link for ToB {
        fn call(&self, _device: &str, request: &Value) -> Result<Value, &'static str> {
            self.answer(request)
        }
    }

    struct TestHost {
        name: &'static str,
        to_b: Option<ToB>,
        transfers: Option<Transfers>,
        allowed: Arc<AtomicBool>,
        arrived: Arc<Mutex<Vec<Item>>>,
        failed: Arc<Mutex<Vec<Item>>>,
    }

    impl Host for TestHost {
        fn call(&self, _device: &str, request: &Value) -> Result<Value, &'static str> {
            self.to_b.as_ref().ok_or("remoteFailed")?.answer(request)
        }
        fn send_file(&self, device: &str, item: &str, name: &str, local: PathBuf, finish: Finish) -> Option<u64> {
            Some(self.transfers.as_ref()?.queue_mailbox_send(device, item, name, local, finish))
        }
        fn fetch_file(&self, device: &str, item: &str, name: &str, size: u64, part: PathBuf, finish: Finish) -> Option<u64> {
            Some(self.transfers.as_ref()?.queue_mailbox_fetch(device, item, name, size, part, finish))
        }
        fn stop_job(&self, job: u64) {
            if let Some(transfers) = &self.transfers {
                transfers.stop(job);
            }
        }
        fn job(&self, job: u64) -> Option<(u64, u64, JobState)> {
            self.transfers.as_ref()?.progress_of(job)
        }
        fn changed(&self, _party: &Party) {}
        fn arrived(&self, _party: &Party, item: &Item) {
            self.arrived.lock().unwrap().push(item.clone());
        }
        fn name_of(&self, _party: &Party) -> Option<String> {
            Some(self.name.to_owned())
        }
        fn allowed(&self, _party: &Party) -> bool {
            self.allowed.load(Ordering::Relaxed)
        }
        fn failed(&self, _party: &Party, item: &Item) {
            self.failed.lock().unwrap().push(item.clone());
        }
        fn spawn(&self, work: Box<dyn FnOnce() + Send>) {
            work();
        }
    }

    struct Pair {
        root: PathBuf,
        a: Arc<Mailbox>,
        b: Arc<Mailbox>,
        b_allowed: Arc<AtomicBool>,
        a_arrived: Arc<Mutex<Vec<Item>>>,
        b_arrived: Arc<Mutex<Vec<Item>>>,
        refuse: Arc<AtomicBool>,
        a_failed: Arc<Mutex<Vec<Item>>>,
        link_error: Arc<Mutex<Option<&'static str>>>,
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn pair() -> Pair {
        let root = scratch("mailbox");
        let slot: Arc<OnceLock<Arc<Mailbox>>> = Arc::default();
        let b_allowed = Arc::new(AtomicBool::new(true));
        let (a_arrived, b_arrived): (Arc<Mutex<Vec<Item>>>, Arc<Mutex<Vec<Item>>>) = Default::default();
        let refuse = Arc::new(AtomicBool::new(false));
        let link_error: Arc<Mutex<Option<&'static str>>> = Arc::default();
        let a_failed: Arc<Mutex<Vec<Item>>> = Arc::default();
        let b = Mailbox::open(
            root.join("b"),
            None,
            Some(root.join("b-downloads")),
            TestHost { name: "Laptop", to_b: None, transfers: None, allowed: b_allowed.clone(), arrived: b_arrived.clone(), failed: Arc::default() },
        )
        .unwrap();
        slot.set(b.clone()).ok().unwrap();
        let a = Mailbox::open(
            root.join("a"),
            None,
            Some(root.join("a-downloads")),
            TestHost {
                name: "Desk",
                to_b: Some(ToB(slot.clone(), refuse.clone(), link_error.clone())),
                transfers: Some(Transfers::new(ToB(slot, refuse.clone(), link_error.clone()))),
                allowed: Arc::new(AtomicBool::new(true)),
                arrived: a_arrived.clone(),
                failed: a_failed.clone(),
            },
        )
        .unwrap();
        Pair { root, a, b, b_allowed, a_arrived, b_arrived, refuse, a_failed, link_error }
    }

    fn wait(what: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !what() {
            assert!(Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn file(root: &Path, name: &str, size: usize) -> PathBuf {
        let path = root.join(name);
        let data: Vec<u8> = (0..size).map(|n| (n % 251) as u8).collect();
        fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn texts_and_names_are_checked_and_cleaned() {
        assert_eq!(clean_text(" hi\u{202E}there\n ").unwrap(), "hithere");
        assert_eq!(clean_text("   "), Err("mailboxTextEmpty"));
        assert_eq!(clean_text(&"x".repeat(MAX_TEXT + 1)), Err("mailboxTextTooLong"));
        assert_eq!(clean_file_name("../etc/passwd"), "..-etc-passwd");
        assert_eq!(clean_file_name(".."), "file");
        assert_eq!(clean_file_name("a\u{0}b\u{200B}c.txt"), "abc.txt");
        assert!(clean_file_name(&"é".repeat(300)).len() <= MAX_NAME);
        assert!(is_id(A) && !is_id("ABC") && !is_id(&A.replace('a', "g")));
    }

    #[test]
    fn a_free_name_gets_a_number() {
        let root = scratch("mailbox-free");
        fs::write(root.join("report.pdf"), b"x").unwrap();
        assert_eq!(free_name(&root, "report.pdf"), root.join("report 2.pdf"));
        assert_eq!(free_name(&root, "new.pdf"), root.join("new.pdf"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_text_goes_from_the_controller_at_once_and_back_through_the_outbox() {
        let pair = pair();
        pair.a.send_text(&Party::Device(B.into()), "hello B").unwrap();
        let sent = pair.a.items(&Party::Device(B.into()));
        assert_eq!(sent[0].status, Status::Delivered);
        let got = pair.b.items(&Party::Peer(A.into()));
        assert_eq!((got[0].text.as_deref(), got[0].status, got[0].read), (Some("hello B"), Status::Received, false));
        assert_eq!(pair.b_arrived.lock().unwrap().len(), 1);
        // The same text again is a duplicate, and the sender takes it as delivered.
        assert_eq!(pair.b.put(A, got[0].remote.as_deref().unwrap(), "hello B"), Err("mailboxDuplicate"));

        pair.b.send_text(&Party::Peer(A.into()), "hello A").unwrap();
        assert_eq!(pair.b.items(&Party::Peer(A.into()))[1].status, Status::Waiting);
        pair.a.poll(B).unwrap();
        let got = pair.a.items(&Party::Device(B.into()));
        assert_eq!(got[1].text.as_deref(), Some("hello A"));
        assert_eq!(pair.b.items(&Party::Peer(A.into()))[1].status, Status::Delivered);
        // A second poll brings nothing new.
        pair.a.poll(B).unwrap();
        assert_eq!(pair.a.items(&Party::Device(B.into())).len(), 2);
        assert_eq!(pair.a_arrived.lock().unwrap().len(), 1);
        assert_eq!(pair.a.conversation(&Party::Device(B.into())).granted, Some(true));
    }

    #[test]
    fn a_file_goes_both_ways_and_the_outbox_copy_goes_after() {
        let pair = pair();
        let source = file(&pair.root, "clip.bin", CHUNK * 2 + 77);
        pair.a.send_paths(&Party::Device(B.into()), &[source.to_string_lossy().into()]).unwrap();
        wait(|| pair.a.items(&Party::Device(B.into()))[0].status == Status::Delivered);
        let got = &pair.b.items(&Party::Peer(A.into()))[0];
        assert_eq!(got.status, Status::Saved);
        let saved = got.path.clone().unwrap();
        assert_eq!(saved, pair.root.join("b-downloads/Lazy Worker/Laptop/clip.bin"));
        assert_eq!(fs::read(&saved).unwrap(), fs::read(&source).unwrap());
        // The app keeps no copy: the incoming folder is empty.
        assert_eq!(fs::read_dir(pair.root.join("b/incoming")).unwrap().count(), 0);

        // The same name again gets a number.
        pair.a.send_paths(&Party::Device(B.into()), &[source.to_string_lossy().into()]).unwrap();
        wait(|| pair.b.items(&Party::Peer(A.into())).len() == 2 && pair.b.items(&Party::Peer(A.into()))[1].status == Status::Saved);
        assert!(pair.root.join("b-downloads/Lazy Worker/Laptop/clip 2.bin").exists());

        let back = file(&pair.root, "photo.png", 1000);
        pair.b.send_paths(&Party::Peer(A.into()), &[back.to_string_lossy().into()]).unwrap();
        let waiting = pair.b.items(&Party::Peer(A.into()))[2].clone();
        assert_eq!(waiting.status, Status::Waiting);
        let copy = waiting.path.clone().unwrap();
        assert!(copy.starts_with(pair.root.join("b/outbox").join(A)));
        // The outbox holds a snapshot: a change to the original after the send does not reach it.
        fs::write(&back, b"changed").unwrap();
        pair.a.poll(B).unwrap();
        wait(|| pair.a.items(&Party::Device(B.into())).last().is_some_and(|item| item.status == Status::Saved));
        let arrived = pair.a.items(&Party::Device(B.into())).last().cloned().unwrap();
        assert_eq!(fs::read(arrived.path.unwrap()).unwrap().len(), 1000);
        wait(|| pair.b.items(&Party::Peer(A.into()))[2].status == Status::Delivered);
        assert!(!copy.exists());
    }

    #[test]
    fn a_folder_is_compressed_before_it_goes() {
        let pair = pair();
        let folder = pair.root.join("Clips");
        fs::create_dir_all(&folder).unwrap();
        file(&folder, "one.txt", 10);
        pair.a.send_paths(&Party::Device(B.into()), &[folder.to_string_lossy().into()]).unwrap();
        wait(|| pair.a.items(&Party::Device(B.into()))[0].status == Status::Delivered);
        let got = &pair.b.items(&Party::Peer(A.into()))[0];
        assert_eq!(got.name.as_deref(), Some("Clips.zip"));
        assert!(got.path.as_ref().unwrap().exists());
        // The zip that was made for the send is gone.
        assert_eq!(fs::read_dir(pair.root.join("a/zips")).map(Iterator::count).unwrap_or(0), 0);
    }

    #[test]
    fn the_outbox_expires_and_a_revoked_peer_loses_it() {
        let pair = pair();
        let back = file(&pair.root, "note.txt", 10);
        pair.b.send_paths(&Party::Peer(A.into()), &[back.to_string_lossy().into()]).unwrap();
        let copy = pair.b.items(&Party::Peer(A.into()))[0].path.clone().unwrap();
        pair.b.expire(now() + OUTBOX_SECONDS + 10);
        let item = &pair.b.items(&Party::Peer(A.into()))[0];
        assert_eq!(item.status, Status::Expired);
        assert!(!copy.exists());

        pair.b.send_paths(&Party::Peer(A.into()), &[back.to_string_lossy().into()]).unwrap();
        pair.b.send_text(&Party::Peer(A.into()), "later").unwrap();
        pair.b_allowed.store(false, Ordering::Relaxed);
        pair.b.revoke_peer(A);
        assert!(pair.b.items(&Party::Peer(A.into())).iter().all(|item| item.status == Status::Expired));
        assert!(!pair.root.join("b/outbox").join(A).exists());
        assert_eq!(pair.b.send_text(&Party::Peer(A.into()), "x"), Err("notPermitted"));
    }

    #[test]
    fn the_outbox_holds_twenty_and_reads_only_its_own_copies() {
        let pair = pair();
        for n in 0..MAX_PENDING {
            pair.b.send_text(&Party::Peer(A.into()), &format!("m{n}")).unwrap();
        }
        assert_eq!(pair.b.send_text(&Party::Peer(A.into()), "one more"), Err("mailboxFull"));
        // An item whose path lies outside the outbox is never read.
        let mut item = Item::new(true, Status::Waiting);
        item.path = Some(PathBuf::from("/etc/hosts"));
        item.owned = true;
        item.name = Some("hosts".into());
        let id = item.id.clone();
        pair.b.set_items(&Party::Peer(A.into()), vec![item]);
        assert_eq!(pair.b.read(A, &id, 0), Err("mailboxUnknown"));
        assert_eq!(pair.b.read(B, &id, 0), Err("mailboxUnknown"));
    }

    #[test]
    fn incoming_files_are_bounded_and_confined() {
        let pair = pair();
        assert_eq!(pair.b.begin(A, "not-an-id", "x", 1), Err("badRequest"));
        assert_eq!(pair.b.begin(A, &new_id(), "x", MAX_FILE_SIZE + 1), Err("filesTooLarge"));
        let begun = pair.b.begin(A, &new_id(), "../../escape.txt", 4).unwrap();
        let upload = begun["uploadId"].as_str().unwrap().to_owned();
        let data = base64::engine::general_purpose::STANDARD.encode(b"abcd");
        // Another device cannot write to it, and an offset must follow what was written.
        assert_eq!(pair.b.write(B, &upload, 0, &data), Err("mailboxUnknown"));
        assert_eq!(pair.b.write(A, &upload, 1, &data), Err("badRequest"));
        pair.b.write(A, &upload, 0, &data).unwrap();
        pair.b.finish(A, &upload).unwrap();
        let saved = pair.b.items(&Party::Peer(A.into()))[0].path.clone().unwrap();
        assert_eq!(saved, pair.root.join("b-downloads/Lazy Worker/Laptop/..-..-escape.txt"));
        for _ in 0..MAX_INCOMING_PER_PEER {
            pair.b.begin(A, &new_id(), "x", 1).unwrap();
        }
        assert_eq!(pair.b.begin(A, &new_id(), "x", 1), Err("mailboxBusy"));
    }

    #[test]
    fn history_keeps_two_hundred_items_of_thirty_days() {
        let pair = pair();
        let party = Party::Peer(A.into());
        let at = now();
        let mut items: Vec<Item> = (0..KEEP_ITEMS + 10)
            .map(|n| {
                let mut item = Item::new(false, Status::Received);
                item.text = Some(format!("{n}"));
                item.at = at - 10;
                item
            })
            .collect();
        items[0].at = at - KEEP_SECONDS - 10;
        pair.b.set_items(&party, items);
        pair.b.trim_all(at);
        let kept = pair.b.items(&party);
        assert_eq!(kept.len(), KEEP_ITEMS);
        assert_eq!(kept[0].text.as_deref(), Some("10"));
        // It is read back the same after a restart.
        drop(pair.b.clone());
        let reopened: Vec<Item> = serde_json::from_slice(&fs::read(pair.root.join("b").join(party.file_name())).unwrap()).unwrap();
        assert_eq!(reopened, kept);
    }

    #[test]
    fn a_refused_send_fails_with_a_reason_and_marks_the_mailbox_off() {
        let pair = pair();
        let party = Party::Device(B.into());
        pair.refuse.store(true, Ordering::Relaxed);
        pair.a.send_text(&party, "hello").unwrap();
        let item = pair.a.items(&party)[0].clone();
        assert_eq!((item.status, item.error.as_deref()), (Status::Failed, Some("mailboxOffThere")));
        assert_eq!(pair.a.conversation(&party).granted, Some(false));
        assert_eq!(pair.a_failed.lock().unwrap().len(), 1);

        // While it is known to be off, a file fails at once and nothing reaches the queue.
        let source = file(&pair.root, "a.txt", 10);
        pair.a.send_paths(&party, &[source.to_string_lossy().into()]).unwrap();
        let item = pair.a.items(&party)[1].clone();
        assert_eq!((item.status, item.error.as_deref()), (Status::Failed, Some("mailboxOffThere")));
        assert_eq!(pair.a_failed.lock().unwrap().len(), 2);

        // Once B allows it again, Try again sends both, and the mailbox counts as on.
        pair.refuse.store(false, Ordering::Relaxed);
        let ids: Vec<String> = pair.a.items(&party).iter().map(|item| item.id.clone()).collect();
        for id in &ids {
            pair.a.retry(&party, id).unwrap();
        }
        wait(|| pair.a.items(&party).iter().all(|item| item.status == Status::Delivered));
        assert_eq!(pair.a.conversation(&party).granted, Some(true));
        assert_eq!(pair.b.items(&Party::Peer(A.into())).len(), 2);
    }

    #[test]
    fn a_refused_file_in_flight_fails_and_marks_the_mailbox_off() {
        let pair = pair();
        let party = Party::Device(B.into());
        pair.a.poll(B).unwrap();
        assert_eq!(pair.a.conversation(&party).granted, Some(true));
        pair.refuse.store(true, Ordering::Relaxed);
        let source = file(&pair.root, "b.txt", 10);
        pair.a.send_paths(&party, &[source.to_string_lossy().into()]).unwrap();
        wait(|| pair.a.items(&party)[0].status == Status::Failed);
        assert_eq!(pair.a.items(&party)[0].error.as_deref(), Some("mailboxOffThere"));
        assert_eq!(pair.a.conversation(&party).granted, Some(false));
        assert_eq!(failure_reason("sshTimeout"), "mailboxOffline");
        assert_eq!(failure_reason("filesChanged"), "filesChanged");
    }

    #[test]
    fn only_a_refusal_turns_the_mailbox_off_and_every_check_is_kept() {
        let pair = pair();
        let party = Party::Device(B.into());
        pair.a.poll(B).unwrap();
        let check = pair.a.conversation(&party).check.unwrap();
        assert_eq!((check.outcome, check.reason), (Outcome::Allowed, None));

        // A device that is not reached, or that does not understand, keeps what it said last.
        for code in ["sshTimeout", "busy", "badRequest"] {
            *pair.link_error.lock().unwrap() = Some(code);
            assert_eq!(pair.a.poll(B), Err(code));
            let conversation = pair.a.conversation(&party);
            assert_eq!(conversation.granted, Some(true), "{code}");
            assert_eq!(conversation.check.map(|check| (check.outcome, check.reason)), Some((Outcome::Unreachable, Some(code))));
        }
        *pair.link_error.lock().unwrap() = None;

        pair.refuse.store(true, Ordering::Relaxed);
        assert_eq!(pair.a.poll(B), Err("notPermitted"));
        assert_eq!(pair.a.conversation(&party).granted, Some(false));
        assert_eq!(pair.a.conversation(&party).check.unwrap().outcome, Outcome::Off);
        // A failed check after it does not hide that the device said off; the next clear answer clears it.
        pair.refuse.store(false, Ordering::Relaxed);
        *pair.link_error.lock().unwrap() = Some("sshTimeout");
        let _ = pair.a.poll(B);
        assert_eq!(pair.a.conversation(&party).granted, Some(false));
        *pair.link_error.lock().unwrap() = None;
        pair.a.poll(B).unwrap();
        assert_eq!(pair.a.conversation(&party).granted, Some(true));
        let summary = pair.a.summary();
        let entry = summary.iter().find(|entry| entry.party == party).unwrap();
        assert_eq!((entry.granted, entry.check.map(|check| check.outcome)), (Some(true), Some(Outcome::Allowed)));
    }
}
