//! The full shell, on both sides.
//!
//! On the target, a `Watch` follows the shell sessions and compares `~/.ssh/authorized_keys` before
//! and after, because a shell can leave a key behind. On the controller, a script opens the
//! hardened ssh command in Terminal. The script is the only place where a command line is written
//! as text, so every value in it is checked and single-quoted.

use super::{ensure_private_dir, ssh};
use crate::devices::KeysWarning;
use std::{
    collections::HashMap,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime},
};

/// How often the file is compared while a shell is open.
pub const CHECK_EVERY: Duration = Duration::from_secs(60);
/// The warnings that are kept. Older ones go first.
const MAX_WARNINGS: usize = 20;
/// A script that Terminal never ran is removed after this long.
const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

struct Session {
    name: String,
    count: usize,
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, Session>,
    /// The hash of the lines that are not ours, when the first shell began or the last change was seen.
    baseline: Option<String>,
    warnings: Vec<KeysWarning>,
    next: u64,
}

/// Follows the open shell sessions and the changes of the foreign lines of `authorized_keys`.
#[derive(Default)]
pub struct Watch(Mutex<State>);

impl Watch {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Notes a change since the baseline: one warning for each device that has a shell now.
    /// Returns who was warned. `digest` is nothing when the file could not be read.
    fn compare(state: &mut State, digest: Option<&str>, time: u64) -> Vec<(String, String)> {
        let Some(digest) = digest else { return Vec::new() };
        if state.baseline.as_deref() == Some(digest) {
            return Vec::new();
        }
        let had = state.baseline.replace(digest.to_owned()).is_some();
        if !had || state.sessions.is_empty() {
            return Vec::new();
        }
        let mut warned: Vec<(String, String)> = state
            .sessions
            .iter()
            .map(|(peer, session)| (peer.clone(), session.name.clone()))
            .collect();
        warned.sort();
        for (peer, name) in &warned {
            state.next += 1;
            let id = state.next;
            state.warnings.push(KeysWarning {
                id,
                peer: peer.clone(),
                name: name.clone(),
                time,
            });
        }
        if state.warnings.len() > MAX_WARNINGS {
            let extra = state.warnings.len() - MAX_WARNINGS;
            state.warnings.drain(..extra);
        }
        warned
    }

    /// A shell session began. The first one takes the baseline. Returns who was warned by a change that came before it.
    pub fn started(&self, peer: &str, name: &str, digest: Option<&str>, time: u64) -> Vec<(String, String)> {
        let mut state = self.lock();
        let warned = Self::compare(&mut state, digest, time);
        if state.sessions.is_empty() {
            state.baseline = digest.map(str::to_owned);
        }
        let session = state.sessions.entry(peer.to_owned()).or_insert(Session {
            name: name.to_owned(),
            count: 0,
        });
        session.count += 1;
        warned
    }

    /// The periodic check. Returns who was warned.
    pub fn check(&self, digest: Option<&str>, time: u64) -> Vec<(String, String)> {
        let mut state = self.lock();
        Self::compare(&mut state, digest, time)
    }

    /// A shell session ended. The file is compared before the session is dropped. Returns who was warned.
    pub fn ended(&self, peer: &str, digest: Option<&str>, time: u64) -> Vec<(String, String)> {
        let mut state = self.lock();
        let warned = Self::compare(&mut state, digest, time);
        if let Some(session) = state.sessions.get_mut(peer) {
            session.count = session.count.saturating_sub(1);
            if session.count == 0 {
                state.sessions.remove(peer);
            }
        }
        if state.sessions.is_empty() {
            state.baseline = None;
        }
        warned
    }

    pub fn active(&self) -> bool {
        !self.lock().sessions.is_empty()
    }

    pub fn warnings(&self) -> Vec<KeysWarning> {
        self.lock().warnings.clone()
    }

    pub fn dismiss(&self, id: u64) {
        self.lock().warnings.retain(|warning| warning.id != id);
    }
}

/// A value inside single quotes. A quote, a newline, or any control character makes it unusable,
/// because it could end the quote or start another command.
fn single_quote(value: &OsStr) -> Result<String, &'static str> {
    let text = value.to_str().ok_or("scriptValueInvalid")?;
    if text.chars().any(|c| c == '\'' || c.is_control()) {
        return Err("scriptValueInvalid");
    }
    Ok(format!("'{text}'"))
}

/// The program and its arguments on one line, each value in single quotes.
fn command_line(program: &OsStr, args: &[&OsStr]) -> Result<String, &'static str> {
    let mut line = single_quote(program)?;
    for arg in args {
        line.push(' ');
        line.push_str(&single_quote(arg)?);
    }
    Ok(line)
}

