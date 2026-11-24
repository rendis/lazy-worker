//! The actions of this device: named shell commands that only this device can write.
//!
//! A paired device with the Actions permission may run them and schedule them, and never reads the
//! command. Schedules run here while the app is open, whether or not the device that set them is on.
//! A run missed while the app was closed or the device slept is not run late: it is logged as skipped.

mod run;
mod schedule;
mod store;

use crate::{
    dispatch::{Origin, Request},
    remote::log::Entry,
};
use rand_core::{OsRng, RngCore};
use run::{Done, Handle, Runner, State};
use schedule::{next_after, Schedule, System, Zone, GRACE};
use serde::Serialize;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use store::{Action, LastRun, Store, DEFAULT_TIMEOUT, LOCAL, MAX_ACTIONS};
use tauri::{AppHandle, Manager};

/// At most this many runs at the same time on a device.
pub const MAX_RUNNING: usize = 2;
/// The finished runs that a device keeps in memory.
pub const KEPT_RUNS: usize = 20;
/// How often the scheduler looks, in seconds.
const TICK: u64 = 15;
/// The time of the last look is written at most this often, in seconds.
const SAVE_CHECK_EVERY: i64 = 60;

/// Who started a run, or who asks about one.
#[derive(Clone, Debug, PartialEq)]
pub enum Owner {
    Local,
    /// A paired device, by its id.
    Peer(String),
    /// A schedule. No request owns this run, so nobody can read or stop it.
    Scheduled,
}

impl Owner {
    pub fn of(origin: Origin) -> Self {
        match origin {
            Origin::Local => Self::Local,
            Origin::Remote(_) => Self::Peer(origin.peer().unwrap_or_default().to_owned()),
        }
    }

    /// The person here sees every run. A device sees only the runs that it started.
    fn may_see(&self, run: &Owner) -> bool {
        match self {
            Self::Local => true,
            Self::Peer(id) => !id.is_empty() && run == self,
            Self::Scheduled => false,
        }
    }
}

/// Tells whether a device is still paired and may run actions here. It is asked before a schedule of that device runs.
pub type PeerCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The record of a run goes to the log of remote access. It never holds the command or its output.
pub type Sink = Arc<dyn Fn(Entry) + Send + Sync>;

struct RunRecord {
    id: String,
    action: String,
    owner: Owner,
    /// The device that started the run, or whose schedule did.
    by: Option<String>,
    handle: Handle,
    /// The order in which finished runs ended. Running ones have none.
    ended: Option<u64>,
}

#[derive(Default)]
struct Runs {
    records: Vec<RunRecord>,
    ended: u64,
}

struct Inner {
    store: Store,
    runs: Mutex<Runs>,
    runner: Runner,
    zone: Box<dyn Zone>,
    sink: Sink,
    allowed: PeerCheck,
    /// The last time the scheduler looked, in unix seconds.
    checked: Mutex<i64>,
    /// The last time that was written to the file.
    saved: Mutex<i64>,
}

#[derive(Clone)]
pub struct Actions(Arc<Inner>);

/// One action as a window shows it. A remote device gets no `command`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub timeout_seconds: u32,
    pub schedule: Option<Schedule>,
    pub notify_on_failure: bool,
    /// `local`, or a device id. A remote device gets `local`, `you` or `other`.
    pub scheduled_by: Option<String>,
    /// The name of the device that set the schedule, for the person here.
    pub scheduled_by_name: Option<String>,
    pub last_run: Option<LastRun>,
    /// Unix seconds.
    pub next_run: Option<i64>,
    pub busy: bool,
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0))
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

/// How the end of a run is named in the log.
fn outcome(done: Done) -> &'static str {
    match done.state {
        State::Finished if done.exit_code == Some(0) => "ok",
        State::Finished | State::Failed | State::Running => "failed",
        State::Stopped => "stopped",
        State::TimedOut => "timedOut",
    }
}

fn json<T: Serialize>(value: T) -> Result<Value, &'static str> {
    serde_json::to_value(value).map_err(|_| "workerUnavailable")
}

