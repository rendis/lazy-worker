//! The files that this device shares with a paired device.
//!
//! Every path in a request is relative to one shared folder, named by its id. A path is checked as
//! text first, then resolved with `canonicalize`, and the result must still lie inside that folder.
//! Nothing here reads or writes outside the folders the user chose, and the data of this app and
//! the SSH keys are never shared, even with the whole disk.

use super::{
    log::{Entry, Log},
    now,
};
use crate::{
    devices::{FilesGrant, FilesMode, FilesScope},
    dispatch::Request,
};
use base64::Engine;
use rand_core::{OsRng, RngCore};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The most that one request moves, in bytes of file data. As base64 it fits a 1 MiB line.
pub const CHUNK: usize = 512 * 1024;
/// The largest file that an upload may announce.
pub const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_FOLDERS: usize = 16;
const MAX_PATH: usize = 4096;
const MAX_NAME: usize = 255;
/// The most entries that one listing returns.
const MAX_LIST: usize = 5000;
/// The most entries that a recursive delete removes.
const DELETE_CAP: usize = 10_000;
const MAX_UPLOADS_PER_PEER: usize = 4;
/// An upload that gets no chunk for this long is dropped, and its partial file removed.
pub const UPLOAD_IDLE: Duration = Duration::from_secs(120);

/// A folder that a device may open: its id in requests, its name for people, and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

/// `~/Documents` for a path in the home folder, and the path as it is for any other.
pub fn tilde(path: &str, home: Option<&Path>) -> String {
    match home.and_then(|home| Path::new(path).strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.to_owned(),
    }
}

/// The folders of a grant as roots. A folder that is gone is left out. The whole disk is `/`, and
/// the home folder is one root with no name, which the interface calls "Home folder".
pub fn roots(grant: &FilesGrant) -> Vec<Root> {
    match grant.scope {
        FilesScope::Disk => vec![Root {
            id: "disk".to_owned(),
            name: String::new(),
            path: PathBuf::from("/"),
        }],
        FilesScope::Folders | FilesScope::Home => grant
            .folders
            .iter()
            .filter_map(|folder| {
                let path = fs::canonicalize(folder).ok().filter(|path| path.is_dir())?;
                let digest = Sha256::digest(path.as_os_str().as_encoded_bytes());
                let id = digest[..4].iter().map(|byte| format!("{byte:02x}")).collect();
                if grant.scope == FilesScope::Home {
                    return Some(Root { id: "home".to_owned(), name: String::new(), path });
                }
                let name = path.file_name()?.to_string_lossy().into_owned();
                Some(Root { id, name, path })
            })
            .collect(),
    }
}

/// What the Connect tab of the other device reads: whether files are shared, and which folders.
pub fn share_view(grant: Option<&FilesGrant>, home: Option<&Path>) -> Value {
    let Some(grant) = grant.filter(|grant| grant.until.active(now())) else {
        return json!({ "on": false });
    };
    let list: Vec<Value> = roots(grant)
        .into_iter()
        .map(|root| json!({
            "id": root.id,
            "name": root.name,
            "display": tilde(&root.path.to_string_lossy(), home),
        }))
        .collect();
    json!({ "on": true, "scope": grant.scope, "mode": grant.mode, "until": grant.until, "roots": list })
}

/// One name inside a folder: not empty, no `/`, no NUL, not `.` or `..`, and at most 255 bytes.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name != "."
        && name != ".."
        && !name.contains(['/', '\0'])
}

/// The names of a relative path. An empty path is the folder itself. `..`, `.`, an empty part, an
/// absolute path, NUL, a part over 255 bytes, and a path over 4096 bytes are refused.
fn components(path: &str) -> Result<Vec<&str>, &'static str> {
    if path.len() > MAX_PATH || path.contains('\0') || path.starts_with('/') {
        return Err("filesPathInvalid");
    }
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.iter().all(|part| valid_name(part)) {
        Ok(parts)
    } else {
        Err("filesPathInvalid")
    }
}

/// A file or folder of the shared folder, as a listing shows it.
fn kind_of(meta: &fs::Metadata) -> Option<&'static str> {
    if meta.is_dir() {
        Some("folder")
    } else if meta.is_file() {
        Some("file")
    } else {
        None
    }
}

fn io_code(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "filesNotFound",
        std::io::ErrorKind::AlreadyExists => "filesExists",
        // EPERM is how macOS answers a folder that its privacy rules close to this app. EACCES is
        // an ordinary file permission.
        _ if error.raw_os_error() == Some(libc::EPERM) => "filesPrivacyDenied",
        std::io::ErrorKind::PermissionDenied => "filesDenied",
        _ => match error.raw_os_error() {
            Some(libc::ENOTEMPTY) => "filesNotEmpty",
            Some(libc::ENOSPC | libc::EDQUOT) => "filesDiskFull",
            Some(libc::ELOOP) => "filesOutside",
            _ => "filesIo",
        },
    }
}

fn modified(meta: &fs::Metadata) -> u64 {
    u64::try_from(meta.mtime()).unwrap_or(0)
}

/// An upload that is in progress on this device.
struct Upload {
    peer: String,
    temp: PathBuf,
    dest: PathBuf,
    /// The path that the log shows.
    label: String,
    size: u64,
    written: u64,
    file: File,
    last: Instant,
}

/// The uploads in progress, for all devices. The partial file of an upload is removed when the
/// upload is cancelled, finishes badly, is idle too long, or its device loses the permission.
#[derive(Default)]
pub struct Uploads {
    map: Mutex<HashMap<String, Arc<Mutex<Upload>>>>,
}

impl Uploads {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Mutex<Upload>>>> {
        self.map.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn remove_partial(upload: &Arc<Mutex<Upload>>) {
        let upload = upload.lock().unwrap_or_else(|error| error.into_inner());
        let _ = fs::remove_file(&upload.temp);
    }

    /// Ends every upload of a device and removes its partial files. Returns how many there were.
    pub fn cancel_peer(&self, peer: &str) -> usize {
        let taken: Vec<Arc<Mutex<Upload>>> = {
            let mut map = self.lock();
            let ids: Vec<String> = map
                .iter()
                .filter(|(_, upload)| upload.lock().unwrap_or_else(|e| e.into_inner()).peer == peer)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| map.remove(id)).collect()
        };
        for upload in &taken {
            Self::remove_partial(upload);
        }
        taken.len()
    }