/// The text of the script: it removes itself, then replaces itself with ssh. The file is gone
/// before ssh starts, so nothing stays on disk whether ssh runs or not.
pub fn script_text(
    script: &Path,
    program: &OsStr,
    args: &[&OsStr],
) -> Result<String, &'static str> {
    Ok(format!(
        "#!/bin/sh\nrm -f -- {}\nexec {}\n",
        single_quote(script.as_os_str())?,
        command_line(program, args)?
    ))
}

/// The single line that a person can paste in any terminal to open the same shell. It holds paths and
/// options only: the key stays in its file, and the script's `rm` is not part of it.
pub fn copy_command(
    layout: &super::Layout,
    destination: &super::destination::Destination,
    peer: &super::identity::DeviceId,
) -> Result<String, &'static str> {
    let command = ssh::shell_command(layout, destination, peer)?;
    let args: Vec<&OsStr> = command.get_args().collect();
    command_line(command.get_program(), &args)
}

/// `command` as one word inside single quotes. A quote becomes `'\''`, so the shell reads the text
/// back exactly. Any other character, a newline included, is safe inside single quotes.
pub fn quote_text(command: &str) -> String {
    format!("'{}'", command.replace('\'', "'\\''"))
}

/// The line that runs an action's command the way an action runs: the login shell of the user, with
/// `-l`, and the command as the one argument of `-c`. The command is the text the user typed.
/// It is local, never from a remote origin.
pub fn action_line(shell: &Path, command: &str) -> Result<String, &'static str> {
    crate::actions::validate_command(command)?;
    Ok(format!("{} -l -c {}", single_quote(shell.as_os_str())?, quote_text(command)))
}

/// The script that tests a command in a terminal: it removes itself, runs the line, and waits for a
/// key, so the output can be read before the window closes. It does not `exec` the shell, because
/// it must print after the command ends.
pub fn test_script_text(script: &Path, shell: &Path, command: &str) -> Result<String, &'static str> {
    Ok(format!(
        "#!/bin/bash\nrm -f -- {}\n{}\nstatus=$?\nprintf '\\n[exit code %s] Press any key to close ' \"$status\"\nIFS= read -r -s -n 1 _\n",
        single_quote(script.as_os_str())?,
        action_line(shell, command)?
    ))
}

/// Writes the test script in `dir`, a private directory, as a new file with mode 0700.
pub fn write_test_script(dir: &Path, shell: &Path, command: &str) -> Result<PathBuf, &'static str> {
    ensure_private_dir(dir)?;
    sweep(dir);
    let path = dir.join(format!(
        "test-{}-{}.command",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let text = test_script_text(&path, shell, command)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)
        .map_err(|_| "remoteIo")?;
    file.write_all(text.as_bytes()).map_err(|_| {
        let _ = fs::remove_file(&path);
        "remoteIo"
    })?;
    Ok(path)
}

/// Opens a test of `command` in the default terminal of the user.
pub fn test_in_terminal(dir: &Path, shell: &Path, command: &str) -> Result<(), &'static str> {
    let script = write_test_script(dir, shell, command)?;
    open_in_terminal(&script)
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Writes the script in `dir`, a private directory, as a new file with mode 0700. Old scripts that
/// were never run are removed first. `command` is the ssh command of `ssh::shell_command`.
pub fn write_script(
    dir: &Path,
    id: &str,
    command: &std::process::Command,
) -> Result<PathBuf, &'static str> {
    ensure_private_dir(dir)?;
    sweep(dir);
    let path = dir.join(format!(
        "shell-{id}-{}-{}.command",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let args: Vec<&OsStr> = command.get_args().collect();
    let text = script_text(&path, command.get_program(), &args)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)
        .map_err(|_| "remoteIo")?;
    file.write_all(text.as_bytes()).map_err(|_| {
        let _ = fs::remove_file(&path);
        "remoteIo"
    })?;
    Ok(path)
}

/// Removes our scripts that are older than `STALE_AFTER`.
fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let ours = name
            .to_str()
            .is_some_and(|name| (name.starts_with("shell-") || name.starts_with("test-")) && name.ends_with(".command"));
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| SystemTime::now().duration_since(time).ok())
            .is_some_and(|age| age > STALE_AFTER);
        if ours && old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Opens the script with the app that macOS uses for `.command` files, which is the default terminal