impl Actions {
    pub fn open(path: PathBuf, sink: Sink, allowed: PeerCheck) -> Result<Self, &'static str> {
        Self::open_with(path, sink, Runner::system(), Box::new(System), allowed)
    }

    fn open_with(
        path: PathBuf,
        sink: Sink,
        runner: Runner,
        zone: Box<dyn Zone>,
        allowed: PeerCheck,
    ) -> Result<Self, &'static str> {
        let store = Store::open(path)?;
        // The first look covers the time since the last one, so the runs that a stop missed are found.
        let checked = store
            .snapshot()
            .checked_at
            .and_then(|at| i64::try_from(at).ok())
            .unwrap_or_else(unix_now);
        Ok(Self(Arc::new(Inner {
            store,
            runs: Mutex::new(Runs::default()),
            runner,
            zone,
            sink,
            allowed,
            checked: Mutex::new(checked),
            saved: Mutex::new(checked),
        })))
    }

    fn record(&self, peer: Option<&str>, request: &str, outcome: &str) {
        (self.0.sink)(Entry::new(peer, None, request, outcome));
    }

    fn view(&self, action: &Action, now: i64, busy: &[String]) -> View {
        View {
            id: action.id.clone(),
            name: action.name.clone(),
            command: Some(action.command.clone()),
            timeout_seconds: action.timeout_seconds,
            schedule: action.schedule.clone(),
            notify_on_failure: action.notify_on_failure,
            scheduled_by: action.scheduled_by.clone(),
            scheduled_by_name: None,
            last_run: action.last_run.clone(),
            next_run: action.schedule.as_ref().and_then(|schedule| next_after(schedule, now, self.0.zone.as_ref())),
            busy: busy.contains(&action.id),
        }
    }

    fn running_actions(&self) -> Vec<String> {
        lock(&self.0.runs)
            .records
            .iter()
            .filter(|record| record.handle.state() == State::Running)
            .map(|record| record.action.clone())
            .collect()
    }

    /// Every action, with its command. The caller removes what its origin may not read.
    pub fn list(&self) -> Vec<View> {
        let (now, busy) = (unix_now(), self.running_actions());
        self.0.store.snapshot().actions.iter().map(|action| self.view(action, now, &busy)).collect()
    }

    pub fn create(&self, name: &str, command: &str, timeout: Option<u32>) -> Result<View, &'static str> {
        let name = store::clean_name(name);
        let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
        if name.is_empty() {
            return Err("actionInvalid");
        }
        store::validate_command(command)?;
        store::validate_timeout(timeout)?;
        let action = Action {
            id: new_id(),
            name,
            command: command.to_owned(),
            timeout_seconds: timeout,
            schedule: None,
            notify_on_failure: false,
            scheduled_by: None,
            last_run: None,
        };
        self.0.store.change(|data| {
            if data.actions.len() >= MAX_ACTIONS {
                return Err("tooManyActions");
            }
            data.actions.push(action.clone());
            Ok(())
        })?;
        Ok(self.view(&action, unix_now(), &[]))
    }

    pub fn update(&self, id: &str, name: &str, command: &str, timeout: Option<u32>) -> Result<View, &'static str> {
        let name = store::clean_name(name);
        if name.is_empty() {
            return Err("actionInvalid");
        }
        store::validate_command(command)?;
        if let Some(timeout) = timeout {
            store::validate_timeout(timeout)?;
        }
        let action = self.0.store.change(|data| {
            let action = data.actions.iter_mut().find(|action| action.id == id).ok_or("actionNotFound")?;
            action.name = name;
            action.command = command.to_owned();
            if let Some(timeout) = timeout {
                action.timeout_seconds = timeout;
            }
            Ok(action.clone())
        })?;
        Ok(self.view(&action, unix_now(), &self.running_actions()))
    }

    pub fn delete(&self, id: &str) -> Result<(), &'static str> {
        self.0.store.change(|data| {
            let before = data.actions.len();
            data.actions.retain(|action| action.id != id);
            if data.actions.len() == before {
                return Err("actionNotFound");
            }
            Ok(())
        })
    }

    /// Starts a run and returns its id. A run goes on after its action is changed or deleted.
    pub fn run(&self, id: &str, owner: Owner) -> Result<String, &'static str> {
        let action = self.0.store.get(id).ok_or("actionNotFound")?;
        let peer = match &owner {
            Owner::Local => None,
            Owner::Peer(peer) => Some(peer.clone()),
            Owner::Scheduled => action.scheduled_by.clone().filter(|by| by != LOCAL),
        };
        let mut runs = lock(&self.0.runs);
        let running = runs.records.iter().filter(|record| record.handle.state() == State::Running);
        if running.clone().any(|record| record.action == id) {
            return Err("actionBusy");
        }
        if running.count() >= MAX_RUNNING {
            return Err("tooManyRuns");
        }
        let run_id = new_id();
        let started = u64::try_from(unix_now()).unwrap_or(0);
        let finishing = Arc::downgrade(&self.0);
        let (finishing_run, finishing_action, finishing_peer) = (run_id.clone(), id.to_owned(), peer.clone());
        let handle = run::spawn(
            &self.0.runner,
            &action.command,
            Duration::from_secs(u64::from(action.timeout_seconds)),
            move |done| {
                if let Some(inner) = finishing.upgrade() {
                    Actions(inner).finish(&finishing_run, &finishing_action, finishing_peer.as_deref(), started, done);
                }
            },
        );
        runs.records.push(RunRecord {
            id: run_id.clone(),
            action: id.to_owned(),
            owner,
            by: peer.clone(),
            handle,
            ended: None,
        });
        drop(runs);
        self.record(peer.as_deref(), "actionStart", "ok");
        Ok(run_id)
    }

    /// Writes the end of a run on its action and in the log, and keeps only the last runs.
    fn finish(&self, run: &str, action: &str, peer: Option<&str>, started: u64, done: Done) {
        let end = u64::try_from(unix_now()).unwrap_or(0).max(started);
        let last = LastRun {
            start: started,
            end,
            exit_code: done.exit_code,
            timed_out: done.timed_out(),
            ok: done.ok(),
        };
        // The action may be gone by now. The run is recorded in the log either way.
        let _ = self.0.store.change(|data| {
            if let Some(found) = data.actions.iter_mut().find(|found| found.id == action) {
                found.last_run = Some(last);
            }
            Ok(())
        });
        self.record(peer, "actionEnd", outcome(done));
        let mut runs = lock(&self.0.runs);
        runs.ended += 1;
        let order = runs.ended;
        if let Some(record) = runs.records.iter_mut().find(|record| record.id == run) {
            record.ended = Some(order);
        }
        let mut finished: Vec<u64> = runs.records.iter().filter_map(|record| record.ended).collect();
        if finished.len() > KEPT_RUNS {
            finished.sort_unstable();
            let oldest_kept = finished[finished.len() - KEPT_RUNS];
            runs.records.retain(|record| record.ended.is_none_or(|order| order >= oldest_kept));
        }
    }

    fn with_run<T>(&self, run: &str, viewer: &Owner, read: impl FnOnce(&RunRecord) -> T) -> Result<T, &'static str> {
        let runs = lock(&self.0.runs);
        runs.records
            .iter()
            .find(|record| record.id == run && viewer.may_see(&record.owner))
            .map(read)
            // A run that belongs to someone else looks like a run that does not exist.
            .ok_or("actionRunNotFound")
    }

    pub fn get_run(&self, run: &str, viewer: &Owner) -> Result<run::View, &'static str> {
        self.with_run(run, viewer, |record| record.handle.view())
    }

    pub fn stop_run(&self, run: &str, viewer: &Owner) -> Result<(), &'static str> {
        self.with_run(run, viewer, |record| record.handle.stop())
    }

    /// Sets when an action runs, and who set it. A single run must still be ahead.
    /// A device may set a schedule on an action that has none, or replace one that it set. The person here may always.
    pub fn schedule(&self, id: &str, schedule: Schedule, notify: bool, by: &str) -> Result<(), &'static str> {
        let now = unix_now();
        schedule.validate_new(u64::try_from(now).unwrap_or(0))?;
        self.0.store.change(|data| {
            let others = data.actions.iter().any(|action| action.id != id && action.schedule.is_some());
            let action = data.actions.iter_mut().find(|action| action.id == id).ok_or("actionNotFound")?;
            if Self::owned_elsewhere(action, by) {
                return Err("scheduleOwnedElsewhere");
            }
            action.schedule = Some(schedule);
            action.notify_on_failure = notify;
            action.scheduled_by = Some(by.to_owned());
            // With no other schedule, the last look may be old, and it must not count the time before this schedule as missed.
            if !others {
                data.checked_at = Some(u64::try_from(now).unwrap_or(0));
            }
            Ok(())
        })
    }

    /// Whether a schedule that someone else set stands on the action, as seen by `by`.
    fn owned_elsewhere(action: &Action, by: &str) -> bool {
        by != LOCAL && action.schedule.is_some() && action.scheduled_by.as_deref() != Some(by)
    }

    /// Clears a schedule for a device. Only one that the same device set, or none at all, can go.
    pub fn clear_schedule_by(&self, id: &str, by: &str) -> Result<(), &'static str> {
        self.0.store.change(|data| {
            let action = data.actions.iter_mut().find(|action| action.id == id).ok_or("actionNotFound")?;
            if Self::owned_elsewhere(action, by) {
                return Err("scheduleOwnedElsewhere");
            }
            action.schedule = None;
            action.scheduled_by = None;
            Ok(())
        })
    }

    pub fn clear_schedule(&self, id: &str) -> Result<(), &'static str> {
        self.0.store.change(|data| {
            let action = data.actions.iter_mut().find(|action| action.id == id).ok_or("actionNotFound")?;
            action.schedule = None;
            action.scheduled_by = None;
            Ok(())
        })
    }

    /// Removes the schedules that a device set, and keeps the actions. With no id it removes the schedules of every device.
    pub fn clear_schedules_of(&self, peer: Option<&str>) -> Result<usize, &'static str> {
        self.0.store.change(|data| {
            let mut cleared = 0;
            for action in &mut data.actions {
                let hit = match (&action.scheduled_by, peer) {
                    (Some(by), Some(peer)) => by == peer,
                    (Some(by), None) => by != LOCAL,
                    (None, _) => false,
                };
                if hit {
                    action.schedule = None;
                    action.scheduled_by = None;
                    cleared += 1;
                }
            }
            Ok(cleared)
        })
    }

    /// What happens when a device loses access: its schedules go, and its runs end.
    pub fn forget(&self, peer: Option<&str>) {
        let _ = self.clear_schedules_of(peer);
        self.stop_runs_of(peer);
    }

    /// Stops the runs that a device started, or that its schedules did. With no id it stops those of every device.
    pub fn stop_runs_of(&self, peer: Option<&str>) {
        let handles: Vec<Handle> = lock(&self.0.runs)
            .records
            .iter()
            .filter(|record| match (&record.by, peer) {
                (Some(by), Some(peer)) => by == peer,
                (Some(_), None) => true,
                (None, _) => false,
            })
            .map(|record| record.handle.clone())
            .collect();
        for handle in handles {
            handle.stop();
        }
    }

    /// One look of the scheduler. A run that is due starts when it is at most `GRACE` seconds late. A later one is
    /// logged as skipped, once, because the app was closed or the device slept.
    pub fn tick(&self, now: i64) {
        let last = {
            let mut checked = lock(&self.0.checked);
            let last = *checked;
            if now <= last {
                return;
            }
            *checked = now;
            last
        };
        let data = self.0.store.snapshot();
        for action in &data.actions {
            let Some(schedule) = &action.schedule else { continue };
            // A device that is gone, or has lost Actions, runs nothing, even if its schedule outlived that.
            if let Some(peer) = action.scheduled_by.as_deref().filter(|by| *by != LOCAL) {
                if !(self.0.allowed)(peer) {
                    let _ = self.clear_schedule(&action.id);
                    self.record(Some(peer), "actionScheduleCleared", "notPermitted");
                    continue;
                }
            }
            let Some(next) = next_after(schedule, last, self.0.zone.as_ref()).filter(|next| *next <= now) else {
                continue;
            };
            let started = if now - next <= GRACE { self.run(&action.id, Owner::Scheduled) } else { Err("skipped") };
            if let Err(code) = started {
                let peer = action.scheduled_by.as_deref().filter(|by| *by != LOCAL);
                self.record(peer, "actionSkipped", code);
            }
            if matches!(schedule, Schedule::Once { .. }) {
                let _ = self.clear_schedule(&action.id);
            }
        }
        self.save_check(now, false);
    }

    /// Writes the time of the last look, at most once a minute, and only while there is an action.
    fn save_check(&self, now: i64, force: bool) {
        let mut saved = lock(&self.0.saved);
        if !force && now - *saved < SAVE_CHECK_EVERY {
            return;
        }
        if self.0.store.snapshot().actions.is_empty() {
            return;
        }
        if self
            .0
            .store
            .change(|data| {
                data.checked_at = Some(u64::try_from(now).unwrap_or(0));
                Ok(())
            })
            .is_ok()
        {
            *saved = now;
        }
    }

    /// Looks every few seconds, from a thread that ends with the app.
    pub fn start_scheduler(&self) {
        let weak = Arc::downgrade(&self.0);
        thread::spawn(move || loop {
            let Some(inner) = weak.upgrade() else { break };
            Actions(inner).tick(unix_now());
            thread::sleep(Duration::from_secs(TICK));
        });
    }

    /// Remembers when the app stopped looking, so the next start can tell what it missed.
    /// Also ends the commands still running, so none outlives the app. Waits a few seconds at most.
    pub fn stop(&self) {
        let now = unix_now();
        *lock(&self.0.checked) = now;
        self.save_check(now, true);
        let running: Vec<Handle> = lock(&self.0.runs)
            .records
            .iter()
            .filter(|record| record.handle.state() == State::Running)
            .map(|record| record.handle.clone())
            .collect();
        for handle in &running {
            handle.stop();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while std::time::Instant::now() < deadline
            && running.iter().any(|handle| handle.state() == State::Running)
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Answers one request of the Actions family. `names` gives the name of a paired device, for the person here.
    pub fn serve(
        &self,
        origin: Origin,
        request: Request,
        names: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Value, &'static str> {
        let owner = Owner::of(origin);
        let writes = matches!(origin, Origin::Local);
        match request {
            Request::ListActions => json(redact(self.list(), origin, names)),
            // Only the person here writes actions. The access table says so too.
            Request::CreateAction { .. } | Request::UpdateAction { .. } | Request::DeleteAction { .. } if !writes => {
                Err("localOnly")
            }
            Request::CreateAction { name, command, timeout_seconds } => {
                json(self.create(&name, &command, timeout_seconds)?)
            }
            Request::UpdateAction { id, name, command, timeout_seconds } => {
                json(self.update(&id, &name, &command, timeout_seconds)?)
            }
            Request::DeleteAction { id } => self.delete(&id).map(|()| Value::Null),
            Request::RunAction { id } => {
                json(serde_json::json!({ "runId": self.run(&id, owner)? }))
            }
            Request::GetActionRun { run_id } => json(self.get_run(&run_id, &owner)?),
            Request::StopActionRun { run_id } => self.stop_run(&run_id, &owner).map(|()| Value::Null),
            Request::ScheduleAction { id, schedule, notify_on_failure } => {
                let schedule: Schedule = serde_json::from_value(schedule).map_err(|_| "scheduleInvalid")?;
                let by = match &owner {
                    Owner::Local => LOCAL.to_owned(),
                    Owner::Peer(peer) if !peer.is_empty() => peer.clone(),
                    Owner::Peer(_) | Owner::Scheduled => return Err("notPermitted"),
                };
                self.schedule(&id, schedule, notify_on_failure, &by).map(|()| Value::Null)
            }
            Request::ClearSchedule { id } => match &owner {
                Owner::Local => self.clear_schedule(&id),
                Owner::Peer(peer) if !peer.is_empty() => self.clear_schedule_by(&id, peer),
                Owner::Peer(_) | Owner::Scheduled => Err("notPermitted"),
            }
            .map(|()| Value::Null),
            _ => Err("badRequest"),
        }
    }
}

/// The list as an origin may read it. A remote device never gets a command, and does not learn which
/// other device set a schedule.
fn redact(mut views: Vec<View>, origin: Origin, names: &dyn Fn(&str) -> Option<String>) -> Vec<View> {
    for view in &mut views {
        match origin {
            Origin::Local => {
                view.scheduled_by_name = view.scheduled_by.as_deref().filter(|by| *by != LOCAL).and_then(names);
            }
            Origin::Remote(_) => {
                view.command = None;
                view.scheduled_by = view.scheduled_by.take().map(|by| {
                    if by == LOCAL {
                        LOCAL.to_owned()
                    } else if Some(by.as_str()) == origin.peer() {
                        "you".to_owned()
                    } else {
                        "other".to_owned()
                    }
                });
            }
        }
    }
    views
}

/// Answers a request of the Actions family for the dispatcher.
pub fn handle(app: &AppHandle, origin: Origin, request: Request) -> Result<Value, &'static str> {
    let actions = app.try_state::<Actions>().ok_or("actionsUnavailable")?;
    let service = app.try_state::<crate::remote::service::Service>();
    let names = |id: &str| service.as_ref().and_then(|service| service.peer_name(id));
    actions.serve(origin, request, &names)
}

