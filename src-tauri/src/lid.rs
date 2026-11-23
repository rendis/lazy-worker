//! Keeps the Mac awake with its lid closed while Caffeinate is on.
//!
//! macOS has one system-wide switch for this, `pmset disablesleep`, and changing it needs root.
//! The user grants that once: a sudoers rule lets this user run those two `pmset` commands
//! without a password. After that, the app turns the switch on and off by itself.

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

const MARKER: &str = "lid-awake-managed";
// Reading the power source starts a process, so the charger is checked every few ticks.
const CHARGER_CHECK_TICKS: u8 = 5;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LidMode {
    /// The Mac sleeps when its lid closes, as macOS does by default.
    #[default]
    Sleep,
    /// Awake while the charger is connected.
    Charger,
    /// Awake on battery too.
    Always,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported: bool,
    /// The Mac already stays awake with its lid closed, and this app did not set that.
    pub external: bool,
}

/// Whether the system switch must be on.
fn wanted(mode: LidMode, caffeinate_active: bool, on_charger: bool) -> bool {
    caffeinate_active
        && match mode {
            LidMode::Sleep => false,
            LidMode::Charger => on_charger,
            LidMode::Always => true,
        }
}

pub struct Lid {
    marker: PathBuf,
    /// This app turned the system switch on, so it must turn it off again.
    /// The marker file keeps this across a forced exit.
    managed: bool,
    on_charger: bool,
    ticks: u8,
}

impl Lid {
    pub fn new(directory: PathBuf) -> Self {
        let marker = directory.join(MARKER);
        Self {
            managed: marker.exists(),
            marker,
            on_charger: false,
            ticks: 0,
        }
    }

    pub fn status(&self) -> Status {
        Status {
            supported: platform::SUPPORTED,
            external: !self.managed && platform::sleep_disabled(),
        }
    }

