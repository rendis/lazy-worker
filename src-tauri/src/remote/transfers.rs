//! The transfers of files between this device and a device that it controls.
//!
//! The file data never reaches the webview. A transfer is a job in a queue, and one worker runs
//! the jobs one after the other, a chunk per request, with the same requests that any paired
//! device sends. The webview reads the progress and may stop a job. A device that ends the
//! permission, or a link that fails, ends the job with the error word it got.

use super::files::CHUNK;
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
};

/// The jobs that the list holds. The oldest finished ones go first.
const MAX_JOBS: usize = 50;

/// How requests reach a device that this one controls.
pub trait Link: Send + Sync + 'static {
    fn call(&self, device: &str, request: &Value) -> Result<Value, &'static str>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
    Stopped,
}

enum Spec {
    /// Sends the local file into `dir` of the shared folder `root`.
    Upload { root: String, dir: String, local: PathBuf },
    /// Saves `path` of the shared folder `root` as the local file `local`.
    Download { root: String, path: String, local: PathBuf },
}

/// One transfer, as the webview reads it. `error` is an error word, never text from outside.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: u64,
    pub device: String,
    pub kind: Kind,
    pub name: String,
    pub total: u64,
    pub done: u64,
    pub state: State,
    pub error: Option<&'static str>,
    #[serde(skip)]
    spec: Option<Spec>,
    #[serde(skip)]
    stop: Arc<AtomicBool>,
}

struct Shared {
    jobs: Mutex<(u64, Vec<Job>)>,
    wake: Condvar,
    link: Arc<dyn Link>,
}

pub struct Transfers {
    shared: Arc<Shared>,
}

/// The link of the app: a request goes to the device through its open session.
pub struct AppLink(pub tauri::AppHandle);

impl Link for AppLink {
    fn call(&self, device: &str, request: &Value) -> Result<Value, &'static str> {
        use tauri::Manager;
        self.0.state::<super::service::Service>().forward(device, request)
    }
}

/// The last part of a path as text for people. Nothing else of the local path leaves this device.
fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().chars().filter(|c| !c.is_control()).collect())
}

/// A name that is free in a folder: `name`, then `name 2`, `name 3`, and so on before the extension.
fn unique(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let path = Path::new(name);
    let stem = path.file_stem().map_or_else(|| name.to_owned(), |stem| stem.to_string_lossy().into_owned());
    let extension = path.extension().map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|n| folder.join(format!("{stem} {n}{extension}")))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .unwrap_or(first)
}

impl Transfers {
    pub fn new(link: impl Link) -> Self {
        let shared = Arc::new(Shared {
            jobs: Mutex::new((1, Vec::new())),
            wake: Condvar::new(),
            link: Arc::new(link),
        });
        let worker = shared.clone();
        let _ = std::thread::Builder::new()
            .name("transfers".into())
            .spawn(move || worker.run());
        Self { shared }
    }

