use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{CaffeinateSettings, RunningProcess};

    pub const SUPPORTED: bool = false;

    pub fn list_processes() -> Result<Vec<RunningProcess>, &'static str> {
        Err("caffeinateUnsupported")
    }

    pub struct Worker;

    impl Worker {
        pub fn start(_settings: &CaffeinateSettings) -> Result<Self, &'static str> {
            Err("caffeinateUnsupported")
        }

        pub fn is_running(&mut self) -> Result<bool, &'static str> {
            Ok(false)
        }

        pub fn reconfigure(
            &mut self,
            _settings: &CaffeinateSettings,
            _target_changed: bool,
        ) -> Result<(), &'static str> {
            Err("caffeinateUnsupported")
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CaffeinateMode {
    #[default]
    #[serde(alias = "command")]
    UntilStopped,
    Timer,
    Process,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct CaffeinateFlags {
    pub i: bool,
    pub d: bool,
    pub m: bool,
    pub s: bool,
    pub u: bool,
}

impl Default for CaffeinateFlags {
    fn default() -> Self {
        // A new installation stays awake on the charger and sleeps as usual on battery.
        Self {
            i: false,
            d: false,
            m: false,
            s: true,
            u: false,
        }
    }
}

impl CaffeinateFlags {
    fn enabled(&self) -> impl Iterator<Item = &'static str> {
        [
            (self.i, "-i"),
            (self.d, "-d"),
            (self.m, "-m"),
            (self.s, "-s"),
            (self.u, "-u"),
        ]
        .into_iter()
        .filter_map(|(enabled, flag)| enabled.then_some(flag))
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CaffeinateSettings {
    pub mode: CaffeinateMode,
    pub flags: CaffeinateFlags,
    pub seconds: u32,
    pub pid: Option<u32>,
}

impl Default for CaffeinateSettings {
    fn default() -> Self {
        Self {
            mode: CaffeinateMode::UntilStopped,
            flags: CaffeinateFlags::default(),
            seconds: 1800,
            pid: None,
        }
    }
}

impl CaffeinateSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.flags.enabled().next().is_none() {
            return Err("caffeinateNoFlags");
        }

        match self.mode {
            CaffeinateMode::Timer if self.seconds == 0 => Err("caffeinateInvalidDuration"),
            CaffeinateMode::Process
                if !self
                    .pid
                    .is_some_and(|pid| (1..=i32::MAX as u32).contains(&pid)) =>
            {
                Err("caffeinateInvalidPid")
            }
            _ => Ok(()),
        }
    }

    fn timeout(&self) -> Option<Duration> {
        (self.mode == CaffeinateMode::Timer).then(|| Duration::from_secs(self.seconds.into()))
    }

    fn same_target(&self, other: &Self) -> bool {
        self.mode == other.mode && (self.mode != CaffeinateMode::Process || self.pid == other.pid)
    }
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct RunningProcess {
    pub pid: u32,
    pub name: String,
}

pub fn list_processes() -> Result<Vec<RunningProcess>, &'static str> {
    platform::list_processes()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub mode: &'static str,
    pub supported: bool,
    pub seconds_left: Option<u64>,
    pub elapsed_seconds: Option<u64>,
    pub error: Option<&'static str>,
}

struct Session {
    worker: platform::Worker,
    started_at: Instant,
    settings: CaffeinateSettings,
}

pub struct Caffeinate {
    session: Option<Session>,
    error: Option<&'static str>,
}

impl Caffeinate {
    pub fn new() -> Self {
        Self {
            session: None,
            error: None,
        }
    }