    /// Asks macOS for the one-time permission when it is missing.
    pub fn authorize() -> Result<(), &'static str> {
        if platform::authorized() {
            Ok(())
        } else {
            platform::authorize()
        }
    }

    /// Takes over a switch that was already on, so the chosen mode decides from now on.
    pub fn adopt(&mut self) {
        if !self.managed && platform::sleep_disabled() {
            self.set_managed(true);
        }
    }

    /// True when this app turned the system switch on and must turn it off again.
    pub fn managed(&self) -> bool {
        self.managed
    }

    /// Brings the system switch in line with the mode. Call it on every clock tick.
    /// Returns false when the switch had to change and macOS refused.
    pub fn reconcile(&mut self, mode: LidMode, caffeinate_active: bool) -> bool {
        if !platform::SUPPORTED {
            return true;
        }
        if mode == LidMode::Charger && caffeinate_active {
            if self.ticks == 0 {
                self.on_charger = platform::on_charger();
            }
            self.ticks = (self.ticks + 1) % CHARGER_CHECK_TICKS;
        } else {
            self.ticks = 0;
        }

        let wanted = wanted(mode, caffeinate_active, self.on_charger);
        if wanted && !self.managed {
            // A switch that is already on belongs to the user. Leave it as it is.
            if platform::sleep_disabled() {
                return true;
            }
            if platform::set_sleep_disabled(true).is_err() {
                return false;
            }
            self.set_managed(true);
        } else if !wanted && self.managed {
            if platform::set_sleep_disabled(false).is_err() {
                return false;
            }
            self.set_managed(false);
        }
        true
    }

    fn set_managed(&mut self, managed: bool) {
        self.managed = managed;
        let _ = if managed {
            fs::write(&self.marker, [])
        } else {
            fs::remove_file(&self.marker)
        };
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::process::{Command, Stdio};

    pub const SUPPORTED: bool = true;
    const PMSET: &str = "/usr/bin/pmset";
    const RULE: &str = "/etc/sudoers.d/lazy-worker-lid";

    fn output(program: &str, args: &[&str]) -> Option<String> {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn sleep_disabled() -> bool {
        output(PMSET, &["-g"]).is_some_and(|settings| super::parse_sleep_disabled(&settings))
    }

    pub fn on_charger() -> bool {
        output(PMSET, &["-g", "batt"]).is_some_and(|source| source.contains("AC Power"))
    }

    /// True when the rule is installed.
    /// `sudo -l` cannot answer this: it also succeeds for a command that still needs a password.
    pub fn authorized() -> bool {
        std::path::Path::new(RULE).exists()
    }

    pub fn set_sleep_disabled(disabled: bool) -> Result<(), ()> {
        let value = if disabled { "1" } else { "0" };
        output("/usr/bin/sudo", &["-n", PMSET, "-a", "disablesleep", value])
            .map(|_| ())
            .ok_or(())
    }

    /// Installs the sudoers rule through the macOS administrator prompt.
    /// `visudo` checks the rule first, so a bad file never reaches `/etc/sudoers.d`.
    pub fn authorize() -> Result<(), &'static str> {
        let user = std::env::var("USER").map_err(|_| "lidAuthorizationFailed")?;
        if !super::safe_user(&user) {
            return Err("lidAuthorizationFailed");
        }
        // The script holds no double quote or backslash, so it needs no AppleScript escaping.
        let script = format!(
            "tmp=$(/usr/bin/mktemp) && echo '{user} ALL=(root) NOPASSWD: {PMSET} -a disablesleep 0, {PMSET} -a disablesleep 1' > $tmp \
             && /usr/sbin/visudo -cf $tmp && /usr/bin/install -m 0440 -o root -g wheel $tmp {RULE}; \
             status=$?; /bin/rm -f $tmp; exit $status"
        );
        let prompt = "Lazy Worker needs permission to keep your Mac awake with the lid closed.";
        let result = Command::new("/usr/bin/osascript")
            .args(["-e", &format!("do shell script \"{script}\" with prompt \"{prompt}\" with administrator privileges")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .output()
            .map_err(|_| "lidAuthorizationFailed")?;
        if result.status.success() && authorized() {
            Ok(())
        } else if String::from_utf8_lossy(&result.stderr).contains("-128") {
            Err("lidAuthorizationCanceled")
        } else {
            Err("lidAuthorizationFailed")
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub const SUPPORTED: bool = false;

    pub fn sleep_disabled() -> bool {
        false
    }

    pub fn on_charger() -> bool {
        false
    }

    pub fn authorized() -> bool {
        false
    }

    pub fn set_sleep_disabled(_disabled: bool) -> Result<(), ()> {
        Err(())
    }

    pub fn authorize() -> Result<(), &'static str> {
        Err("lidAuthorizationFailed")
    }
}

/// Reads `SleepDisabled` from the output of `pmset -g`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_sleep_disabled(settings: &str) -> bool {
    settings.lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next() == Some("SleepDisabled") && words.next() == Some("1")
    })
}

/// A short user name can enter the sudoers rule as it is.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn safe_user(user: &str) -> bool {
    !user.is_empty()
        && user.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_wanted_only_while_caffeinate_is_on() {
        assert!(!wanted(LidMode::Always, false, true));
        assert!(!wanted(LidMode::Charger, false, true));
        assert!(!wanted(LidMode::Sleep, true, true));
        assert!(wanted(LidMode::Always, true, false));
    }

    #[test]
    fn charger_mode_follows_the_power_source() {
        assert!(wanted(LidMode::Charger, true, true));
        assert!(!wanted(LidMode::Charger, true, false));
    }

    #[test]
    fn reads_the_system_switch() {
        assert!(parse_sleep_disabled(
            " SleepDisabled\t\t1\n Sleep On Power Button 1\n"
        ));
        assert!(!parse_sleep_disabled(" SleepDisabled\t\t0\n"));
        assert!(!parse_sleep_disabled(
            " sleep                1 (sleep prevented by caffeinate)\n"
        ));
    }

    #[test]
    fn refuses_a_user_name_that_could_change_the_rule() {
        assert!(safe_user("rendis"));
        assert!(safe_user("first.last-2"));
        assert!(!safe_user("a, ALL"));
        assert!(!safe_user("a'b"));
        assert!(!safe_user(""));
    }
}