    /// Queues the files of the list, one job each, into `dir` of the shared folder.
    pub fn queue_uploads(&self, device: &str, root: &str, dir: &str, files: &[String]) -> Result<(), &'static str> {
        if files.is_empty() || files.len() > MAX_JOBS {
            return Err("filesTooMany");
        }
        let mut specs = Vec::new();
        for file in files {
            let local = PathBuf::from(file);
            if !local.is_absolute() || file.contains('\0') {
                return Err("filesPathInvalid");
            }
            let meta = fs::metadata(&local).map_err(|_| "filesNotFound")?;
            if !meta.is_file() {
                return Err("filesNotRegular");
            }
            specs.push((base_name(file), meta.len(), Spec::Upload { root: root.to_owned(), dir: dir.to_owned(), local }));
        }
        for (name, total, spec) in specs {
            self.push(device, Kind::Upload, name, total, spec);
        }
        Ok(())
    }

    /// Queues downloads. One path is saved as the file `destination`. Several are saved by their
    /// names into the folder `destination`, and a name that is taken gets a number.
    pub fn queue_downloads(&self, device: &str, root: &str, paths: &[String], destination: &str) -> Result<(), &'static str> {
        if paths.is_empty() || paths.len() > MAX_JOBS {
            return Err("filesTooMany");
        }
        let target = PathBuf::from(destination);
        if !target.is_absolute() || destination.contains('\0') {
            return Err("filesPathInvalid");
        }
        let mut specs = Vec::new();
        for (index, path) in paths.iter().enumerate() {
            let name = base_name(path);
            if name.is_empty() {
                return Err("filesPathInvalid");
            }
            let local = if paths.len() == 1 {
                target.clone()
            } else {
                if !target.is_dir() {
                    return Err("filesNotFolder");
                }
                // Earlier names of this batch are not on disk yet, so they are counted by hand.
                let mut local = unique(&target, &name);
                while specs.iter().any(|(_, _, spec): &(String, u64, Spec)| matches!(spec, Spec::Download { local: taken, .. } if *taken == local)) {
                    local = unique(&target, &format!("{name} {}", index + 2));
                }
                local
            };
            specs.push((name, 0, Spec::Download { root: root.to_owned(), path: path.clone(), local }));
        }
        for (name, total, spec) in specs {
            self.push(device, Kind::Download, name, total, spec);
        }
        Ok(())
    }

    fn push(&self, device: &str, kind: Kind, name: String, total: u64, spec: Spec) {
        let mut jobs = self.shared.jobs.lock().unwrap_or_else(|error| error.into_inner());
        let id = jobs.0;
        jobs.0 += 1;
        jobs.1.push(Job {
            id,
            device: device.to_owned(),
            kind,
            name,
            total,
            done: 0,
            state: State::Queued,
            error: None,
            spec: Some(spec),
            stop: Arc::new(AtomicBool::new(false)),
        });
        // The list stays small: the oldest finished jobs go.
        while jobs.1.len() > MAX_JOBS {
            let Some(index) = jobs.1.iter().position(|job| !matches!(job.state, State::Queued | State::Running)) else { break };
            jobs.1.remove(index);
        }
        drop(jobs);
        self.shared.wake.notify_one();
    }

    /// The jobs, oldest first, as the webview reads them.
    pub fn list(&self) -> Value {
        let jobs = self.shared.jobs.lock().unwrap_or_else(|error| error.into_inner());
        serde_json::to_value(&jobs.1).unwrap_or(Value::Null)
    }

    /// Stops a job. One that has not begun never begins, and one that runs stops after its chunk.
    pub fn stop(&self, id: u64) {
        let mut jobs = self.shared.jobs.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(job) = jobs.1.iter_mut().find(|job| job.id == id) {
            job.stop.store(true, Ordering::Release);
            if job.state == State::Queued {
                job.state = State::Stopped;
                job.spec = None;
            }
        }
    }

    /// Stops every job that has not ended, for example when a device is forgotten.
    pub fn stop_device(&self, device: &str) {
        let ids: Vec<u64> = {
            let jobs = self.shared.jobs.lock().unwrap_or_else(|error| error.into_inner());
            jobs.1.iter().filter(|job| job.device == device).map(|job| job.id).collect()
        };
        for id in ids {
            self.stop(id);
        }
    }

    /// Removes the jobs that have ended.
    pub fn clear(&self) {
        let mut jobs = self.shared.jobs.lock().unwrap_or_else(|error| error.into_inner());
        jobs.1.retain(|job| matches!(job.state, State::Queued | State::Running));
    }
}