    pub fn status(&mut self) -> Status {
        let mut status = Status {
            mode: "off",
            supported: platform::SUPPORTED,
            seconds_left: None,
            elapsed_seconds: None,
            error: self.error,
        };
        if let Some(session) = &mut self.session {
            match session.worker.is_running() {
                Ok(true) => {
                    let elapsed = session.started_at.elapsed();
                    status.mode = "manual";
                    status.elapsed_seconds = Some(elapsed.as_secs());
                    status.seconds_left = session.settings.timeout().map(|timeout| {
                        let remaining = timeout.saturating_sub(elapsed);
                        remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)
                    });
                }
                finished => {
                    self.error = finished.err();
                    self.session = None;
                    status.error = self.error;
                }
            }
        }
        status
    }

    pub fn set_active(&mut self, active: bool, settings: &CaffeinateSettings) -> Status {
        if !active {
            self.session = None;
            self.error = None;
        } else if self.status().mode == "off" {
            self.error = match self.start(settings) {
                Ok(session) => {
                    self.session = Some(session);
                    None
                }
                Err(error) => Some(error),
            };
        }
        self.status()
    }

    pub fn apply_settings(&mut self, settings: CaffeinateSettings) {
        self.status();
        let Some(session) = &mut self.session else {
            return;
        };

        self.error = None;
        let target_changed = !session.settings.same_target(&settings);
        let timer = settings.timeout();
        if !target_changed
            && session.settings.flags == settings.flags
            && session.settings.timeout() == timer
        {
            return;
        }

        if session
            .worker
            .reconfigure(&settings, target_changed)
            .is_err()
        {
            self.error = Some("caffeinateReconfigureFailed");
            return;
        }
        if target_changed || timer.is_some() {
            session.started_at = Instant::now();
        }
        session.settings = settings;
    }

    fn start(&self, settings: &CaffeinateSettings) -> Result<Session, &'static str> {
        if platform::SUPPORTED {
            settings.validate()?;
        }
        let worker = platform::Worker::start(settings)?;
        Ok(Session {
            worker,
            started_at: Instant::now(),
            settings: settings.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_without_caffeinate_settings_stay_awake_on_the_charger_only() {
        let settings: CaffeinateSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            serde_json::to_value(settings).unwrap(),
            serde_json::json!({
                "mode": "untilStopped",
                "flags": {"i": false, "d": false, "m": false, "s": true, "u": false},
                "seconds": 1800, "pid": null
            })
        );
    }

    #[test]
    fn legacy_command_migrates_without_retaining_executable_arguments() {
        let settings: CaffeinateSettings = serde_json::from_str(
            r#"{"mode":"command","executable":"/usr/bin/true","arguments":["old"],"flags":{"d":true}}"#,
        )
        .unwrap();
        assert!(matches!(settings.mode, CaffeinateMode::UntilStopped));
        assert!(settings.flags.d);
        let saved = serde_json::to_value(settings).unwrap();
        assert_eq!(saved["mode"], "untilStopped");
        assert!(saved.get("executable").is_none());
        assert!(saved.get("arguments").is_none());
    }

    #[test]
    fn timer_and_process_fields_are_checked_only_in_their_own_modes() {
        let mut settings = CaffeinateSettings {
            seconds: 0,
            pid: Some(u32::MAX),
            ..CaffeinateSettings::default()
        };
        assert_eq!(settings.validate(), Ok(()));
        settings.mode = CaffeinateMode::Timer;
        assert_eq!(settings.validate(), Err("caffeinateInvalidDuration"));
        settings.mode = CaffeinateMode::Process;
        assert_eq!(settings.validate(), Err("caffeinateInvalidPid"));
        settings.pid = Some(0);
        assert_eq!(settings.validate(), Err("caffeinateInvalidPid"));
        settings.pid = Some(i32::MAX as u32);
        assert_eq!(settings.validate(), Ok(()));
        settings.flags.s = false;
        assert_eq!(settings.validate(), Err("caffeinateNoFlags"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn active_timer_keeps_its_elapsed_time_until_an_effective_change() {
        let mut caffeinate = Caffeinate::new();
        let mut settings = CaffeinateSettings {
            mode: CaffeinateMode::Timer,
            ..CaffeinateSettings::default()
        };
        assert_eq!(caffeinate.set_active(true, &settings).mode, "manual");
        let started_at = Instant::now() - Duration::from_secs(60);
        caffeinate.session.as_mut().unwrap().started_at = started_at;

        caffeinate.apply_settings(settings.clone());
        assert_eq!(caffeinate.session.as_ref().unwrap().started_at, started_at);

        settings.seconds += 60;
        caffeinate.apply_settings(settings);
        let status = caffeinate.status();
        assert_eq!(status.mode, "manual");
        assert!(caffeinate.session.as_ref().unwrap().started_at > started_at);
    }
}