    /// Ends the uploads that are idle past `UPLOAD_IDLE` at `at`, and those whose device is not
    /// allowed any more (`allowed` says so by device id).
    pub fn sweep(&self, at: Instant, allowed: impl Fn(&str) -> bool) {
        let dead: Vec<Arc<Mutex<Upload>>> = {
            let mut map = self.lock();
            let ids: Vec<String> = map
                .iter()
                .filter(|(_, upload)| {
                    let upload = upload.lock().unwrap_or_else(|e| e.into_inner());
                    at.saturating_duration_since(upload.last) >= UPLOAD_IDLE || !allowed(&upload.peer)
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| map.remove(id)).collect()
        };
        for upload in &dead {
            Self::remove_partial(upload);
        }
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        self.lock().len()
    }
}

/// Everything that one request needs: the grant of the device that sent it, read just now.
pub struct Files {
    pub grant: FilesGrant,
    pub peer: String,
    pub name: String,
    /// Paths that are never shared: the data of this app and the SSH keys. Canonical when they exist.
    pub protected: Vec<PathBuf>,
    pub uploads: Arc<Uploads>,
    pub log: Option<Arc<Log>>,
}

impl Files {
    fn root(&self, id: &str) -> Result<Root, &'static str> {
        roots(&self.grant)
            .into_iter()
            .find(|root| root.id == id)
            .ok_or("filesRootUnknown")
    }

    fn is_protected(&self, path: &Path) -> bool {
        self.protected.iter().any(|protected| path.starts_with(protected))
    }