impl Shared {
    /// Waits for a queued job and runs it, for as long as the app lives.
    fn run(&self) {
        loop {
            let next = {
                let mut jobs = self.jobs.lock().unwrap_or_else(|error| error.into_inner());
                loop {
                    let found = jobs.1.iter_mut().find(|job| job.state == State::Queued);
                    if let Some(job) = found {
                        job.state = State::Running;
                        break (job.id, job.device.clone(), job.spec.take(), job.stop.clone());
                    }
                    jobs = self.wake.wait(jobs).unwrap_or_else(|error| error.into_inner());
                }
            };
            let (id, device, spec, stop) = next;
            let outcome = match spec {
                Some(Spec::Upload { root, dir, local }) => self.upload(id, &device, &root, &dir, &local, &stop),
                Some(Spec::Download { root, path, local }) => self.download(id, &device, &root, &path, &local, &stop),
                None => Err("filesIo"),
            };
            let mut jobs = self.jobs.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(job) = jobs.1.iter_mut().find(|job| job.id == id) {
                match outcome {
                    Ok(()) => job.state = State::Done,
                    Err("stopped") => job.state = State::Stopped,
                    Err(code) => {
                        job.state = State::Failed;
                        job.error = Some(code);
                    }
                }
            }
        }
    }

    fn progress(&self, id: u64, total: Option<u64>, done: u64) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(job) = jobs.1.iter_mut().find(|job| job.id == id) {
            if let Some(total) = total {
                job.total = total;
            }
            job.done = done;
        }
    }

    fn ask(&self, device: &str, command: &str, args: Value) -> Result<Value, &'static str> {
        self.link.call(device, &json!({ "command": command, "args": args }))
    }

    fn upload(&self, id: u64, device: &str, root: &str, dir: &str, local: &Path, stop: &AtomicBool) -> Result<(), &'static str> {
        let mut file = File::open(local).map_err(|_| "filesNotFound")?;
        let meta = file.metadata().map_err(|_| "filesIo")?;
        if !meta.is_file() {
            return Err("filesNotRegular");
        }
        let size = meta.len();
        self.progress(id, Some(size), 0);
        if stop.load(Ordering::Acquire) {
            return Err("stopped");
        }
        let name = base_name(&local.to_string_lossy());
        let begun = self.ask(device, "beginUpload", json!({ "root": root, "path": dir, "name": name, "size": size }))?;
        let upload_id = begun
            .get("uploadId")
            .and_then(Value::as_str)
            .filter(|text| text.len() <= 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or("remoteFailed")?
            .to_owned();
        let cancel = |code: &'static str| {
            let _ = self.ask(device, "cancelUpload", json!({ "uploadId": upload_id }));
            code
        };
        let mut offset = 0u64;
        let mut buffer = vec![0u8; CHUNK];
        while offset < size {
            if stop.load(Ordering::Acquire) {
                return Err(cancel("stopped"));
            }
            let want = usize::try_from((size - offset).min(CHUNK as u64)).unwrap_or(CHUNK);
            file.read_exact(&mut buffer[..want]).map_err(|_| cancel("filesChanged"))?;
            let data = base64::engine::general_purpose::STANDARD.encode(&buffer[..want]);
            self.ask(device, "writeFileChunk", json!({ "uploadId": upload_id, "offset": offset, "data": data }))
                .map_err(|code| cancel(code))?;
            offset += want as u64;
            self.progress(id, None, offset);
        }
        if stop.load(Ordering::Acquire) {
            return Err(cancel("stopped"));
        }
        self.ask(device, "finishUpload", json!({ "uploadId": upload_id })).map_err(|code| cancel(code))?;
        Ok(())
    }

    fn download(&self, id: u64, device: &str, root: &str, path: &str, local: &Path, stop: &AtomicBool) -> Result<(), &'static str> {
        let info = self.ask(device, "statFile", json!({ "root": root, "path": path }))?;
        if info.get("kind").and_then(Value::as_str) != Some("file") {
            return Err("filesIsFolder");
        }
        let size = info.get("size").and_then(Value::as_u64).ok_or("remoteFailed")?;
        self.progress(id, Some(size), 0);
        let folder = local.parent().ok_or("filesPathInvalid")?;
        let part = folder.join(format!(".lw-download-{id}.part"));
        let mut out = OpenOptions::new().write(true).create_new(true).open(&part).map_err(|_| "filesDenied")?;
        let outcome = self.fetch(id, device, root, path, size, &mut out, stop);
        let outcome = outcome.and_then(|()| {
            out.sync_all().map_err(|_| "filesIo")?;
            fs::rename(&part, local).map_err(|_| "filesDenied")
        });
        if outcome.is_err() {
            let _ = fs::remove_file(&part);
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    fn fetch(&self, id: u64, device: &str, root: &str, path: &str, size: u64, out: &mut File, stop: &AtomicBool) -> Result<(), &'static str> {
        let mut offset = 0u64;
        loop {
            if stop.load(Ordering::Acquire) {
                return Err("stopped");
            }
            let answer = self.ask(device, "readFileChunk", json!({ "root": root, "path": path, "offset": offset, "length": CHUNK }))?;
            // The answer comes from another device, so it is checked before any byte is kept.
            if answer.get("size").and_then(Value::as_u64) != Some(size) {
                return Err("filesChanged");
            }
            let text = answer.get("data").and_then(Value::as_str).ok_or("remoteFailed")?;
            let data = base64::engine::general_purpose::STANDARD.decode(text).map_err(|_| "remoteFailed")?;
            if data.len() > CHUNK || offset + data.len() as u64 > size {
                return Err("remoteFailed");
            }
            out.write_all(&data).map_err(|_| "filesDiskFull")?;
            offset += data.len() as u64;
            self.progress(id, None, offset);
            let eof = answer.get("eof").and_then(Value::as_bool).ok_or("remoteFailed")?;
            if eof {
                return if offset == size { Ok(()) } else { Err("filesChanged") };
            }
            if data.is_empty() {
                return Err("remoteFailed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        devices::{FilesGrant, FilesMode, FilesScope, ShellUntil},
        remote::{
            files::{Files, Uploads},
            testing::scratch,
        },
    };
    use rand_core::{OsRng, RngCore};
    use std::time::{Duration, Instant};

    /// A link that hands each request to the files of a target device in this process.
    struct Loopback {
        files: Mutex<Files>,
        calls: Mutex<Vec<String>>,
        /// After this many chunk calls the grant ends, as a revoke would.
        revoke_after: Mutex<Option<usize>>,
    }

    impl Link for Arc<Loopback> {
        fn call(&self, _device: &str, request: &Value) -> Result<Value, &'static str> {
            let mut files = self.files.lock().unwrap();
            let name = request["command"].as_str().unwrap().to_owned();
            self.calls.lock().unwrap().push(name.clone());
            if name == "writeFileChunk" || name == "readFileChunk" {
                let mut after = self.revoke_after.lock().unwrap();
                if let Some(left) = after.as_mut() {
                    if *left == 0 {
                        files.grant.until = ShellUntil::At(1);
                        files.uploads.cancel_peer(&files.peer);
                    } else {
                        *left -= 1;
                    }
                }
            }
            files.run(serde_json::from_value(request.clone()).map_err(|_| "badRequest")?)
        }
    }

    struct Setup {
        base: PathBuf,
        shared: PathBuf,
        local: PathBuf,
        root: String,
        loopback: Arc<Loopback>,
        transfers: Transfers,
    }

    fn setup(label: &str) -> Setup {
        let base = fs::canonicalize(scratch(label)).unwrap();
        let (shared, local) = (base.join("shared"), base.join("local"));
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&local).unwrap();
        let grant = FilesGrant {
            scope: FilesScope::Folders,
            folders: vec![shared.to_str().unwrap().to_owned()],
            mode: FilesMode::ReadWrite,
            until: ShellUntil::OFF,
        };
        let root = crate::remote::files::roots(&grant)[0].id.clone();
        let loopback = Arc::new(Loopback {
            files: Mutex::new(Files {
                grant,
                peer: "0123456789abcdef0123456789abcdef".into(),
                name: "Laptop".into(),
                protected: Vec::new(),
                uploads: Arc::new(Uploads::default()),
                log: None,
            }),
            calls: Mutex::default(),
            revoke_after: Mutex::default(),
        });
        let transfers = Transfers::new(loopback.clone());
        Setup { base, shared, local, root, loopback, transfers }
    }

    fn wait_for_end(transfers: &Transfers) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let jobs = transfers.list();
            let jobs = jobs.as_array().unwrap().clone();
            if jobs.iter().all(|job| !matches!(job["state"].as_str(), Some("queued" | "running"))) {
                return jobs;
            }
            assert!(Instant::now() < deadline, "the transfers did not end");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn random(length: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; length];
        OsRng.fill_bytes(&mut bytes);
        bytes
    }

    #[test]
    fn files_go_up_and_come_back_unchanged_and_several_queue_in_order() {
        let s = setup("transfer-round");
        let (big, small) = (random(5 * 1024 * 1024 + 7), random(10));
        fs::write(s.local.join("big.bin"), &big).unwrap();
        fs::write(s.local.join("small.bin"), &small).unwrap();
        fs::write(s.local.join("empty.bin"), b"").unwrap();
        let files: Vec<String> = ["big.bin", "small.bin", "empty.bin"].iter().map(|n| s.local.join(n).to_str().unwrap().to_owned()).collect();
        s.transfers.queue_uploads("dev", &s.root, "", &files).unwrap();
        let jobs = wait_for_end(&s.transfers);
        assert_eq!(jobs.iter().map(|j| j["state"].as_str().unwrap()).collect::<Vec<_>>(), ["done", "done", "done"]);
        assert_eq!(jobs[0]["done"], big.len());
        assert_eq!(fs::read(s.shared.join("big.bin")).unwrap(), big);
        assert_eq!(fs::read(s.shared.join("empty.bin")).unwrap(), b"");
        // Download one file to a chosen name, then two into a folder where one name is taken.
        let target = s.local.join("copy.bin");
        s.transfers.queue_downloads("dev", &s.root, &["big.bin".into()], target.to_str().unwrap()).unwrap();
        let out = s.base.join("out");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("small.bin"), b"mine").unwrap();
        s.transfers.queue_downloads("dev", &s.root, &["small.bin".into(), "empty.bin".into()], out.to_str().unwrap()).unwrap();
        let jobs = wait_for_end(&s.transfers);
        assert!(jobs.iter().all(|job| job["state"] == "done"), "{jobs:?}");
        assert_eq!(fs::read(&target).unwrap(), big);
        assert_eq!(fs::read(out.join("small.bin")).unwrap(), b"mine");
        assert_eq!(fs::read(out.join("small 2.bin")).unwrap(), small);
        assert_eq!(fs::read(out.join("empty.bin")).unwrap(), b"");
        assert!(!fs::read_dir(&out).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".part")));
        s.transfers.clear();
        assert_eq!(s.transfers.list().as_array().unwrap().len(), 0);
        let _ = fs::remove_dir_all(&s.base);
    }

    #[test]
    fn a_grant_that_ends_in_the_middle_cuts_the_upload_and_the_download() {
        let s = setup("transfer-revoke");
        fs::write(s.local.join("big.bin"), random(4 * 1024 * 1024)).unwrap();
        *s.loopback.revoke_after.lock().unwrap() = Some(2);
        s.transfers.queue_uploads("dev", &s.root, "", &[s.local.join("big.bin").to_str().unwrap().to_owned()]).unwrap();
        let jobs = wait_for_end(&s.transfers);
        assert_eq!(jobs[0]["state"], "failed");
        assert_eq!(jobs[0]["error"], "notPermitted");
        let leftovers = fs::read_dir(&s.shared).unwrap().count();
        assert_eq!(leftovers, 0, "no partial and no final file");
        // A download that is cut leaves no partial file here.
        s.loopback.files.lock().unwrap().grant.until = ShellUntil::OFF;
        fs::write(s.shared.join("d.bin"), random(3 * 1024 * 1024)).unwrap();
        *s.loopback.revoke_after.lock().unwrap() = Some(1);
        let target = s.local.join("d.bin");
        s.transfers.queue_downloads("dev", &s.root, &["d.bin".into()], target.to_str().unwrap()).unwrap();
        let jobs = wait_for_end(&s.transfers);
        assert_eq!(jobs[1]["state"], "failed");
        assert!(!target.exists());
        assert!(!fs::read_dir(&s.local).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains(".part")));
        let _ = fs::remove_dir_all(&s.base);
    }

    #[test]
    fn a_stopped_job_ends_and_removes_what_it_began() {
        let s = setup("transfer-stop");
        fs::write(s.local.join("a.bin"), random(4 * 1024 * 1024)).unwrap();
        fs::write(s.local.join("b.bin"), b"b").unwrap();
        // Holding the target lets the test stop the first job while it runs.
        let held = s.loopback.files.lock().unwrap();
        s.transfers.queue_uploads("dev", &s.root, "", &[s.local.join("a.bin").to_str().unwrap().to_owned(), s.local.join("b.bin").to_str().unwrap().to_owned()]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while s.transfers.list()[0]["state"] != "running" {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        s.transfers.stop(1);
        s.transfers.stop(2);
        drop(held);
        let jobs = wait_for_end(&s.transfers);
        assert_eq!(jobs[0]["state"], "stopped");
        assert_eq!(jobs[1]["state"], "stopped");
        assert_eq!(fs::read_dir(&s.shared).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&s.base);
    }

    #[test]
    fn what_the_other_device_answers_is_checked_before_it_is_kept() {
        struct Liar(Mutex<Vec<Value>>);
        impl Link for Arc<Liar> {
            fn call(&self, _device: &str, _request: &Value) -> Result<Value, &'static str> {
                Ok(self.0.lock().unwrap().remove(0))
            }
        }
        let base = fs::canonicalize(scratch("transfer-liar")).unwrap();
        let big = "A".repeat(4);
        for answers in [
            // The size changes between two chunks.
            vec![json!({"kind": "file", "size": 8}), json!({"data": big, "size": 9, "eof": false})],
            // More bytes than the file has.
            vec![json!({"kind": "file", "size": 2}), json!({"data": big, "size": 2, "eof": true})],
            // An end before the size.
            vec![json!({"kind": "file", "size": 8}), json!({"data": big, "size": 8, "eof": true})],
            // Not base64.
            vec![json!({"kind": "file", "size": 8}), json!({"data": "***", "size": 8, "eof": false})],
            // A folder.
            vec![json!({"kind": "folder", "size": 0})],
        ] {
            let liar = Arc::new(Liar(Mutex::new(answers)));
            let transfers = Transfers::new(liar);
            let target = base.join("x.bin");
            transfers.queue_downloads("dev", "r", &["x.bin".into()], target.to_str().unwrap()).unwrap();
            let jobs = wait_for_end(&transfers);
            assert_eq!(jobs[0]["state"], "failed", "{jobs:?}");
            assert!(!target.exists());
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_batch_is_checked_before_anything_is_queued() {
        let s = setup("transfer-bad");
        fs::write(s.local.join("ok.bin"), b"x").unwrap();
        let ok = s.local.join("ok.bin").to_str().unwrap().to_owned();
        assert_eq!(s.transfers.queue_uploads("dev", &s.root, "", &[ok.clone(), "relative".into()]), Err("filesPathInvalid"));
        assert_eq!(s.transfers.queue_uploads("dev", &s.root, "", &[ok.clone(), s.local.to_str().unwrap().to_owned()]), Err("filesNotRegular"));
        assert_eq!(s.transfers.queue_uploads("dev", &s.root, "", &[]), Err("filesTooMany"));
        assert_eq!(s.transfers.queue_downloads("dev", &s.root, &["a".into(), "b".into()], s.base.join("nope").to_str().unwrap()), Err("filesNotFolder"));
        assert_eq!(s.transfers.queue_downloads("dev", &s.root, &["a".into()], "relative"), Err("filesPathInvalid"));
        assert_eq!(s.transfers.list().as_array().unwrap().len(), 0);
        let _ = fs::remove_dir_all(&s.base);
    }
}
