//! The applications installed on this device, for a device that has the Applications permission.
//!
//! An application is named by its bundle identifier. Only an identifier from the scanned list ever
//! reaches a path or a command, so text from a request never becomes either.

use serde::Serialize;
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
#[path = "apps/macos.rs"]
mod platform;

#[cfg(not(target_os = "macos"))]
#[path = "apps/unsupported.rs"]
mod platform;

/// The most applications that the list holds.
pub const MAX_APPS: usize = 1000;
const MAX_ID: usize = 255;
/// The scan reads the disk, so its answer is kept for this long.
const CACHE_FOR: Duration = Duration::from_secs(60);
/// The helper has this long to draw one icon, and may write this much.
const ICON_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ICON: usize = 512 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// One installed application. `path` stays on this device and is never sent.
#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub path: PathBuf,
}

/// An application as another device sees it.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub open: bool,
}

/// 1 to 255 characters of letters, digits, dots, and hyphens. It never starts with a hyphen, so
/// no command could read it as an option.
pub fn valid_bundle_id(text: &str) -> bool {
    (1..=MAX_ID).contains(&text.len())
        && !text.starts_with('-')
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
}

static CACHE: Mutex<Option<(Instant, Arc<Vec<App>>)>> = Mutex::new(None);

fn installed() -> Arc<Vec<App>> {
    let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
    if let Some((scanned, apps)) = cache.as_ref() {
        if scanned.elapsed() < CACHE_FOR {
            return Arc::clone(apps);
        }
    }
    let apps = Arc::new(platform::scan());
    *cache = Some((Instant::now(), Arc::clone(&apps)));
    apps
}

/// The application of the scanned list with this identifier.
fn find(bundle_id: &str) -> Result<App, &'static str> {
    if !valid_bundle_id(bundle_id) {
        return Err("appNotFound");
    }
    installed()
        .iter()
        .find(|app| app.bundle_id == bundle_id)
        .cloned()
        .ok_or("appNotFound")
}

/// Every installed application, with whether it is open now. It reads the disk and the system,
/// so call it off the main thread.
pub fn list() -> Result<Vec<Listed>, &'static str> {
    platform::supported()?;
    let open = platform::running();
    Ok(installed()
        .iter()
        .map(|app| Listed {
            bundle_id: app.bundle_id.clone(),
            name: app.name.clone(),
            version: app.version.clone(),
            open: open.contains(&app.bundle_id),
        })
        .collect())
}

/// The command that opens an application: the system `open` with the identifier and nothing else.
fn open_command(bundle_id: &str) -> Command {
    let mut command = Command::new("/usr/bin/open");
    command.args(["-b", bundle_id]);
    command
}

/// Opens an application of the scanned list. It brings an open one forward.
pub fn open(bundle_id: &str) -> Result<(), &'static str> {
    platform::supported()?;
    let app = find(bundle_id)?;
    let status = open_command(&app.bundle_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if status.is_ok_and(|status| status.success()) {
        Ok(())
    } else {
        Err("appOpenFailed")
    }
}

/// The icon of an application of the scanned list, as one PNG.
pub fn icon(bundle_id: &str) -> Result<Vec<u8>, &'static str> {
    platform::supported()?;
    let app = find(bundle_id)?;
    let png = platform::render_icon(&app.path)?;
    if png.starts_with(PNG_SIGNATURE) {
        Ok(png)
    } else {
        Err("appIconFailed")
    }
}

/// The icon as the text that goes in an answer.
#[cfg(unix)]
pub fn icon_base64(bundle_id: &str) -> Result<String, &'static str> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(icon(bundle_id)?))
}

#[cfg(not(unix))]
pub fn icon_base64(_: &str) -> Result<String, &'static str> {
    Err("unsupported")
}

/// Runs a program and returns what it writes to standard output. The program has `timeout` to end,
/// and more than `cap` bytes of output, or a failure status, is an error.
fn run_limited(
    mut command: Command,
    timeout: Duration,
    cap: usize,
) -> Result<Vec<u8>, &'static str> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "appIconFailed")?;
    let stdout = child.stdout.take().ok_or("appIconFailed")?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(cap as u64 + 1).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = reader.join().map_err(|_| "appIconFailed")?;
    match (status, bytes) {
        (Some(status), Ok(bytes)) if status.success() && !bytes.is_empty() && bytes.len() <= cap => {
            Ok(bytes)
        }
        _ => Err("appIconFailed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_id_is_short_and_made_of_safe_characters() {
        for good in ["com.apple.Safari", "com.rendis.lazy-worker", "a", "Foo.Bar-2"] {
            assert!(valid_bundle_id(good), "{good}");
        }
        let longest = "a".repeat(255);
        assert!(valid_bundle_id(&longest));
        for bad in [
            "", "-b", "--help", "com.apple Safari", "com/apple", "com.apple;ls", "a\nb",
            "com.apple.\u{43f}", "$(id)", "a`b`", "com.apple.Safari ", "../x", "a\0b",
        ] {
            assert!(!valid_bundle_id(bad), "{bad:?}");
        }
        assert!(!valid_bundle_id(&"a".repeat(256)));
    }

    #[test]
    fn opening_runs_the_system_open_with_the_bundle_id_and_nothing_else() {
        let command = open_command("com.apple.Safari");
        assert_eq!(command.get_program(), "/usr/bin/open");
        let arguments: Vec<_> = command.get_args().collect();
        assert_eq!(arguments, ["-b", "com.apple.Safari"]);
    }

    #[test]
    fn an_application_outside_the_list_is_not_found_and_nothing_runs() {
        for id in ["com.example.not-installed-anywhere", "-b", "", "a b"] {
            assert_eq!(open(id), Err("appNotFound"), "{id:?}");
            assert_eq!(icon(id), Err("appNotFound"), "{id:?}");
        }
    }

    #[test]
    fn a_program_that_is_slow_or_noisy_or_fails_gives_no_icon() {
        let sh = |script: &str| {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            command
        };
        let quick = Duration::from_secs(5);
        assert_eq!(run_limited(sh("printf abc"), quick, 10), Ok(b"abc".to_vec()));
        assert_eq!(run_limited(sh("printf abcdefghijkl"), quick, 10), Err("appIconFailed"));
        assert_eq!(run_limited(sh("printf abc; exit 1"), quick, 10), Err("appIconFailed"));
        assert_eq!(run_limited(sh("exit 0"), quick, 10), Err("appIconFailed"));
        let started = Instant::now();
        assert_eq!(
            run_limited(sh("sleep 5"), Duration::from_millis(200), 10),
            Err("appIconFailed")
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        // A program that fills the pipe and never ends is cut at the time limit too.
        assert_eq!(
            run_limited(sh("yes"), Duration::from_millis(300), 1024),
            Err("appIconFailed")
        );
    }
}