    /// The folder or file at a path, as a canonical path that lies inside the root and is not
    /// protected. A link is followed, and one that leaves the root is refused.
    fn existing(&self, root: &Root, path: &str) -> Result<PathBuf, &'static str> {
        let mut joined = root.path.clone();
        joined.extend(components(path)?);
        let canonical = fs::canonicalize(&joined).map_err(|error| io_code(&error))?;
        if !canonical.starts_with(&root.path) {
            return Err("filesOutside");
        }
        if self.is_protected(&canonical) {
            return Err("filesProtected");
        }
        Ok(canonical)
    }

    /// The canonical folder that holds the last name of `path`, and that name. The entry itself may be a link.
    fn child(&self, root: &Root, path: &str) -> Result<(PathBuf, String), &'static str> {
        let mut parts = components(path)?;
        let name = parts.pop().ok_or("filesPathInvalid")?;
        let parent = self.existing(root, &parts.join("/"))?;
        Ok((parent, name.to_owned()))
    }

    fn folder(&self, root: &Root, path: &str) -> Result<PathBuf, &'static str> {
        let folder = self.existing(root, path)?;
        if !fs::metadata(&folder).map_err(|error| io_code(&error))?.is_dir() {
            return Err("filesNotFolder");
        }
        Ok(folder)
    }

    /// A regular file, as its canonical path and metadata. A folder, device, socket, or pipe is refused.
    fn file(&self, root: &Root, path: &str) -> Result<(PathBuf, fs::Metadata), &'static str> {
        let file = self.existing(root, path)?;
        let meta = fs::metadata(&file).map_err(|error| io_code(&error))?;
        if meta.is_dir() {
            return Err("filesIsFolder");
        }
        if !meta.is_file() {
            return Err("filesNotRegular");
        }
        Ok((file, meta))
    }

    fn record(&self, event: &str, outcome: &str, root: &Root, path: &str, size: Option<u64>) {
        if let Some(log) = &self.log {
            let shown = if root.name.is_empty() { path.to_owned() } else { format!("{}/{}", root.name, path) };
            let _ = log.append(&Entry::new(Some(&self.peer), Some(&self.name), event, outcome).with_file(&shown, size));
        }
    }

    /// Runs one files request for this device. A grant that has ended refuses it, and a read-only
    /// grant refuses every write.
    pub fn run(&self, request: Request) -> Result<Value, &'static str> {
        if !self.grant.until.active(now()) {
            return Err("notPermitted");
        }
        let writes = !matches!(
            request,
            Request::ListFiles { .. } | Request::StatFile { .. } | Request::ReadFileChunk { .. }
        );
        if writes && self.grant.mode != FilesMode::ReadWrite {
            return Err("notPermitted");
        }
        match request {
            Request::ListFiles { root, path } => self.list(&root, &path),
            Request::StatFile { root, path } => self.stat(&root, &path),
            Request::ReadFileChunk { root, path, offset, length } => self.read(&root, &path, offset, length),
            Request::BeginUpload { root, path, name, size } => self.begin(&root, &path, &name, size),
            Request::WriteFileChunk { upload_id, offset, data } => self.write(&upload_id, offset, &data),
            Request::FinishUpload { upload_id } => self.finish(&upload_id),
            Request::CancelUpload { upload_id } => self.cancel(&upload_id),
            Request::MakeFolder { root, path, name } => self.make_folder(&root, &path, &name),
            Request::DeleteEntry { root, path, recursive } => self.delete(&root, &path, recursive),
            Request::RenameEntry { root, path, name } => self.rename(&root, &path, &name),
            _ => Err("badRequest"),
        }
    }

    fn list(&self, root: &str, path: &str) -> Result<Value, &'static str> {
        let root = self.root(root)?;
        let folder = self.folder(&root, path)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        for item in fs::read_dir(&folder).map_err(|error| io_code(&error))? {
            let Ok(item) = item else { continue };
            let Some(name) = item.file_name().to_str().map(str::to_owned) else { continue };
            let entry_path = folder.join(&name);
            let Ok(link) = fs::symlink_metadata(&entry_path) else { continue };
            // A link is shown as what it points to, and only when that lies inside the root.
            let (meta, target) = if link.file_type().is_symlink() {
                let Ok(target) = fs::canonicalize(&entry_path) else { continue };
                let Ok(meta) = fs::metadata(&target) else { continue };
                if !target.starts_with(&root.path) {
                    continue;
                }
                (meta, target)
            } else {
                (link, entry_path)
            };
            let Some(kind) = kind_of(&meta) else { continue };
            if self.is_protected(&target) {
                continue;
            }
            if entries.len() >= MAX_LIST {
                truncated = true;
                break;
            }
            entries.push(json!({
                "name": name,
                "kind": kind,
                "size": if kind == "file" { meta.len() } else { 0 },
                "modified": modified(&meta),
            }));
        }
        entries.sort_by_cached_key(|entry| {
            (
                entry["kind"] != "folder",
                entry["name"].as_str().unwrap_or_default().to_lowercase(),
            )
        });
        Ok(json!({ "entries": entries, "truncated": truncated }))
    }

    fn stat(&self, root: &str, path: &str) -> Result<Value, &'static str> {
        let root = self.root(root)?;
        let target = self.existing(&root, path)?;
        let meta = fs::metadata(&target).map_err(|error| io_code(&error))?;
        let kind = kind_of(&meta).ok_or("filesNotRegular")?;
        Ok(json!({ "kind": kind, "size": if kind == "file" { meta.len() } else { 0 }, "modified": modified(&meta) }))
    }

    fn read(&self, root: &str, path: &str, offset: u64, length: u32) -> Result<Value, &'static str> {
        let length = usize::try_from(length).unwrap_or(usize::MAX);
        if length == 0 || length > CHUNK {
            return Err("filesChunkInvalid");
        }
        let root = self.root(root)?;
        let (file, meta) = self.file(&root, path)?;
        let size = meta.len();
        if offset == 0 {
            self.record("fileDownload", "ok", &root, path, Some(size));
        }
        let mut data = Vec::new();
        if offset < size {
            let mut open = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&file)
                .map_err(|error| io_code(&error))?;
            open.seek(SeekFrom::Start(offset)).map_err(|error| io_code(&error))?;
            open.take(length as u64).read_to_end(&mut data).map_err(|error| io_code(&error))?;
        }
        let eof = offset.saturating_add(data.len() as u64) >= size;
        Ok(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
            "size": size,
            "eof": eof,
        }))
    }

    fn begin(&self, root: &str, path: &str, name: &str, size: u64) -> Result<Value, &'static str> {
        if size > MAX_FILE_SIZE {
            return Err("filesTooLarge");
        }
        if !valid_name(name) {
            return Err("filesPathInvalid");
        }
        let root = self.root(root)?;
        let folder = self.folder(&root, path)?;
        let dest = folder.join(name);
        if self.is_protected(&dest) {
            return Err("filesProtected");
        }
        match fs::symlink_metadata(&dest) {
            Ok(meta) if meta.is_dir() => return Err("filesIsFolder"),
            Ok(meta) if !meta.is_file() && !meta.file_type().is_symlink() => return Err("filesNotRegular"),
            _ => {}
        }
        let mut bytes = [0u8; 16];
        OsRng.try_fill_bytes(&mut bytes).map_err(|_| "filesIo")?;
        let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        // The partial file has a short name of its own, so a long name never makes it too long.
        let temp = folder.join(format!(".lw-upload-{id}.part"));
        let mut map = self.uploads.lock();
        let mine = map
            .values()
            .filter(|upload| upload.lock().unwrap_or_else(|e| e.into_inner()).peer == self.peer)
            .count();
        if mine >= MAX_UPLOADS_PER_PEER {
            return Err("filesTooManyUploads");
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)
            .map_err(|error| io_code(&error))?;
        let label = if root.name.is_empty() { format!("{path}/{name}") } else { format!("{}/{path}/{name}", root.name) };
        let label = label.replace("//", "/");
        map.insert(
            id.clone(),
            Arc::new(Mutex::new(Upload {
                peer: self.peer.clone(),
                temp,
                dest,
                label,
                size,
                written: 0,
                file,
                last: Instant::now(),
            })),
        );
        Ok(json!({ "uploadId": id }))
    }

    /// The upload of this device with that id.
    fn upload(&self, id: &str) -> Result<Arc<Mutex<Upload>>, &'static str> {
        let upload = self.uploads.lock().get(id).cloned().ok_or("filesUploadUnknown")?;
        if upload.lock().unwrap_or_else(|e| e.into_inner()).peer != self.peer {
            return Err("filesUploadUnknown");
        }
        Ok(upload)
    }

    fn write(&self, id: &str, offset: u64, data: &str) -> Result<Value, &'static str> {
        let upload = self.upload(id)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| "filesChunkInvalid")?;
        if bytes.is_empty() || bytes.len() > CHUNK {
            return Err("filesChunkInvalid");
        }
        let mut upload = upload.lock().unwrap_or_else(|e| e.into_inner());
        if offset != upload.written {
            return Err("filesOffsetInvalid");
        }
        if upload.written + bytes.len() as u64 > upload.size {
            return Err("filesTooLarge");
        }
        let result = upload.file.write_all(&bytes);
        if let Err(error) = result {
            let code = io_code(&error);
            drop(upload);
            self.drop_upload(id);
            return Err(code);
        }
        upload.written += bytes.len() as u64;
        upload.last = Instant::now();
        Ok(json!({ "written": upload.written }))
    }

    /// Removes an upload and its partial file.
    fn drop_upload(&self, id: &str) {
        if let Some(upload) = self.uploads.lock().remove(id) {
            Uploads::remove_partial(&upload);
        }
    }

    fn finish(&self, id: &str) -> Result<Value, &'static str> {
        let upload = self.upload(id)?;
        let (temp, dest, label, size) = {
            let upload = upload.lock().unwrap_or_else(|e| e.into_inner());
            if upload.written != upload.size {
                return Err("filesIncomplete");
            }
            upload.file.sync_all().map_err(|error| io_code(&error))?;
            (upload.temp.clone(), upload.dest.clone(), upload.label.clone(), upload.size)
        };
        let renamed = fs::rename(&temp, &dest);
        self.uploads.lock().remove(id);
        if let Err(error) = renamed {
            let _ = fs::remove_file(&temp);
            return Err(io_code(&error));
        }
        if let Some(log) = &self.log {
            let _ = log.append(&Entry::new(Some(&self.peer), Some(&self.name), "fileUpload", "ok").with_file(&label, Some(size)));
        }
        Ok(Value::Null)
    }

    fn cancel(&self, id: &str) -> Result<Value, &'static str> {
        // One that is already gone is as good as one that was cancelled.
        if self.upload(id).is_ok() {
            self.drop_upload(id);
        }
        Ok(Value::Null)
    }

    fn make_folder(&self, root: &str, path: &str, name: &str) -> Result<Value, &'static str> {
        if !valid_name(name) {
            return Err("filesPathInvalid");
        }
        let root = self.root(root)?;
        let folder = self.folder(&root, path)?;
        let created = folder.join(name);
        if self.is_protected(&created) {
            return Err("filesProtected");
        }
        fs::create_dir(&created).map_err(|error| io_code(&error))?;
        Ok(Value::Null)
    }

    /// Gives an entry a new name in its own folder. A name that exists is refused, and an entry never
    /// leaves its folder.
    fn rename(&self, root: &str, path: &str, name: &str) -> Result<Value, &'static str> {
        if !valid_name(name) || name.starts_with(".lw-upload-") {
            return Err("filesPathInvalid");
        }
        let root = self.root(root)?;
        let (parent, old) = self.child(&root, path)?;
        let from = parent.join(&old);
        let to = parent.join(name);
        if self.is_protected(&from) || self.is_protected(&to) {
            return Err("filesProtected");
        }
        let meta = fs::symlink_metadata(&from).map_err(|error| io_code(&error))?;
        if !(meta.is_file() || meta.is_dir() || meta.file_type().is_symlink()) {
            return Err("filesNotRegular");
        }
        if old == name {
            return Ok(Value::Null);
        }
        // `rename` would replace an entry, so look first. A name that only changes case on a
        // case-insensitive disk is the same entry and passes.
        if let Ok(existing) = fs::symlink_metadata(&to) {
            use std::os::unix::fs::MetadataExt;
            if (existing.dev(), existing.ino()) != (meta.dev(), meta.ino()) {
                return Err("filesExists");
            }
        }
        // A case-only change is the same entry, so it may replace. Any other rename refuses an entry
        // that appeared after the check above (RENAME_EXCL), so it never replaces a file.
        let same_entry = fs::symlink_metadata(&to).is_ok();
        rename_exclusive(&from, &to, same_entry).map_err(|error| io_code(&error))?;
        let size = meta.is_file().then(|| meta.len());
        let shown = format!("{path} -> {name}");
        self.record("fileRename", "ok", &root, &shown, size);
        Ok(Value::Null)
    }

    fn delete(&self, root: &str, path: &str, recursive: bool) -> Result<Value, &'static str> {
        let root = self.root(root)?;
        // The entry itself is not followed: a link is removed, never what it points to.
        let (parent, name) = self.child(&root, path)?;
        let target = parent.join(&name);
        if self.is_protected(&target) {
            return Err("filesProtected");
        }
        let meta = fs::symlink_metadata(&target).map_err(|error| io_code(&error))?;
        let size = meta.is_file().then(|| meta.len());
        if meta.is_dir() {
            if recursive {
                if self.protected.iter().any(|protected| protected.starts_with(&target)) {
                    return Err("filesProtected");
                }
                let mut seen = 0;
                count_entries(&target, &mut seen)?;
                fs::remove_dir_all(&target).map_err(|error| io_code(&error))?;
            } else {
                fs::remove_dir(&target).map_err(|error| io_code(&error))?;
            }
        } else if meta.is_file() || meta.file_type().is_symlink() {
            fs::remove_file(&target).map_err(|error| io_code(&error))?;
        } else {
            return Err("filesNotRegular");
        }
        self.record("fileDelete", "ok", &root, path, size);
        Ok(Value::Null)
    }
}