/// of the user. The shell asks for a terminal, so it cannot run without one.
#[cfg(target_os = "macos")]
pub fn open_in_terminal(script: &Path) -> Result<(), &'static str> {
    use std::process::{Command, Stdio};
    let opened = Command::new("/usr/bin/open")
        .arg(script)
        .stdin(Stdio::null())
        .status();
    match opened {
        Ok(status) if status.success() => Ok(()),
        _ => {
            let _ = fs::remove_file(script);
            Err("terminalFailed")
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn open_in_terminal(script: &Path) -> Result<(), &'static str> {
    let _ = fs::remove_file(script);
    Err("unsupported")
}

/// Builds the script for one device and opens it in the default terminal.
pub fn open(
    layout: &super::Layout,
    destination: &super::destination::Destination,
    peer: &super::identity::DeviceId,
) -> Result<(), &'static str> {
    let command = ssh::shell_command(layout, destination, peer)?;
    let script = write_script(&layout.remote_dir().join("terminal"), peer.as_str(), &command)?;
    open_in_terminal(&script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{identity::DeviceId, testing::scratch};
    use std::os::unix::fs::MetadataExt;

    fn args(list: &[&str]) -> Vec<std::ffi::OsString> {
        list.iter().map(std::ffi::OsString::from).collect()
    }

    fn text(script: &str, program: &str, list: &[&str]) -> Result<String, &'static str> {
        let owned = args(list);
        let borrowed: Vec<&OsStr> = owned.iter().map(|arg| arg.as_os_str()).collect();
        script_text(Path::new(script), OsStr::new(program), &borrowed)
    }

    const TRICKY: [&str; 8] = [
        "echo hi",
        "echo 'it''s'",
        "echo \"$HOME\" `id` $(id)",
        "a\nb\n'c'\n",
        "'",
        "''",
        "printf '%s' \"a\\\"b\" ; echo $? && ls | wc",
        "é ü ñ 日本 \\ \t",
    ];

    #[test]
    fn a_command_is_quoted_so_the_shell_reads_it_back_exactly() {
        assert_eq!(quote_text("echo hi"), "'echo hi'");
        assert_eq!(quote_text("it's"), "'it'\\''s'");
        assert_eq!(quote_text("'"), "''\\'''");
        for command in TRICKY {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("printf %s {}", quote_text(command)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), command, "{command:?}");
        }
    }

    #[test]
    fn the_line_runs_the_login_shell_with_the_command_as_one_argument() {
        let shell = Path::new("/bin/zsh");
        assert_eq!(action_line(shell, "echo 'a'").unwrap(), "'/bin/zsh' -l -c 'echo '\\''a'\\'''");
        assert_eq!(action_line(shell, "echo $HOME").unwrap(), "'/bin/zsh' -l -c 'echo $HOME'");
        // The same limits as the store.
        assert_eq!(action_line(shell, ""), Err("actionInvalid"));
        assert_eq!(action_line(shell, "  \n"), Err("actionInvalid"));
        assert_eq!(action_line(shell, "a\0b"), Err("actionInvalid"));
        assert!(action_line(shell, &"a".repeat(4 * 1024)).is_ok());
        assert_eq!(action_line(shell, &"a".repeat(4 * 1024 + 1)), Err("actionInvalid"));
        // A shell path that could break the quote is refused.
        assert_eq!(action_line(Path::new("/bin/z'sh"), "ls"), Err("scriptValueInvalid"));
    }

    #[test]
    fn the_test_script_removes_itself_runs_the_line_and_waits_for_a_key() {
        let text = test_script_text(Path::new("/s/test-1.command"), Path::new("/bin/zsh"), "echo 'x' \"$y\"\n").unwrap();
        assert_eq!(
            text,
            "#!/bin/bash\nrm -f -- '/s/test-1.command'\n'/bin/zsh' -l -c 'echo '\\''x'\\'' \"$y\"\n'\n\
status=$?\nprintf '\\n[exit code %s] Press any key to close ' \"$status\"\nIFS= read -r -s -n 1 _\n"
        );
        assert_eq!(test_script_text(Path::new("/s/x.command"), Path::new("/bin/zsh"), "a\0"), Err("actionInvalid"));
    }

    #[test]
    fn the_test_script_is_private_new_and_runs_every_command_exactly() {
        let remote = scratch("test-script");
        let dir = remote.join("terminal");
        for (index, command) in TRICKY.iter().enumerate() {
            let path = write_test_script(&dir, Path::new("/bin/sh"), &format!("printf %s {}", quote_text(command))).unwrap();
            let meta = fs::metadata(&path).unwrap();
            assert_eq!(meta.mode() & 0o777, 0o700);
            assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
            // The text is the script that Terminal would run, with the key wait left out.
            let text = fs::read_to_string(&path).unwrap();
            assert!(text.starts_with("#!/bin/bash\nrm -f -- '"), "{index}");
            let line = text.lines().nth(2).unwrap_or_default();
            assert!(line.starts_with("'/bin/sh' -l -c "), "{index}");
            let _ = fs::remove_file(&path);
        }
        // Two scripts never share a path, and a script is never replaced.
        let first = write_test_script(&dir, Path::new("/bin/sh"), "ls").unwrap();
        let second = write_test_script(&dir, Path::new("/bin/sh"), "ls").unwrap();
        assert_ne!(first, second);
        assert_eq!(write_test_script(&dir, Path::new("/bin/z'sh"), "ls"), Err("scriptValueInvalid"));
    }

    #[test]
    fn the_script_removes_itself_and_runs_ssh_with_every_value_quoted() {
        let script = text(
            "/Users/me/Library/Application Support/x/terminal/shell-ab.command",
            "/usr/bin/ssh",
            &["-F", "/r s/ssh_config", "-t", "--", "fe80::1%en0", "shell"],
        )
        .unwrap();
        assert_eq!(
            script,
            "#!/bin/sh\nrm -f -- '/Users/me/Library/Application Support/x/terminal/shell-ab.command'\n\
exec '/usr/bin/ssh' '-F' '/r s/ssh_config' '-t' '--' 'fe80::1%en0' 'shell'\n"
        );
    }

    #[test]
    fn a_value_with_a_quote_a_newline_or_a_control_character_is_refused() {
        for bad in ["a'b", "a\nb", "a\rb", "a\u{7}b", "a\0b", "a\u{1b}[0m", "'; rm -rf ~; '", "\u{85}"] {
            assert_eq!(text("/s/x.command", "/usr/bin/ssh", &[bad]), Err("scriptValueInvalid"), "{bad:?}");
            assert_eq!(text("/s/x.command", bad, &["a"]), Err("scriptValueInvalid"), "{bad:?}");
            assert_eq!(text(bad, "/usr/bin/ssh", &["a"]), Err("scriptValueInvalid"), "{bad:?}");
        }
        // A character that the shell treats as syntax is harmless inside single quotes.
        let text = text("/s/x.command", "/usr/bin/ssh", &["$HOME `id` \"x\" ; & | \\"]).unwrap();
        assert!(text.contains("'$HOME `id` \"x\" ; & | \\'"));
    }

    #[test]
    fn the_script_for_a_real_destination_has_the_exact_text_and_a_private_file() {
        let remote = scratch("shell-script");
        let layout = crate::remote::Layout::for_paths(remote.clone(), None);
        let destination = crate::remote::destination::Destination::parse("me@studio.local:2222").unwrap();
        let id = DeviceId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let command = ssh::shell_command(&layout, &destination, &id).unwrap();
        let dir = remote.join("terminal");
        let path = write_script(&dir, id.as_str(), &command).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        let expected = format!(
            "#!/bin/sh\nrm -f -- '{}'\nexec '/usr/bin/ssh' '-F' '{}/ssh_config' '-o' 'HostKeyAlias=lw-{}' \
'-o' 'AddressFamily=inet' '-o' 'ControlMaster=no' '-t' '-o' 'RequestTTY=force' '-l' 'me' '-p' '2222' '--' 'studio.local' 'shell'\n",
            path.display(),
            remote.display(),
            id.as_str()
        );
        assert_eq!(written, expected);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        // The word after the destination is the fixed `shell`, and nothing else follows it.
        assert!(written.trim_end().ends_with("'studio.local' 'shell'"));
        // A second script never replaces the first.
        let second = write_script(&dir, id.as_str(), &command).unwrap();
        assert_ne!(path, second);
        // A path with a quote is refused and no file is written.
        let bad = crate::remote::Layout::for_paths(remote.join("it's"), None);
        let before = fs::read_dir(&dir).unwrap().count();
        let command = ssh::shell_command(&bad, &destination, &id);
        assert!(command.is_err() || write_script(&dir, id.as_str(), &command.unwrap()).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), before);
        fs::remove_dir_all(remote).unwrap();
    }

    #[test]
    fn the_copied_command_is_one_line_with_the_same_argv_and_no_key_material() {
        let remote = scratch("shell-copy");
        let layout = crate::remote::Layout::for_paths(remote.clone(), None);
        let destination = crate::remote::destination::Destination::parse("me@studio.local:2222").unwrap();
        let id = DeviceId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let line = copy_command(&layout, &destination, &id).unwrap();
        assert_eq!(
            line,
            format!(
                "'/usr/bin/ssh' '-F' '{}/ssh_config' '-o' 'HostKeyAlias=lw-{}' '-o' 'AddressFamily=inet' \
'-o' 'ControlMaster=no' '-t' '-o' 'RequestTTY=force' '-l' 'me' '-p' '2222' '--' 'studio.local' 'shell'",
                remote.display(),
                id.as_str()
            )
        );
        // The script is the same command with its own removal and `exec` in front.
        let command = ssh::shell_command(&layout, &destination, &id).unwrap();
        let path = write_script(&remote.join("terminal"), id.as_str(), &command).unwrap();
        assert!(fs::read_to_string(&path).unwrap().ends_with(&format!("exec {line}\n")));
        assert!(!line.contains('\n') && !line.contains("rm -f") && !line.contains("exec"));
        // Paths and options only: no key text, and the key file is never named.
        for word in ["PRIVATE KEY", "BEGIN", "ssh-ed25519", "ssh-rsa", "id_"] {
            assert!(!line.contains(word), "{word}");
        }
        // A value that could end the quote is refused.
        let bad = crate::remote::Layout::for_paths(remote.join("it's"), None);
        assert!(copy_command(&bad, &destination, &id).is_err());
        fs::remove_dir_all(remote).unwrap();
    }

    #[test]
    fn old_scripts_are_swept_and_other_files_stay() {
        let dir = scratch("shell-sweep");
        let old = dir.join("shell-x.command");
        let other = dir.join("notes.txt");
        fs::write(&old, "x").unwrap();
        fs::write(&other, "x").unwrap();
        let long_ago = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options().write(true).open(&old).unwrap().set_modified(long_ago).unwrap();
        fs::File::options().write(true).open(&other).unwrap().set_modified(long_ago).unwrap();
        sweep(&dir);
        assert!(!old.exists() && other.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn a_change_while_a_shell_is_open_warns_once_and_names_the_device() {
        let watch = Watch::default();
        assert!(watch.started(A, "Studio", Some("h1"), 10).is_empty());
        assert!(watch.active());
        assert!(watch.check(Some("h1"), 20).is_empty());
        assert_eq!(watch.check(Some("h2"), 30), vec![(A.to_owned(), "Studio".to_owned())]);
        // The same file does not warn again.
        assert!(watch.check(Some("h2"), 40).is_empty());
        let warnings = watch.warnings();
        assert_eq!((warnings.len(), warnings[0].peer.as_str(), warnings[0].name.as_str(), warnings[0].time), (1, A, "Studio", 30));
        // A change that is found at the end counts too.
        assert_eq!(watch.ended(A, Some("h3"), 50).len(), 1);
        assert!(!watch.active());
        assert_eq!(watch.warnings().len(), 2);
        let id = watch.warnings()[0].id;
        watch.dismiss(id);
        assert_eq!(watch.warnings().len(), 1);
        // With no shell open, a change is not a finding, and a read that failed is never one.
        assert!(watch.check(Some("h9"), 60).is_empty());
        assert!(watch.started(B, "Mini", None, 70).is_empty());
        assert!(watch.check(None, 80).is_empty());
        assert_eq!(watch.warnings().len(), 1);
    }

    #[test]
    fn two_shells_share_one_baseline_and_both_devices_are_named() {
        let watch = Watch::default();
        watch.started(A, "Studio", Some("h1"), 1);
        // A change before the second shell starts is found first, for the first device only.
        assert_eq!(watch.started(B, "Mini", Some("h2"), 2), vec![(A.to_owned(), "Studio".to_owned())]);
        assert_eq!(watch.check(Some("h3"), 3).len(), 2);
        assert!(watch.ended(A, Some("h3"), 4).is_empty());
        assert!(watch.active());
        assert!(watch.ended(B, Some("h3"), 5).is_empty());
        assert!(!watch.active());
    }

    #[test]
    fn the_warnings_are_limited() {
        let watch = Watch::default();
        watch.started(A, "Studio", Some("0"), 0);
        for index in 1..=(MAX_WARNINGS + 5) {
            watch.check(Some(&index.to_string()), index as u64);
        }
        let warnings = watch.warnings();
        assert_eq!(warnings.len(), MAX_WARNINGS);
        assert_eq!(warnings.last().unwrap().time, (MAX_WARNINGS + 5) as u64);
    }
}
