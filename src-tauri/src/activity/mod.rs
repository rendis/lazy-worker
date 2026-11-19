use crate::preferences::{self, Preferences};
use chrono::Local;
use serde::Serialize;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;

#[cfg(not(target_os = "macos"))]
#[path = "unsupported.rs"]
mod platform;

#[cfg(target_os = "macos")]
pub(crate) use platform::helper_command;
use platform::Worker;
pub use platform::{accessibility_status, request_accessibility};

enum Mode {
    Manual(Option<Instant>),
    Scheduled,
}

struct Session {
    worker: Worker,
    mode: Mode,
    started_at: Instant,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported: bool,
    pub mode: &'static str,
    pub seconds_left: Option<u64>,
    pub elapsed_seconds: Option<u64>,
    pub error: Option<&'static str>,
}

pub struct Activity {
    preferences: Preferences,
    path: PathBuf,
    session: Option<Session>,
    skip_schedule: bool,
    error: Option<&'static str>,
}

impl Activity {
    pub fn new(preferences: Preferences, path: PathBuf) -> Self {
        Self {
            preferences,
            path,
            session: None,
            skip_schedule: false,
            error: None,
        }
    }

    pub fn preferences(&self) -> Preferences {
        self.preferences.clone()
    }

    pub fn save_preferences(&mut self, preferences: Preferences) -> Result<(), &'static str> {
        preferences::save(&self.path, &preferences)?;
        if self.preferences.schedule != preferences.schedule {
            self.skip_schedule = false;
            self.error = None;
        }
        self.preferences = preferences;
        self.tick();
        Ok(())
    }

    pub fn status(&self) -> Status {
        let (mode, seconds_left) = match self.session.as_ref().map(|session| &session.mode) {
            Some(Mode::Manual(deadline)) => (
                "manual",
                deadline.map(|end| {
                    let remaining = end.saturating_duration_since(Instant::now());
                    remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)
                }),
            ),
            Some(Mode::Scheduled) => ("scheduled", None),
            None => ("off", None),
        };
        Status {
            supported: platform::SUPPORTED,
            mode,
            seconds_left,
            elapsed_seconds: self
                .session
                .as_ref()
                .map(|session| session.started_at.elapsed().as_secs()),
            error: self.error,
        }
    }

    pub fn set_active(&mut self, active: bool) -> Status {
        self.error = None;
        if active && self.session.is_none() {
            let deadline = self
                .preferences
                .duration
                .seconds()
                .map(|seconds| Instant::now() + Duration::from_secs(seconds));
            self.start(Mode::Manual(deadline));
        } else if !active {
            self.session = None;
            self.skip_schedule = self.in_schedule();
        }
        self.status()
    }

    pub fn tick(&mut self) {
        let in_schedule = self.in_schedule();
        if !in_schedule {
            self.skip_schedule = false;
        }

        if let Some(session) = self.session.as_mut() {
            if !session.worker.is_running() {
                self.error = Some("workerExited");
                self.session = None;
                self.skip_schedule = in_schedule;
            } else {
                let finished = match session.mode {
                    Mode::Manual(Some(end)) => Instant::now() >= end,
                    Mode::Scheduled => !in_schedule,
                    Mode::Manual(None) => false,
                };
                if finished {
                    self.session = None;
                }
            }
        }

        if platform::SUPPORTED && in_schedule && !self.skip_schedule && self.session.is_none() {
            self.start(Mode::Scheduled);
            self.skip_schedule = self.session.is_none();
        }
    }

    fn in_schedule(&self) -> bool {
        self.preferences.schedule.contains(Local::now().time())
    }

    fn start(&mut self, mode: Mode) {
        match Worker::start() {
            Ok(worker) => {
                self.session = Some(Session {
                    worker,
                    mode,
                    started_at: Instant::now(),
                });
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}