/// Counts what a recursive delete would remove, and stops at the cap. Links are not followed.
fn count_entries(folder: &Path, seen: &mut usize) -> Result<(), &'static str> {
    for item in fs::read_dir(folder).map_err(|error| io_code(&error))? {
        let item = item.map_err(|error| io_code(&error))?;
        *seen += 1;
        if *seen > DELETE_CAP {
            return Err("filesTooMany");
        }
        if item.file_type().is_ok_and(|kind| kind.is_dir()) {
            count_entries(&item.path(), seen)?;
        }
    }
    Ok(())
}

/// The paths that are never shared. Existing ones are made canonical.
pub fn protected_paths(config_dir: Option<&Path>, home: Option<&Path>) -> Vec<PathBuf> {
    let candidates = config_dir.map(Path::to_path_buf).into_iter().chain(home.map(|home| home.join(".ssh")));
    candidates
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect()
}

/// Checks the folders of a new grant: each must be an existing folder, not the whole disk,
/// not inside a protected path, and at most `MAX_FOLDERS` in all. A leading `~/` is the home folder.
/// `~/Lazy Worker` is created when it does not exist yet. Returns canonical paths without repeats.
pub fn check_folders(
    folders: &[String],
    home: Option<&Path>,
    protected: &[PathBuf],
) -> Result<Vec<String>, &'static str> {
    if folders.is_empty() || folders.len() > MAX_FOLDERS {
        return Err("filesFolderInvalid");
    }
    let mut checked: Vec<String> = Vec::new();
    for folder in folders {
        if folder.len() > MAX_PATH || folder.contains('\0') {
            return Err("filesFolderInvalid");
        }
        let path = match (folder.strip_prefix("~/"), home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ if folder.starts_with('/') => PathBuf::from(folder),
            _ => return Err("filesFolderInvalid"),
        };
        if home.is_some_and(|home| path == home.join("Lazy Worker")) && !path.exists() {
            fs::create_dir(&path).map_err(|_| "filesFolderInvalid")?;
        }
        let canonical = fs::canonicalize(&path).map_err(|_| "filesFolderInvalid")?;
        if !canonical.is_dir() || canonical == Path::new("/") || protected.iter().any(|p| canonical.starts_with(p)) {
            return Err("filesFolderInvalid");
        }
        let text = canonical.to_str().ok_or("filesFolderInvalid")?.to_owned();
        if !checked.contains(&text) {
            checked.push(text);
        }
    }
    Ok(checked)
}

/// Opens Privacy & Security at Full Disk Access. It changes nothing: the person turns the switch.
pub fn open_full_disk_access() -> Result<(), &'static str> {
    let opened = std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .stdin(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if opened { Ok(()) } else { Err("privacySettingsFailed") }
}

/// How long one location gets to answer. A privacy question that the person has not answered yet
/// holds the read open, so a slow answer ends as `pending` and the prompt stays on screen.
const TOUCH_WAIT: Duration = Duration::from_secs(10);

/// What reading one protected location showed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Touch {
    /// The folder can be read.
    Readable,
    /// macOS refused it, or the person answered "Don't Allow".
    Denied,
    /// Nobody answered in time.
    Pending,
    /// The folder could not be read for another reason.
    Failed,
}

/// One protected location and what reading it showed. `path` is shown with `~` for the home folder.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Checked {
    pub path: String,
    pub touch: Touch,
}

/// The places that macOS closes behind a privacy question the first time an app reads them.
fn home_protected(home: &Path) -> Vec<PathBuf> {
    ["Desktop", "Documents", "Downloads", "Library/Mobile Documents"].iter().map(|name| home.join(name)).collect()
}

/// The disks mounted under `/Volumes`, not counting the one that holds the system.
pub fn mounted_volumes() -> Vec<PathBuf> {
    let Ok(read) = fs::read_dir("/Volumes") else { return Vec::new() };
    let mut volumes: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|item| item.path())
        .filter(|path| fs::canonicalize(path).is_ok_and(|real| real != Path::new("/")))
        .collect();
    volumes.sort();
    volumes
}

/// The protected locations inside what a grant reaches, in the order they are asked. With chosen
/// folders these are the folders and the protected places they hold. For the home folder they are
/// Desktop, Documents, Downloads, and iCloud Drive, and the whole disk adds each mounted volume.
pub fn privacy_locations(scope: FilesScope, folders: &[String], home: Option<&Path>, volumes: &[PathBuf]) -> Vec<PathBuf> {
    let protected: Vec<PathBuf> = home.map(home_protected).unwrap_or_default();
    let mut found: Vec<PathBuf> = Vec::new();
    match scope {
        FilesScope::Home => found.extend(protected),
        FilesScope::Disk => {
            found.extend(protected);
            found.extend(volumes.iter().cloned());
        }
        FilesScope::Folders => {
            for folder in folders.iter().map(PathBuf::from) {
                for place in protected.iter().chain(volumes) {
                    if place.starts_with(&folder) && *place != folder && !found.contains(place) {
                        found.push(place.clone());
                    }
                }
                if !found.contains(&folder) {
                    found.push(folder);
                }
            }
        }
    }
    found
}

/// Reads one folder the way a remote request would, so macOS asks about it now. A folder that is
/// not there is left out (`None`).
fn touch_one(path: &Path) -> Option<Touch> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let owned = path.to_path_buf();
    std::thread::spawn(move || {
        let _ = sender.send(fs::read_dir(&owned).map(|mut read| drop(read.next())));
    });
    match receiver.recv_timeout(TOUCH_WAIT) {
        Ok(Ok(())) => Some(Touch::Readable),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(Err(error)) if io_code(&error) == "filesPrivacyDenied" => Some(Touch::Denied),
        Ok(Err(_)) => Some(Touch::Failed),
        Err(_) => Some(Touch::Pending),
    }
}