/// Removes the schedules of a device that lost its access, and stops its runs. With no id it does this for every device.
pub fn forget_peer(app: &AppHandle, peer: Option<&str>) {
    if let Some(actions) = app.try_state::<Actions>() {
        actions.forget(peer);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        run::testing::{alive, runner},
        schedule::testing::{ny, Dst, Plain},
        store::testing::{ID_A, ID_B},
        *,
    };
    use crate::{dispatch::Permissions, remote::testing::scratch};
    use serde_json::json;
    use std::fs;

    type Log = Arc<Mutex<Vec<Entry>>>;

    fn open(label: &str, zone: Box<dyn Zone>) -> (Actions, Log, PathBuf) {
        open_allowing(label, zone, allow_all())
    }

    fn open_allowing(label: &str, zone: Box<dyn Zone>, allowed: PeerCheck) -> (Actions, Log, PathBuf) {
        let dir = scratch(label);
        let log: Log = Arc::default();
        let sink: Sink = {
            let log = log.clone();
            Arc::new(move |entry| log.lock().unwrap().push(entry))
        };
        let actions = Actions::open_with(dir.join("actions.json"), sink, runner(dir.clone()), zone, allowed).unwrap();
        (actions, log, dir)
    }

    fn allow_all() -> PeerCheck {
        Arc::new(|_| true)
    }

    fn request(value: Value) -> Request {
        serde_json::from_value(value).unwrap()
    }

    fn peer(id: &str) -> Origin {
        Origin::Remote(Permissions { control: false, applications: false, actions: true, peer: crate::dispatch::PeerId::parse(id) })
    }

    fn names(id: &str) -> Option<String> {
        (id == ID_A).then(|| "Studio".to_owned())
    }

    fn call(actions: &Actions, origin: Origin, value: Value) -> Result<Value, &'static str> {
        actions.serve(origin, request(value), &names)
    }

    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let until = std::time::Instant::now() + Duration::from_secs(20);
        while !ready() {
            assert!(std::time::Instant::now() < until, "{what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn finished(actions: &Actions, run: &str) -> run::View {
        wait_for("the run ended", || actions.get_run(run, &Owner::Local).unwrap().state != State::Running);
        actions.get_run(run, &Owner::Local).unwrap()
    }

    fn create(actions: &Actions, name: &str, command: &str) -> String {
        call(actions, Origin::Local, json!({"command": "createAction", "args": {"name": name, "command": command}}))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn a_run_is_asynchronous_and_its_result_is_recorded_without_the_command() {
        let (actions, log, dir) = open("actions-run", Box::new(Plain));
        let id = create(&actions, "Say hi", "echo secret-command-text; exit 2");
        let started = call(&actions, Origin::Local, json!({"command": "runAction", "args": {"id": id}})).unwrap();
        let run = started["runId"].as_str().unwrap().to_owned();
        let view = finished(&actions, &run);
        assert_eq!((view.state, view.exit_code), (State::Finished, Some(2)));
        assert_eq!(view.output, "secret-command-text\n");
        wait_for("the end was logged", || log.lock().unwrap().len() >= 2);
        let entries = log.lock().unwrap().clone();
        assert_eq!(entries[0].request, "actionStart");
        assert_eq!((entries[1].request.as_str(), entries[1].outcome.as_str()), ("actionEnd", "failed"));
        assert!(entries.iter().all(|entry| entry.peer.is_none()));
        assert!(!format!("{entries:?}").contains("secret-command-text"));
        let listed = actions.list();
        let last = listed[0].last_run.as_ref().unwrap();
        assert!(!last.ok && last.exit_code == Some(2) && !last.timed_out);
        assert!(!listed[0].busy);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_run_answer_has_the_shape_the_window_reads() {
        let (actions, _, dir) = open("actions-shape", Box::new(Plain));
        let id = create(&actions, "Say hi", "echo hi");
        let run = actions.run(&id, Owner::Local).unwrap();
        finished(&actions, &run);
        let answer = call(&actions, Origin::Local, json!({"command": "getActionRun", "args": {"runId": run}})).unwrap();
        assert_eq!(answer["state"], "finished");
        assert_eq!(answer["exitCode"], 0);
        assert_eq!(answer["output"], "hi\n");
        assert_eq!(answer["truncated"], false);
        assert!(answer["seconds"].is_u64());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn at_most_two_runs_at_once_and_one_for_each_action() {
        let (actions, _, dir) = open("actions-limits", Box::new(Plain));
        let (a, b, c) = (create(&actions, "A", "sleep 30"), create(&actions, "B", "sleep 30"), create(&actions, "C", "echo c"));
        let first = actions.run(&a, Owner::Local).unwrap();
        assert_eq!(actions.run(&a, Owner::Local), Err("actionBusy"));
        let second = actions.run(&b, Owner::Local).unwrap();
        assert_eq!(actions.run(&c, Owner::Local), Err("tooManyRuns"));
        assert!(actions.list().iter().filter(|view| view.busy).count() == 2);
        actions.stop_run(&first, &Owner::Local).unwrap();
        assert_eq!(finished(&actions, &first).state, State::Stopped);
        // A place is free again.
        let third = actions.run(&c, Owner::Local).unwrap();
        assert_eq!(finished(&actions, &third).output, "c\n");
        actions.stop_run(&second, &Owner::Local).unwrap();
        finished(&actions, &second);
        assert_eq!(actions.run("nope", Owner::Local), Err("actionNotFound"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_the_last_twenty_finished_runs_are_kept() {
        let (actions, _, dir) = open("actions-kept", Box::new(Plain));
        let id = create(&actions, "Echo", "echo x");
        let mut runs = Vec::new();
        for _ in 0..(KEPT_RUNS + 3) {
            let run = actions.run(&id, Owner::Local).unwrap();
            finished(&actions, &run);
            // The record is pruned by the thread that ended the run.
            wait_for("recorded", || lock(&actions.0.runs).records.iter().any(|r| r.id == run && r.ended.is_some()));
            runs.push(run);
        }
        assert_eq!(lock(&actions.0.runs).records.len(), KEPT_RUNS);
        assert_eq!(actions.get_run(&runs[0], &Owner::Local), Err("actionRunNotFound"));
        assert!(actions.get_run(runs.last().unwrap(), &Owner::Local).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_device_reads_and_stops_only_the_runs_it_started() {
        let (actions, log, dir) = open("actions-owner", Box::new(Plain));
        let id = create(&actions, "Wait", "sleep 30");
        let mine = call(&actions, peer(ID_A), json!({"command": "runAction", "args": {"id": id}})).unwrap()["runId"]
            .as_str()
            .unwrap()
            .to_owned();
        // Another device and the scheduler cannot see it, and it looks like a run that does not exist.
        for other in [peer(ID_B), Origin::Remote(Permissions { actions: true, ..Permissions::default() })] {
            for command in ["getActionRun", "stopActionRun"] {
                assert_eq!(
                    call(&actions, other, json!({"command": command, "args": {"runId": mine}})),
                    Err("actionRunNotFound")
                );
            }
        }
        assert!(call(&actions, peer(ID_A), json!({"command": "getActionRun", "args": {"runId": mine}})).is_ok());
        // The person here sees every run.
        assert_eq!(actions.get_run(&mine, &Owner::Local).unwrap().state, State::Running);
        call(&actions, peer(ID_A), json!({"command": "stopActionRun", "args": {"runId": mine}})).unwrap();
        assert_eq!(finished(&actions, &mine).state, State::Stopped);
        wait_for("logged", || log.lock().unwrap().len() >= 2);
        let entries = log.lock().unwrap().clone();
        assert!(entries.iter().all(|entry| entry.peer.as_deref() == Some(ID_A)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_device_never_reads_a_command_and_never_writes_an_action() {
        let (actions, _, dir) = open("actions-redact", Box::new(Plain));
        let id = create(&actions, "Secret job", "curl -H 'Authorization: sk-secret-123' https://x");
        let local = call(&actions, Origin::Local, json!({"command": "listActions"})).unwrap();
        assert_eq!(local[0]["command"], "curl -H 'Authorization: sk-secret-123' https://x");
        let remote = call(&actions, peer(ID_A), json!({"command": "listActions"})).unwrap();
        assert!(!remote.to_string().contains("sk-secret-123"));
        assert!(remote[0].get("command").is_none());
        assert_eq!(remote[0]["name"], "Secret job");
        for value in [
            json!({"command": "createAction", "args": {"name": "x", "command": "echo"}}),
            json!({"command": "updateAction", "args": {"id": id, "name": "x", "command": "echo"}}),
            json!({"command": "deleteAction", "args": {"id": id}}),
        ] {
            assert_eq!(call(&actions, peer(ID_A), value), Err("localOnly"));
        }
        assert_eq!(actions.list().len(), 1);
        assert_eq!(actions.list()[0].command.as_deref(), Some("curl -H 'Authorization: sk-secret-123' https://x"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_schedule_records_who_set_it_and_a_device_sees_only_whether_it_was_itself() {
        let (actions, _, dir) = open("actions-who", Box::new(Plain));
        let id = create(&actions, "Backup", "echo hi");
        let at = u64::try_from(unix_now()).unwrap() + 3600;
        let set = |origin: Origin| {
            call(&actions, origin, json!({"command": "scheduleAction", "args": {
                "id": id, "schedule": {"kind": "once", "at": at}, "notifyOnFailure": true}})).unwrap();
        };
        set(peer(ID_A));
        let local = call(&actions, Origin::Local, json!({"command": "listActions"})).unwrap();
        assert_eq!((local[0]["scheduledBy"].as_str(), local[0]["scheduledByName"].as_str()), (Some(ID_A), Some("Studio")));
        assert_eq!(local[0]["notifyOnFailure"], true);
        assert_eq!(local[0]["nextRun"], at);
        assert_eq!(call(&actions, peer(ID_A), json!({"command": "listActions"})).unwrap()[0]["scheduledBy"], "you");
        assert_eq!(call(&actions, peer(ID_B), json!({"command": "listActions"})).unwrap()[0]["scheduledBy"], "other");
        set(Origin::Local);
        assert_eq!(call(&actions, peer(ID_A), json!({"command": "listActions"})).unwrap()[0]["scheduledBy"], "local");
        assert_eq!(actions.list()[0].scheduled_by.as_deref(), Some(LOCAL));
        assert_eq!(
            call(&actions, peer(ID_B), json!({"command": "clearSchedule", "args": {"id": id}})),
            Err("scheduleOwnedElsewhere")
        );
        call(&actions, Origin::Local, json!({"command": "clearSchedule", "args": {"id": id}})).unwrap();
        assert!(actions.list()[0].schedule.is_none() && actions.list()[0].scheduled_by.is_none());
        // A schedule that is not valid is refused.
        for schedule in [
            json!({"kind": "once", "at": 5}),
            json!({"kind": "daily", "time": "24:00"}),
            json!({"kind": "weekly", "day": 7, "time": "01:00"}),
            json!({"kind": "hourly"}),
            json!({"kind": "daily", "time": "01:00", "extra": 1}),
        ] {
            assert_eq!(
                call(&actions, Origin::Local, json!({"command": "scheduleAction", "args": {"id": id, "schedule": schedule, "notifyOnFailure": false}})),
                Err("scheduleInvalid"),
                "{schedule}"
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removing_a_device_clears_its_schedules_and_keeps_the_actions() {
        let (actions, _, dir) = open("actions-peer-removed", Box::new(Plain));
        let (a, b, c) = (create(&actions, "A", "echo a"), create(&actions, "B", "echo b"), create(&actions, "C", "echo c"));
        let daily = Schedule::Daily { time: "02:00".into() };
        actions.schedule(&a, daily.clone(), false, ID_A).unwrap();
        actions.schedule(&b, daily.clone(), false, ID_B).unwrap();
        actions.schedule(&c, daily, false, LOCAL).unwrap();
        assert_eq!(actions.clear_schedules_of(Some(ID_A)), Ok(1));
        let by_id = |id: &str| actions.list().into_iter().find(|view| view.id == id).unwrap();
        assert!(by_id(&a).schedule.is_none() && by_id(&a).scheduled_by.is_none());
        assert!(by_id(&b).schedule.is_some() && by_id(&c).schedule.is_some());
        assert_eq!(actions.list().len(), 3);
        // Cut all removes every device's schedules, and the person's stay.
        assert_eq!(actions.clear_schedules_of(None), Ok(1));
        assert!(by_id(&b).schedule.is_none() && by_id(&c).schedule.is_some());
        assert_eq!(actions.clear_schedules_of(Some(ID_A)), Ok(0));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn creating_and_editing_check_every_field() {
        let (actions, _, dir) = open("actions-edit", Box::new(Plain));
        let id = create(&actions, "  Back\u{202E}up  ", "echo hi");
        let listed = &actions.list()[0];
        assert_eq!((listed.name.as_str(), listed.timeout_seconds), ("Backup", DEFAULT_TIMEOUT));
        for (name, command, timeout) in [("", "echo", None), ("a", "  ", None), ("a", "a\0b", None), ("a", "echo", Some(0)), ("a", "echo", Some(3601))] {
            assert_eq!(actions.create(name, command, timeout).err(), Some("actionInvalid"));
            assert_eq!(actions.update(&id, name, command, timeout).err(), Some("actionInvalid"));
        }
        assert_eq!(actions.update("nope", "a", "echo", None).err(), Some("actionNotFound"));
        let edited = actions.update(&id, "Renamed", "echo again", Some(10)).unwrap();
        assert_eq!((edited.name.as_str(), edited.timeout_seconds), ("Renamed", 10));
        // Keeps the limit when none is sent.
        assert_eq!(actions.update(&id, "Renamed", "echo again", None).unwrap().timeout_seconds, 10);
        actions.delete(&id).unwrap();
        assert_eq!(actions.delete(&id), Err("actionNotFound"));
        for index in 0..MAX_ACTIONS {
            actions.create(&format!("a{index}"), "echo", None).unwrap();
        }
        assert_eq!(actions.create("one more", "echo", None).err(), Some("tooManyActions"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_stored_action_survives_a_restart_and_a_bad_file_stops_the_start() {
        let (actions, _, dir) = open("actions-restart", Box::new(Plain));
        let id = create(&actions, "Keep", "echo keep");
        drop(actions);
        let sink: Sink = Arc::new(|_| {});
        let again = Actions::open_with(dir.join("actions.json"), sink.clone(), runner(dir.clone()), Box::new(Plain), allow_all()).unwrap();
        assert_eq!(again.list()[0].id, id);
        fs::write(dir.join("actions.json"), "garbage").unwrap();
        assert_eq!(Actions::open_with(dir.join("actions.json"), sink, runner(dir.clone()), Box::new(Plain), allow_all()).err(), Some("actionsFileInvalid"));
        assert_eq!(fs::read_to_string(dir.join("actions.json")).unwrap(), "garbage");
        fs::remove_dir_all(dir).unwrap();
    }

    fn schedule_at(actions: &Actions, id: &str, schedule: Schedule, by: &str) {
        // The file is written directly, so a test can place a schedule in the past.
        actions
            .0
            .store
            .change(|data| {
                let action = data.actions.iter_mut().find(|action| action.id == id).unwrap();
                action.schedule = Some(schedule);
                action.scheduled_by = Some(by.to_owned());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_due_run_starts_and_a_single_run_clears_itself() {
        let (actions, log, dir) = open("actions-tick", Box::new(Plain));
        let id = create(&actions, "Once", "echo once");
        *lock(&actions.0.checked) = 1_000;
        schedule_at(&actions, &id, Schedule::Once { at: 1_030 }, ID_A);
        actions.tick(1_020);
        assert!(actions.list()[0].last_run.is_none());
        actions.tick(1_035);
        wait_for("it ran", || actions.list()[0].last_run.is_some());
        let view = &actions.list()[0];
        assert!(view.schedule.is_none() && view.scheduled_by.is_none(), "a single run clears itself");
        assert!(view.last_run.as_ref().unwrap().ok);
        wait_for("logged", || log.lock().unwrap().len() >= 2);
        let entries = log.lock().unwrap().clone();
        // The run is attributed to the device that set the schedule.
        assert!(entries.iter().all(|entry| entry.peer.as_deref() == Some(ID_A)));
        // Later looks do not run it again.
        actions.tick(1_100);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(log.lock().unwrap().len(), 2);
        // Nobody holds the run: it cannot be read through a request.
        let runs = lock(&actions.0.runs);
        assert!(runs.records.iter().all(|record| record.owner == Owner::Scheduled));
        drop(runs);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_run_missed_while_the_app_was_closed_is_skipped_once_and_not_run_late() {
        let (actions, log, dir) = open("actions-skipped", Box::new(Plain));
        let id = create(&actions, "Nightly", "echo night");
        // The last look was at 01:00 on day 1, and the app starts at 09:00 on day 2. The run of 02:00 on day 1 was missed.
        let (last, now) = (86_400 + 3600, 2 * 86_400 + 9 * 3600);
        *lock(&actions.0.checked) = last;
        schedule_at(&actions, &id, Schedule::Daily { time: "02:00".into() }, LOCAL);
        actions.tick(now);
        thread::sleep(Duration::from_millis(200));
        assert!(actions.list()[0].last_run.is_none(), "a missed run is not run late");
        let entries = log.lock().unwrap().clone();
        assert_eq!(entries.len(), 1, "skipped once, not once for every day");
        assert_eq!((entries[0].request.as_str(), entries[0].outcome.as_str()), ("actionSkipped", "skipped"));
        assert!(entries[0].peer.is_none());
        // The schedule stays, and the next look does not log it again.
        assert!(actions.list()[0].schedule.is_some());
        actions.tick(now + 15);
        assert_eq!(log.lock().unwrap().len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_single_run_that_was_missed_is_skipped_and_cleared() {
        let (actions, log, dir) = open("actions-once-missed", Box::new(Plain));
        let id = create(&actions, "Once", "echo once");
        *lock(&actions.0.checked) = 1_000;
        schedule_at(&actions, &id, Schedule::Once { at: 1_500 }, ID_B);
        // The device slept from 1 000 to 5 000.
        actions.tick(5_000);
        thread::sleep(Duration::from_millis(100));
        assert!(actions.list()[0].last_run.is_none() && actions.list()[0].schedule.is_none());
        let entries = log.lock().unwrap().clone();
        assert_eq!((entries[0].request.as_str(), entries[0].peer.as_deref()), ("actionSkipped", Some(ID_B)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_due_run_that_finds_the_action_busy_is_skipped_and_the_clock_going_back_changes_nothing() {
        let (actions, log, dir) = open("actions-busy", Box::new(Plain));
        let id = create(&actions, "Slow", "sleep 30");
        let running = actions.run(&id, Owner::Local).unwrap();
        *lock(&actions.0.checked) = 1_000;
        schedule_at(&actions, &id, Schedule::Once { at: 1_010 }, LOCAL);
        actions.tick(1_015);
        let skipped: Vec<_> = log.lock().unwrap().iter().filter(|e| e.request == "actionSkipped").map(|e| e.outcome.clone()).collect();
        assert_eq!(skipped, ["actionBusy"]);
        // A clock that went back is ignored.
        actions.tick(900);
        assert_eq!(*lock(&actions.0.checked), 1_015);
        actions.stop_run(&running, &Owner::Local).unwrap();
        finished(&actions, &running);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_time_of_the_last_look_is_kept_for_the_next_start() {
        let (actions, _, dir) = open("actions-checked", Box::new(Plain));
        create(&actions, "Nightly", "echo");
        *lock(&actions.0.checked) = 5_000;
        *lock(&actions.0.saved) = 5_000;
        // Within a minute nothing is written.
        actions.tick(5_030);
        assert_ne!(actions.0.store.snapshot().checked_at, Some(5_030));
        actions.tick(5_070);
        assert_eq!(actions.0.store.snapshot().checked_at, Some(5_070));
        actions.stop();
        let stopped = actions.0.store.snapshot().checked_at.unwrap();
        assert!(stopped >= 1_700_000_000);
        drop(actions);
        let sink: Sink = Arc::new(|_| {});
        let again = Actions::open_with(dir.join("actions.json"), sink, runner(dir.clone()), Box::new(Plain), allow_all()).unwrap();
        assert_eq!(*lock(&again.0.checked), i64::try_from(stopped).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_daily_run_fires_once_on_the_days_the_clock_skips_or_repeats_an_hour() {
        // Looks every 15 minutes across a day and returns when each run started.
        fn starts(actions: &Actions, log: &Log, from: i64, hours: i64) -> usize {
            *lock(&actions.0.checked) = from;
            for step in 1..=(hours * 4) {
                actions.tick(from + step * 900);
                // The clock here is made up and runs fast, so each run ends before the next look.
                wait_for("the run ended", || actions.running_actions().is_empty());
            }
            let starts = log.lock().unwrap().iter().filter(|entry| entry.request == "actionStart").count();
            log.lock().unwrap().clear();
            starts
        }
        let (actions, log, dir) = open("actions-dst", Box::new(Dst));
        let id = create(&actions, "Night", "echo n");
        // 2026-11-01: 01:30 happens twice.
        schedule_at(&actions, &id, Schedule::Daily { time: "01:30".into() }, LOCAL);
        assert_eq!(starts(&actions, &log, ny(2026, 10, 31, 12, 0), 30), 1);
        // 2026-03-08: 02:30 does not exist, and the run happens once, when the skip ends.
        schedule_at(&actions, &id, Schedule::Daily { time: "02:30".into() }, LOCAL);
        assert_eq!(starts(&actions, &log, ny(2026, 3, 7, 12, 0), 54), 2, "03-08 once, and 03-09 once");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_shell_runner_ends_a_group_on_stop() {
        let (actions, _, dir) = open("actions-group", Box::new(Plain));
        let id = create(&actions, "Group", "sleep 30 & echo $!; wait");
        let run = actions.run(&id, Owner::Local).unwrap();
        wait_for("child id printed", || actions.get_run(&run, &Owner::Local).unwrap().output.contains('\n'));
        let child: i32 = actions.get_run(&run, &Owner::Local).unwrap().output.trim().parse().unwrap();
        assert!(alive(child));
        actions.stop_run(&run, &Owner::Local).unwrap();
        assert_eq!(finished(&actions, &run).state, State::Stopped);
        wait_for("child ended", || !alive(child));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_device_may_only_change_a_schedule_that_it_set_or_that_nobody_set() {
        let (actions, _, dir) = open("actions-owned", Box::new(Plain));
        let id = create(&actions, "Backup", "echo hi");
        let at = u64::try_from(unix_now()).unwrap() + 3600;
        let set = |origin: Origin| {
            call(&actions, origin, json!({"command": "scheduleAction", "args": {
                "id": id, "schedule": {"kind": "once", "at": at}, "notifyOnFailure": false}}))
        };
        let clear = |origin: Origin| call(&actions, origin, json!({"command": "clearSchedule", "args": {"id": id}}));
        assert!(set(peer(ID_A)).is_ok(), "no schedule yet");
        assert!(set(peer(ID_A)).is_ok(), "its own schedule");
        assert_eq!(set(peer(ID_B)), Err("scheduleOwnedElsewhere"));
        assert_eq!(clear(peer(ID_B)), Err("scheduleOwnedElsewhere"));
        assert_eq!(actions.list()[0].scheduled_by.as_deref(), Some(ID_A));
        assert!(clear(peer(ID_A)).is_ok());
        assert!(clear(peer(ID_B)).is_ok(), "nothing to protect");
        assert!(set(Origin::Local).is_ok());
        assert_eq!(set(peer(ID_A)), Err("scheduleOwnedElsewhere"));
        assert_eq!(clear(peer(ID_A)), Err("scheduleOwnedElsewhere"));
        assert_eq!(actions.list()[0].scheduled_by.as_deref(), Some(LOCAL));
        assert!(set(Origin::Local).is_ok() && clear(Origin::Local).is_ok());
        assert!(set(peer(ID_A)).is_ok());
        assert!(set(Origin::Local).is_ok(), "the person here can replace any schedule");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_schedule_of_a_device_that_lost_actions_is_cleared_and_logged_instead_of_run() {
        let still: Arc<Mutex<bool>> = Arc::new(Mutex::new(true));
        let check: PeerCheck = {
            let still = still.clone();
            Arc::new(move |id| id == ID_A && *still.lock().unwrap())
        };
        let (actions, log, dir) = open_allowing("actions-tick-peer", Box::new(Plain), check);
        let (a, b, c) = (create(&actions, "A", "echo a"), create(&actions, "B", "echo b"), create(&actions, "C", "echo c"));
        *lock(&actions.0.checked) = 1_000;
        schedule_at(&actions, &a, Schedule::Once { at: 1_010 }, ID_A);
        schedule_at(&actions, &b, Schedule::Once { at: 1_010 }, ID_B);
        schedule_at(&actions, &c, Schedule::Once { at: 1_010 }, LOCAL);
        *still.lock().unwrap() = false;
        actions.tick(1_012);
        thread::sleep(Duration::from_millis(200));
        let entries = log.lock().unwrap().clone();
        let cleared: Vec<_> = entries.iter().filter(|e| e.request == "actionScheduleCleared").filter_map(|e| e.peer.clone()).collect();
        assert_eq!(cleared.len(), 2);
        assert!(cleared.contains(&ID_A.to_owned()) && cleared.contains(&ID_B.to_owned()));
        let starts: Vec<_> = entries.iter().filter(|e| e.request == "actionStart").collect();
        assert_eq!(starts.len(), 1, "only the person's own schedule ran");
        assert!(starts[0].peer.is_none());
        assert!(actions.list().iter().all(|view| view.schedule.is_none()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forgetting_a_device_stops_its_runs_and_clears_its_schedules_and_leaves_the_rest() {
        let (actions, _, dir) = open("actions-forget", Box::new(Plain));
        let (a, b, c) = (create(&actions, "A", "sleep 30"), create(&actions, "B", "sleep 30"), create(&actions, "C", "echo c"));
        let run_a = actions.run(&a, Owner::Peer(ID_A.to_owned())).unwrap();
        let run_b = actions.run(&b, Owner::Local).unwrap();
        actions.schedule(&c, Schedule::Daily { time: "02:00".into() }, false, ID_A).unwrap();
        actions.forget(Some(ID_A));
        wait_for("the run of the device ended", || actions.get_run(&run_a, &Owner::Local).unwrap().state != State::Running);
        assert_eq!(actions.get_run(&run_b, &Owner::Local).unwrap().state, State::Running, "the person's run goes on");
        assert!(actions.list().iter().find(|view| view.id == c).unwrap().schedule.is_none());
        actions.stop_run(&run_b, &Owner::Local).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