/// Reads each location one after the other, so each privacy question comes alone. It blocks for
/// the answers, so it belongs on a background thread.
pub fn touch_locations(locations: &[PathBuf], home: Option<&Path>) -> Vec<Checked> {
    locations
        .iter()
        .filter_map(|path| {
            touch_one(path).map(|touch| Checked { path: tilde(&path.to_string_lossy(), home), touch })
        })
        .collect()
}

/// Renames without replacing an existing entry, unless the destination is the same entry (a case change).
fn rename_exclusive(from: &Path, to: &Path, same_entry: bool) -> std::io::Result<()> {
    if same_entry {
        return fs::rename(from, to);
    }
    #[cfg(target_os = "macos")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from = CString::new(from.as_os_str().as_bytes())?;
        let to = CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both are valid NUL-terminated paths that live for the call.
        let status = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        if status == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if fs::symlink_metadata(to).is_ok() {
            return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
        }
        fs::rename(from, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{devices::ShellUntil, remote::testing::scratch};
    use std::os::unix::fs::symlink;

    fn grant(folders: &[&Path], mode: FilesMode) -> FilesGrant {
        FilesGrant {
            scope: FilesScope::Folders,
            folders: folders.iter().map(|path| path.to_str().unwrap().to_owned()).collect(),
            mode,
            until: ShellUntil::OFF,
        }
    }

    fn files(grant: FilesGrant, protected: Vec<PathBuf>) -> Files {
        Files {
            grant,
            peer: "0123456789abcdef0123456789abcdef".to_owned(),
            name: "Laptop".to_owned(),
            protected,
            uploads: Arc::new(Uploads::default()),
            log: None,
        }
    }

    /// A shared folder with a file, a sub folder, and a folder next to it that is not shared.
    struct Setup {
        base: PathBuf,
        shared: PathBuf,
        outside: PathBuf,
        files: Files,
        root: String,
    }

    fn setup(label: &str, mode: FilesMode) -> Setup {
        let base = fs::canonicalize(scratch(label)).unwrap();
        let shared = base.join("shared");
        let outside = base.join("outside");
        fs::create_dir_all(shared.join("docs")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(shared.join("a.txt"), b"hello").unwrap();
        fs::write(shared.join("docs/b.txt"), b"bee").unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        let files = files(grant(&[&shared], mode), Vec::new());
        let root = roots(&files.grant)[0].id.clone();
        Setup { base, shared, outside, files, root }
    }

    impl Setup {
        fn call(&self, value: Value) -> Result<Value, &'static str> {
            self.files.run(serde_json::from_value(value).unwrap())
        }
        fn list(&self, path: &str) -> Result<Value, &'static str> {
            self.call(json!({"command": "listFiles", "args": {"root": self.root, "path": path}}))
        }
        fn read(&self, path: &str, offset: u64, length: u32) -> Result<Value, &'static str> {
            self.call(json!({"command": "readFileChunk", "args": {"root": self.root, "path": path, "offset": offset, "length": length}}))
        }
        fn begin(&self, dir: &str, name: &str, size: u64) -> Result<Value, &'static str> {
            self.call(json!({"command": "beginUpload", "args": {"root": self.root, "path": dir, "name": name, "size": size}}))
        }
        fn put(&self, id: &str, offset: u64, bytes: &[u8]) -> Result<Value, &'static str> {
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            self.call(json!({"command": "writeFileChunk", "args": {"uploadId": id, "offset": offset, "data": data}}))
        }
        fn finish(&self, id: &str) -> Result<Value, &'static str> {
            self.call(json!({"command": "finishUpload", "args": {"uploadId": id}}))
        }
        fn delete(&self, path: &str, recursive: bool) -> Result<Value, &'static str> {
            self.call(json!({"command": "deleteEntry", "args": {"root": self.root, "path": path, "recursive": recursive}}))
        }
        fn rename(&self, path: &str, name: &str) -> Result<Value, &'static str> {
            self.call(json!({"command": "renameEntry", "args": {"root": self.root, "path": path, "name": name}}))
        }
        fn names(&self, path: &str) -> Vec<String> {
            self.list(path).unwrap()["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["name"].as_str().unwrap().to_owned())
                .collect()
        }
        fn done(self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn a_listing_sorts_folders_first_and_reads_sizes() {
        let s = setup("files-list", FilesMode::Read);
        assert_eq!(s.names(""), ["docs", "a.txt"]);
        let listing = s.list("").unwrap();
        assert_eq!(listing["entries"][1]["size"], 5);
        assert_eq!(s.names("docs"), ["b.txt"]);
        s.done();
    }

    #[test]
    fn a_path_must_stay_relative_and_clean() {
        let s = setup("files-paths", FilesMode::Read);
        let long_name = "a".repeat(256);
        let long_path = vec!["a"; 2100].join("/");
        for bad in [
            "..", "../outside", "docs/../../outside", "/etc", "/", "docs//b.txt", "docs/", "./a.txt",
            "a\0b", long_name.as_str(), long_path.as_str(),
        ] {
            assert_eq!(s.list(bad), Err("filesPathInvalid"), "{bad:?}");
        }
        // A name of exactly 255 bytes is fine as text, and is simply not there.
        assert_eq!(s.list(&"a".repeat(255)), Err("filesNotFound"));
        assert_eq!(s.list("nothing"), Err("filesNotFound"));
        assert_eq!(s.call(json!({"command": "listFiles", "args": {"root": "nope", "path": ""}})), Err("filesRootUnknown"));
        s.done();
    }

    #[test]
    fn a_link_that_leaves_the_folder_is_refused_and_one_inside_is_followed() {
        let s = setup("files-links", FilesMode::Read);
        symlink(&s.outside, s.shared.join("escape")).unwrap();
        symlink(s.outside.join("secret.txt"), s.shared.join("secret-link")).unwrap();
        symlink(s.shared.join("docs"), s.shared.join("inside")).unwrap();
        symlink(s.shared.join("nothing"), s.shared.join("broken")).unwrap();
        // The listing hides what leaves the folder, and what is broken.
        assert_eq!(s.names(""), ["docs", "inside", "a.txt"]);
        assert_eq!(s.list("escape"), Err("filesOutside"));
        assert_eq!(s.read("escape/secret.txt", 0, 10), Err("filesOutside"));
        assert_eq!(s.read("secret-link", 0, 10), Err("filesOutside"));
        // A link inside the folder works like the folder it points to.
        assert_eq!(s.names("inside"), ["b.txt"]);
        assert_eq!(s.read("inside/b.txt", 0, 10).unwrap()["size"], 3);
        s.done();
    }

    #[test]
    fn a_device_file_a_socket_and_a_pipe_are_not_read() {
        let s = setup("files-special", FilesMode::Read);
        let socket = s.shared.join("sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let fifo = std::ffi::CString::new(s.shared.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        symlink("/dev/null", s.shared.join("null")).unwrap();
        assert_eq!(s.read("sock", 0, 10), Err("filesNotRegular"));
        assert_eq!(s.read("pipe", 0, 10), Err("filesNotRegular"));
        assert_eq!(s.read("null", 0, 10), Err("filesOutside"));
        // None of them shows in a listing.
        assert_eq!(s.names(""), ["docs", "a.txt"]);
        assert_eq!(s.read("docs", 0, 10), Err("filesIsFolder"));
        s.done();
    }

    #[test]
    fn the_data_of_this_app_and_the_ssh_keys_are_never_shared_even_with_the_whole_disk() {
        let base = fs::canonicalize(scratch("files-protected")).unwrap();
        let config = base.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("id_ed25519"), b"key").unwrap();
        fs::write(base.join("open.txt"), b"x").unwrap();
        let mut disk = grant(&[], FilesMode::ReadWrite);
        disk.scope = FilesScope::Disk;
        let files = files(disk, vec![config.clone()]);
        let call = |value: Value| files.run(serde_json::from_value(value).unwrap());
        let path = |p: &Path| p.strip_prefix("/").unwrap().to_str().unwrap().to_owned();
        let read = |p: &Path| call(json!({"command": "readFileChunk", "args": {"root": "disk", "path": path(p), "offset": 0, "length": 8}}));
        assert!(read(&base.join("open.txt")).is_ok());
        assert_eq!(read(&config.join("id_ed25519")), Err("filesProtected"));
        let listing = call(json!({"command": "listFiles", "args": {"root": "disk", "path": path(&base)}})).unwrap();
        let names: Vec<&str> = listing["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["open.txt"]);
        let delete = |p: &Path, recursive: bool| call(json!({"command": "deleteEntry", "args": {"root": "disk", "path": path(p), "recursive": recursive}}));
        assert_eq!(delete(&config, true), Err("filesProtected"));
        assert_eq!(delete(&base, true), Err("filesProtected"));
        assert_eq!(
            call(json!({"command": "beginUpload", "args": {"root": "disk", "path": path(&config), "name": "authorized_keys", "size": 1}})),
            Err("filesProtected")
        );
        assert!(config.join("id_ed25519").exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_rename_stays_in_its_folder_and_never_replaces_an_entry() {
        let s = setup("files-rename", FilesMode::ReadWrite);
        fs::write(s.shared.join("docs/b.txt"), b"b").unwrap();
        assert_eq!(s.rename("a.txt", "c.txt"), Ok(Value::Null));
        assert!(!s.shared.join("a.txt").exists() && s.shared.join("c.txt").exists());
        assert_eq!(s.rename("docs", "papers"), Ok(Value::Null));
        assert_eq!(s.rename("papers/b.txt", "papers"), Ok(Value::Null));
        // An existing name, a path, a dot name, and a name of ours for partial uploads are refused.
        fs::write(s.shared.join("d.txt"), b"d").unwrap();
        assert_eq!(s.rename("c.txt", "d.txt"), Err("filesExists"));
        assert_eq!(fs::read(s.shared.join("d.txt")).unwrap(), b"d");
        for bad in ["", ".", "..", "x/y", "../x", ".lw-upload-1.part"] {
            assert_eq!(s.rename("c.txt", bad), Err("filesPathInvalid"), "{bad}");
        }
        assert_eq!(s.rename("../c.txt", "e"), Err("filesPathInvalid"));
        assert_eq!(s.rename("gone.txt", "e"), Err("filesNotFound"));
        assert_eq!(s.rename("", "e"), Err("filesPathInvalid"));
        s.done();
    }

    #[test]
    fn a_read_only_grant_refuses_every_write() {
        let s = setup("files-readonly", FilesMode::Read);
        let writes = [
            json!({"command": "beginUpload", "args": {"root": s.root, "path": "", "name": "n", "size": 1}}),
            json!({"command": "writeFileChunk", "args": {"uploadId": "x", "offset": 0, "data": "AA=="}}),
            json!({"command": "finishUpload", "args": {"uploadId": "x"}}),
            json!({"command": "cancelUpload", "args": {"uploadId": "x"}}),
            json!({"command": "makeFolder", "args": {"root": s.root, "path": "", "name": "n"}}),
            json!({"command": "deleteEntry", "args": {"root": s.root, "path": "a.txt", "recursive": false}}),
            json!({"command": "renameEntry", "args": {"root": s.root, "path": "a.txt", "name": "n"}}),
        ];
        for write in writes {
            assert_eq!(s.call(write.clone()), Err("notPermitted"), "{write}");
        }
        assert!(s.shared.join("a.txt").exists() && !s.shared.join("n").exists());
        assert!(s.list("").is_ok());
        s.done();
    }

    #[test]
    fn an_expired_grant_refuses_every_request() {
        let mut s = setup("files-expired", FilesMode::ReadWrite);
        s.files.grant.until = ShellUntil::At(now() - 1);
        assert_eq!(s.list(""), Err("notPermitted"));
        assert_eq!(s.read("a.txt", 0, 1), Err("notPermitted"));
        assert_eq!(s.begin("", "x", 1), Err("notPermitted"));
        assert_eq!(s.delete("a.txt", false), Err("notPermitted"));
        assert!(s.shared.join("a.txt").exists());
        s.done();
    }

    /// A few MiB that no chunk size divides.
    fn random_bytes(length: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; length];
        OsRng.fill_bytes(&mut bytes);
        bytes
    }

    #[test]
    fn a_file_round_trips_in_chunks_through_a_temporary_name() {
        let s = setup("files-round", FilesMode::ReadWrite);
        let data = random_bytes(3 * 1024 * 1024 + 12_345);
        let id = s.begin("docs", "big.bin", data.len() as u64).unwrap()["uploadId"].as_str().unwrap().to_owned();
        let mut offset = 0;
        for chunk in data.chunks(CHUNK) {
            s.put(&id, offset as u64, chunk).unwrap();
            offset += chunk.len();
            // The final name does not exist until the upload finishes.
            assert!(!s.shared.join("docs/big.bin").exists());
        }
        assert_eq!(s.names("docs").len(), 2, "the partial file is a hidden temp name");
        s.finish(&id).unwrap();
        assert_eq!(fs::read(s.shared.join("docs/big.bin")).unwrap(), data);
        assert!(!fs::read_dir(s.shared.join("docs")).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".part")));
        // Download it back by offset and length.
        let mut back = Vec::new();
        loop {
            let answer = s.read("docs/big.bin", back.len() as u64, CHUNK as u32).unwrap();
            back.extend(base64::engine::general_purpose::STANDARD.decode(answer["data"].as_str().unwrap()).unwrap());
            assert_eq!(answer["size"], data.len());
            if answer["eof"] == true {
                break;
            }
        }
        assert_eq!(back, data);
        assert_eq!(s.files.uploads.count(), 0);
        s.done();
    }

    #[test]
    fn chunks_out_of_order_too_big_or_past_the_size_are_refused() {
        let s = setup("files-offsets", FilesMode::ReadWrite);
        let id = s.begin("", "x.bin", 10).unwrap()["uploadId"].as_str().unwrap().to_owned();
        assert_eq!(s.put(&id, 5, b"abc"), Err("filesOffsetInvalid"));
        s.put(&id, 0, b"abcd").unwrap();
        assert_eq!(s.put(&id, 0, b"abcd"), Err("filesOffsetInvalid"));
        assert_eq!(s.put(&id, 8, b"abcd"), Err("filesOffsetInvalid"));
        assert_eq!(s.put(&id, 4, b"abcdefg"), Err("filesTooLarge"));
        assert_eq!(s.put(&id, 4, b""), Err("filesChunkInvalid"));
        assert_eq!(s.call(json!({"command": "writeFileChunk", "args": {"uploadId": id, "offset": 4, "data": "***"}})), Err("filesChunkInvalid"));
        assert_eq!(s.finish(&id), Err("filesIncomplete"));
        s.put(&id, 4, b"efghij").unwrap();
        s.finish(&id).unwrap();
        assert_eq!(fs::read(s.shared.join("x.bin")).unwrap(), b"abcdefghij");
        // A chunk over the cap is refused before it touches the file.
        let id = s.begin("", "y.bin", MAX_FILE_SIZE).unwrap()["uploadId"].as_str().unwrap().to_owned();
        assert_eq!(s.put(&id, 0, &vec![0u8; CHUNK + 1]), Err("filesChunkInvalid"));
        assert_eq!(s.read("a.txt", 0, CHUNK as u32 + 1), Err("filesChunkInvalid"));
        assert_eq!(s.read("a.txt", 0, 0), Err("filesChunkInvalid"));
        // The size cap, and an upload id that is not there.
        assert_eq!(s.begin("", "z.bin", MAX_FILE_SIZE + 1), Err("filesTooLarge"));
        assert_eq!(s.put("nope", 0, b"a"), Err("filesUploadUnknown"));
        s.done();
    }

    #[test]
    fn an_upload_never_replaces_a_folder_but_replaces_a_file_whole() {
        let s = setup("files-replace", FilesMode::ReadWrite);
        assert_eq!(s.begin("", "docs", 1), Err("filesIsFolder"));
        assert_eq!(s.begin("", "bad/name", 1), Err("filesPathInvalid"));
        assert_eq!(s.begin("", "..", 1), Err("filesPathInvalid"));
        assert_eq!(s.begin("", &"n".repeat(256), 1), Err("filesPathInvalid"));
        assert_eq!(s.begin("a.txt", "x", 1), Err("filesNotFolder"));
        let id = s.begin("", "a.txt", 3).unwrap()["uploadId"].as_str().unwrap().to_owned();
        s.put(&id, 0, b"new").unwrap();
        // The old content stays until the rename.
        assert_eq!(fs::read(s.shared.join("a.txt")).unwrap(), b"hello");
        s.finish(&id).unwrap();
        assert_eq!(fs::read(s.shared.join("a.txt")).unwrap(), b"new");
        s.done();
    }

    #[test]
    fn a_cancelled_idle_or_revoked_upload_leaves_no_partial_file() {
        let s = setup("files-partial", FilesMode::ReadWrite);
        let partials = || fs::read_dir(&s.shared).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".part")).count();
        let id = s.begin("", "a.bin", 100).unwrap()["uploadId"].as_str().unwrap().to_owned();
        s.put(&id, 0, b"abc").unwrap();
        assert_eq!(partials(), 1);
        s.call(json!({"command": "cancelUpload", "args": {"uploadId": id}})).unwrap();
        assert_eq!(partials(), 0);
        assert_eq!(s.put(&id, 3, b"d"), Err("filesUploadUnknown"));
        // Revoking the permission mid-upload ends the upload.
        let id = s.begin("", "b.bin", 100).unwrap()["uploadId"].as_str().unwrap().to_owned();
        s.put(&id, 0, b"abc").unwrap();
        assert_eq!(s.files.uploads.cancel_peer(&s.files.peer), 1);
        assert_eq!(partials(), 0);
        assert_eq!(s.put(&id, 3, b"d"), Err("filesUploadUnknown"));
        // An idle upload is dropped, and so is one whose device is not allowed any more.
        let first = s.begin("", "c.bin", 100).unwrap()["uploadId"].as_str().unwrap().to_owned();
        s.files.uploads.sweep(Instant::now(), |_| true);
        assert_eq!(partials(), 1);
        s.files.uploads.sweep(Instant::now() + UPLOAD_IDLE, |_| true);
        assert_eq!(partials(), 0);
        assert_eq!(s.put(&first, 0, b"a"), Err("filesUploadUnknown"));
        s.begin("", "d.bin", 100).unwrap();
        s.files.uploads.sweep(Instant::now(), |_| false);
        assert_eq!(partials(), 0);
        assert!(!s.shared.join("a.bin").exists() && !s.shared.join("b.bin").exists());
        s.done();
    }

    #[test]
    fn one_device_holds_a_few_uploads_and_cannot_touch_those_of_another() {
        let s = setup("files-many", FilesMode::ReadWrite);
        let ids: Vec<String> = (0..MAX_UPLOADS_PER_PEER)
            .map(|n| s.begin("", &format!("f{n}"), 5).unwrap()["uploadId"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(s.begin("", "more", 5), Err("filesTooManyUploads"));
        let mut other = setup("files-many-other", FilesMode::ReadWrite);
        other.files.uploads = s.files.uploads.clone();
        other.files.peer = "fedcba9876543210fedcba9876543210".to_owned();
        assert_eq!(other.put(&ids[0], 0, b"a"), Err("filesUploadUnknown"));
        assert_eq!(other.finish(&ids[0]), Err("filesUploadUnknown"));
        assert_eq!(s.files.uploads.cancel_peer(&other.files.peer), 0);
        assert_eq!(s.files.uploads.count(), MAX_UPLOADS_PER_PEER);
        s.files.uploads.cancel_peer(&s.files.peer);
        other.done();
        s.done();
    }

    #[test]
    fn folders_are_made_and_deleted_with_care() {
        let s = setup("files-delete", FilesMode::ReadWrite);
        s.call(json!({"command": "makeFolder", "args": {"root": s.root, "path": "docs", "name": "new"}})).unwrap();
        assert!(s.shared.join("docs/new").is_dir());
        assert_eq!(
            s.call(json!({"command": "makeFolder", "args": {"root": s.root, "path": "docs", "name": "new"}})),
            Err("filesExists")
        );
        assert_eq!(
            s.call(json!({"command": "makeFolder", "args": {"root": s.root, "path": "docs", "name": "a/b"}})),
            Err("filesPathInvalid")
        );
        // A folder that holds something needs the recursive choice. The shared folder itself cannot go.
        assert_eq!(s.delete("docs", false), Err("filesNotEmpty"));
        assert_eq!(s.delete("", true), Err("filesPathInvalid"));
        s.delete("docs/new", false).unwrap();
        s.delete("docs", true).unwrap();
        s.delete("a.txt", false).unwrap();
        assert_eq!(s.names(""), Vec::<String>::new());
        assert_eq!(s.delete("a.txt", false), Err("filesNotFound"));
        // A link is removed, and what it points to stays.
        symlink(s.outside.join("secret.txt"), s.shared.join("link")).unwrap();
        s.delete("link", false).unwrap();
        assert!(!s.shared.join("link").exists() && s.outside.join("secret.txt").exists());
        s.done();
    }

    #[test]
    fn a_recursive_delete_has_a_cap() {
        let s = setup("files-cap", FilesMode::ReadWrite);
        let big = s.shared.join("big");
        fs::create_dir(&big).unwrap();
        let mut seen = 0;
        assert!(count_entries(&s.shared, &mut seen).is_ok());
        for n in 0..=DELETE_CAP {
            File::create(big.join(n.to_string())).unwrap();
        }
        assert_eq!(s.delete("big", true), Err("filesTooMany"));
        assert!(big.join("0").exists());
        s.done();
    }

    #[test]
    fn folders_of_a_grant_are_checked_and_the_default_one_is_created() {
        let base = fs::canonicalize(scratch("files-check")).unwrap();
        let home = base.join("home");
        fs::create_dir_all(home.join("Documents")).unwrap();
        fs::create_dir_all(home.join(".ssh")).unwrap();
        let protected = vec![home.join(".ssh")];
        let checked = check_folders(
            &["~/Lazy Worker".to_owned(), "~/Documents".to_owned(), home.join("Documents").to_str().unwrap().to_owned()],
            Some(&home),
            &protected,
        )
        .unwrap();
        assert_eq!(checked, [home.join("Lazy Worker").to_str().unwrap(), home.join("Documents").to_str().unwrap()]);
        assert!(home.join("Lazy Worker").is_dir());
        for bad in [vec![], vec!["/".to_owned()], vec!["relative".to_owned()], vec!["~/Nope".to_owned()], vec![home.join(".ssh").to_str().unwrap().to_owned()], vec!["/a\0b".to_owned()]] {
            assert_eq!(check_folders(&bad, Some(&home), &protected), Err("filesFolderInvalid"), "{bad:?}");
        }
        assert_eq!(check_folders(&vec!["~/Documents".to_owned(); MAX_FOLDERS + 1], Some(&home), &protected), Err("filesFolderInvalid"));
        assert_eq!(tilde(home.join("Documents").to_str().unwrap(), Some(&home)), "~/Documents");
        assert_eq!(tilde("/etc", Some(&home)), "/etc");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_share_view_names_the_folders_and_hides_an_ended_grant() {
        let s = setup("files-view", FilesMode::Read);
        let view = share_view(Some(&s.files.grant), None);
        assert_eq!(view["on"], true);
        assert_eq!(view["mode"], "read");
        assert_eq!(view["roots"][0]["name"], "shared");
        let mut ended = s.files.grant.clone();
        ended.until = ShellUntil::At(1);
        assert_eq!(share_view(Some(&ended), None), json!({"on": false}));
        assert_eq!(share_view(None, None), json!({"on": false}));
        s.done();
    }
    #[test]
    fn stored_grants_keep_their_scope_and_home_is_new() {
        for (word, scope) in [("folders", FilesScope::Folders), ("disk", FilesScope::Disk), ("home", FilesScope::Home)] {
            let stored = json!({"scope": word, "folders": [], "mode": "read", "until": "untilOff"});
            let grant: FilesGrant = serde_json::from_value(stored).unwrap();
            assert_eq!(grant.scope, scope);
            assert_eq!(serde_json::to_value(scope).unwrap(), word);
        }
        // The old whole-disk grant still reaches `/`, and a home grant is one root without a name.
        let old = FilesGrant { scope: FilesScope::Disk, folders: vec![], mode: FilesMode::Read, until: ShellUntil::OFF };
        assert_eq!(roots(&old)[0].path, Path::new("/"));
        let base = fs::canonicalize(scratch("files-home-root")).unwrap();
        let home = FilesGrant { scope: FilesScope::Home, folders: vec![base.to_str().unwrap().to_owned()], mode: FilesMode::Read, until: ShellUntil::OFF };
        let list = roots(&home);
        assert_eq!((list.len(), list[0].id.as_str(), list[0].name.as_str()), (1, "home", ""));
        assert_eq!(list[0].path, base);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_protected_locations_follow_the_scope() {
        let home = Path::new("/Users/alex");
        let volumes = vec![PathBuf::from("/Volumes/Backup"), PathBuf::from("/Volumes/Nas")];
        let ask = |scope, folders: &[&str]| {
            let folders: Vec<String> = folders.iter().map(|folder| (*folder).to_owned()).collect();
            privacy_locations(scope, &folders, Some(home), &volumes)
        };
        let places = ["Desktop", "Documents", "Downloads", "Library/Mobile Documents"].map(|name| home.join(name));
        assert_eq!(ask(FilesScope::Home, &["/Users/alex"]), places);
        let mut disk = places.to_vec();
        disk.extend(volumes.clone());
        assert_eq!(ask(FilesScope::Disk, &[]), disk);
        // Chosen folders: the protected places inside them come first, then the folder itself.
        assert_eq!(ask(FilesScope::Folders, &["/Users/alex"]).len(), 5);
        assert_eq!(ask(FilesScope::Folders, &["/Users/alex/Documents/Work"]), [PathBuf::from("/Users/alex/Documents/Work")]);
        assert_eq!(
            ask(FilesScope::Folders, &["/Volumes", "/Volumes"]),
            [volumes[0].clone(), volumes[1].clone(), PathBuf::from("/Volumes")]
        );
        assert_eq!(ask(FilesScope::Folders, &["/opt/tools"]), [PathBuf::from("/opt/tools")]);
        assert!(privacy_locations(FilesScope::Home, &[], None, &[]).is_empty());
    }

    #[test]
    fn a_folder_that_macos_closes_maps_to_the_privacy_code() {
        // EPERM is the macOS privacy answer; EACCES is an ordinary permission.
        let privacy = std::io::Error::from_raw_os_error(libc::EPERM);
        assert_eq!(privacy.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(io_code(&privacy), "filesPrivacyDenied");
        assert_eq!(io_code(&std::io::Error::from_raw_os_error(libc::EACCES)), "filesDenied");
        assert_eq!(io_code(&std::io::ErrorKind::NotFound.into()), "filesNotFound");
    }

    #[test]
    fn touching_reports_readable_missing_and_skips_nothing_else() {
        let base = fs::canonicalize(scratch("files-touch")).unwrap();
        let checked = touch_locations(&[base.clone(), base.join("missing")], Some(&base));
        assert_eq!(checked, [Checked { path: "~".to_owned(), touch: Touch::Readable }]);
        let _ = fs::remove_dir_all(&base);
    }
}
