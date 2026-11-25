//! The system SSH client, run with a configuration of our own.
//!
//! Every call passes `-F` with our file, so nothing in `~/.ssh/config` or `/etc/ssh` is read.
//! Only a validated `Destination` and a word from a closed list reach the arguments, and `--`
//! stands before the host.

use super::{destination::Destination, identity::DeviceId, known_hosts, write_temp, Layout};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const STDOUT_CAP: usize = 1024 * 1024;
const STDERR_CAP: usize = 8 * 1024;
pub const MAX_INPUT: usize = 1024 * 1024;
const SSH: &str = "/usr/bin/ssh";

/// What the remote side runs. The words are fixed here and never come from the caller's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteCommand {
    Dispatch,
    Pair,
    /// An interactive shell. The target honours it only while the user there has granted it.
    Shell,
}

impl RemoteCommand {
    fn word(self) -> &'static str {
        match self {
            Self::Dispatch => "dispatch",
            Self::Pair => "pair",
            Self::Shell => "shell",
        }
    }
}

/// Which key and which connection sharing a config file sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Profile {
    /// The key of this device, with a master connection shared by the calls of one peer.
    Device,
    /// The fixed pairing key alone, and a new connection each time. The key of this device is
    /// never offered to a host that pairing has not yet bound to its device.
    Pairing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Remote(RemoteCommand),
    /// Ask the multiplexing master of the destination to exit.
    ControlExit,
    /// Print the settings that would apply, and connect to nothing.
    PrintConfig(Profile),
}

impl Mode {
    fn profile(self) -> Profile {
        match self {
            Self::Remote(RemoteCommand::Pair) => Profile::Pairing,
            Self::PrintConfig(profile) => profile,
            Self::Remote(RemoteCommand::Dispatch | RemoteCommand::Shell) | Self::ControlExit => {
                Profile::Device
            }
        }
    }
}

/// One option of our config file.
struct Opt {
    name: &'static str,
    value: String,
    /// Dropping it would weaken the rules, so a client without it is refused.
    required: bool,
}

fn opt(name: &'static str, value: &str, required: bool) -> Opt {
    Opt {
        name,
        value: value.to_owned(),
        required,
    }
}

/// Quotes a path for the config file. A literal `%` is doubled because ssh expands `%` tokens.
/// A path that could end the quote, start an expansion, or break the line is refused.
fn quote(path: &Path, suffix: &str) -> Result<String, &'static str> {
    let text = path.to_str().ok_or("configPathInvalid")?;
    let bad = |c: char| c.is_control() || matches!(c, '"' | '\\' | '$');
    if !path.is_absolute() || text.chars().any(bad) {
        return Err("configPathInvalid");
    }
    Ok(format!("\"{}{suffix}\"", text.replace('%', "%%")))
}

fn options(layout: &Layout, profile: Profile) -> Result<Vec<Opt>, &'static str> {
    let identity = match profile {
        Profile::Device => layout.private_key(),
        Profile::Pairing => layout.pairing_key_file(),
    };
    let mut list = vec![
        opt("BatchMode", "yes", true),
        opt("StrictHostKeyChecking", "yes", true),
        Opt {
            name: "UserKnownHostsFile",
            value: quote(&layout.known_hosts_file(), "")?,
            required: true,
        },
        opt("GlobalKnownHostsFile", "/dev/null", true),
        opt("UpdateHostKeys", "no", false),
        opt("CheckHostIP", "no", false),
        opt("HashKnownHosts", "no", false),
        Opt {
            name: "IdentityFile",
            value: quote(&identity, "")?,
            required: true,
        },
        opt("IdentitiesOnly", "yes", true),
        opt("IdentityAgent", "none", false),
        opt("AddKeysToAgent", "no", false),
        opt("UseKeychain", "no", false),
        opt("PubkeyAuthentication", "yes", true),
        opt("PasswordAuthentication", "no", true),
        opt("KbdInteractiveAuthentication", "no", false),
        // The name that OpenSSH before 8.7 knows. It is an alias of the one above after that.
        opt("ChallengeResponseAuthentication", "no", false),
        opt("PreferredAuthentications", "publickey", true),
        opt("ForwardAgent", "no", true),
        opt("ForwardX11", "no", true),
        opt("ClearAllForwardings", "yes", true),
        opt("PermitLocalCommand", "no", true),
        opt("ProxyCommand", "none", true),
        opt("Tunnel", "no", false),
        opt("RequestTTY", "no", false),
        opt("ConnectTimeout", "5", false),
        opt("ServerAliveInterval", "15", false),
        opt("ServerAliveCountMax", "2", false),
    ];
    match (profile, layout.control_dir()) {
        (Profile::Device, Some(directory)) => {
            // The socket path is passed per call, named by the peer. See `build`.
            quote(directory, "")?;
            list.push(opt("ControlMaster", "auto", false));
            list.push(opt("ControlPersist", "60", false));
        }
        _ => list.push(opt("ControlMaster", "no", true)),
    }
    list.push(opt("LogLevel", "ERROR", false));
    list.push(opt("VisualHostKey", "no", false));
    Ok(list)
}

/// The text of the config file, without the options named in `dropped` (lowercase).
fn render_config(
    layout: &Layout,
    profile: Profile,
    dropped: &[String],
) -> Result<String, &'static str> {
    let mut text = String::from("# Written by Lazy Worker. Changes are replaced.\nHost *\n");
    for option in options(layout, profile)? {
        if !dropped
            .iter()
            .any(|name| name.eq_ignore_ascii_case(option.name))
        {
            text.push_str(&format!("    {} {}\n", option.name, option.value));
        }
    }
    Ok(text)
}

/// The names of the options that ssh reports as unknown or unsupported, in lowercase.
fn rejected_options(stderr: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in stderr.lines() {
        let name = if let Some((_, rest)) = line.split_once("Bad configuration option: ") {
            rest.split_whitespace().next()
        } else if let Some((_, rest)) = line.split_once("Unsupported option \"") {
            rest.split('"').next()
        } else {
            None
        };
        if let Some(name) = name {
            names.push(name.to_ascii_lowercase());
        }
    }
    names
}

/// Writes our config file. The local ssh checks it first (`ssh -G`), and an option that it does
/// not know is left out. Returns the names that were left out, in lowercase. A missing option
/// that guards the rules is an error instead.
pub fn write_config(layout: &Layout) -> Result<Vec<String>, &'static str> {
    write_profile(layout, Profile::Device)
}

/// Writes the config file of a pairing. It offers the pairing key only.
pub fn write_pairing_config(layout: &Layout) -> Result<Vec<String>, &'static str> {
    write_profile(layout, Profile::Pairing)
}

fn write_profile(layout: &Layout, profile: Profile) -> Result<Vec<String>, &'static str> {
    let mut dropped: Vec<String> = Vec::new();
    let path = match profile {
        Profile::Device => layout.config_file(),
        Profile::Pairing => layout.pairing_config_file(),
    };
    for _ in 0..32 {
        let temp = write_temp(&path, render_config(layout, profile, &dropped)?.as_bytes())?;
        let stderr = match probe(layout, profile, &temp) {
            Ok(stderr) => stderr,
            Err(code) => {
                let _ = fs::remove_file(&temp);
                return Err(code);
            }
        };
        let rejected = rejected_options(&stderr);
        if rejected.is_empty() {
            return match fs::rename(&temp, &path) {
                Ok(()) => Ok(dropped),
                Err(_) => {
                    let _ = fs::remove_file(&temp);
                    Err("remoteIo")
                }
            };
        }
        let _ = fs::remove_file(&temp);
        let known = options(layout, profile)?;
        for name in rejected {
            let Some(option) = known
                .iter()
                .find(|option| option.name.eq_ignore_ascii_case(&name))
            else {
                return Err("sshConfigInvalid");
            };
            if option.required {
                return Err("sshConfigUnsupported");
            }
            if !dropped.contains(&name) {
                dropped.push(name);
            }
        }
    }
    Err("sshConfigInvalid")
}

/// Runs `ssh -G` on a candidate file. Gives its error text, or a code when ssh cannot be run or
/// fails for a reason other than an unknown option.
fn probe(layout: &Layout, profile: Profile, candidate: &Path) -> Result<String, &'static str> {
    let peer = DeviceId::parse(&"0".repeat(32))?;
    let destination = Destination::new("probe", "example.invalid", None)?;
    // The same arguments as a real call, with the candidate in place of our config file.
    let mut args: Vec<_> = build(layout, &destination, &peer, Mode::PrintConfig(profile))?
        .get_args()
        .map(ToOwned::to_owned)
        .collect();
    args[1] = candidate.as_os_str().to_owned();
    let mut command = tool(SSH);
    command.args(args);
    let done = execute(command, &[], Duration::from_secs(5))?;
    if done.timed_out {
        return Err("sshTimeout");
    }
    if !done.success && rejected_options(&done.stderr).is_empty() {
        return Err("sshConfigInvalid");
    }
    Ok(done.stderr)
}

/// A command for a system tool: fixed path, an environment of `PATH` and `HOME` only, and
/// piped streams.
pub(super) fn tool(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    command
}

/// True for a multicast DNS name: a host that ends in `.local`, with or without the final dot.
fn is_local_name(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host.ends_with(".local") || host.ends_with(".local.")
}

fn build(
    layout: &Layout,
    destination: &Destination,
    peer: &DeviceId,
    mode: Mode,
) -> Result<Command, &'static str> {
    let profile = mode.profile();
    let mut command = tool(SSH);
    command
        .arg("-F")
        .arg(match profile {
            Profile::Device => layout.config_file(),
            Profile::Pairing => layout.pairing_config_file(),
        })
        .arg("-o")
        .arg(format!("HostKeyAlias={}", known_hosts::alias(peer)));
    // A `.local` name often resolves to an IPv6 link-local address first. The `from=` of the key on the
    // other device never matches that address, so these names connect over IPv4 only.
    if is_local_name(destination.host()) {
        command.arg("-o").arg("AddressFamily=inet");
    }
    // One socket per peer, so a live master never carries a request meant for another device
    // that took over the address. The name is the validated id, with no `%` token in it. A
    // pairing shares no connection.
    // A shell never shares a connection. The line of the key is checked when a connection is made,
    // so a shared one could carry a terminal that a grant no longer allows.
    let shell = matches!(mode, Mode::Remote(RemoteCommand::Shell));
    if let (Profile::Device, Some(directory), false) = (profile, layout.control_dir(), shell) {
        command.arg("-o").arg(format!(
            "ControlPath={}",
            quote(directory, &format!("/{}", peer.as_str()))?
        ));
    }
    match mode {
        Mode::Remote(RemoteCommand::Shell) => {
            command.args(["-o", "ControlMaster=no", "-t", "-o", "RequestTTY=force"]);
        }
        Mode::Remote(_) => {}
        Mode::ControlExit => {
            command.args(["-O", "exit"]);
        }
        Mode::PrintConfig(_) => {
            command.arg("-G");
        }
    }
    command
        .arg("-l")
        .arg(destination.user())
        .arg("-p")
        .arg(destination.port().to_string())
        .arg("--")
        .arg(destination.host());
    if let Mode::Remote(remote) = mode {
        command.arg(remote.word());
    }
    Ok(command)
}

/// The command that runs one remote word on the destination. Data travels on stdin.
pub fn command(
    layout: &Layout,
    destination: &Destination,
    peer: &DeviceId,
    remote: RemoteCommand,
) -> Result<Command, &'static str> {
    build(layout, destination, peer, Mode::Remote(remote))
}

/// The command that opens an interactive shell on the destination, with a terminal and no shared connection.
pub fn shell_command(
    layout: &Layout,
    destination: &Destination,
    peer: &DeviceId,
) -> Result<Command, &'static str> {
    build(layout, destination, peer, Mode::Remote(RemoteCommand::Shell))
}

/// Closes the multiplexing master of a destination, for example when its device is removed.
/// Succeeds when there is none.
pub fn stop_master(
    layout: &Layout,
    destination: &Destination,
    peer: &DeviceId,
) -> Result<(), &'static str> {
    let done = execute(
        build(layout, destination, peer, Mode::ControlExit)?,
        &[],
        Duration::from_secs(5),
    )?;
    if done.timed_out {
        return Err("sshTimeout");
    }
    if done.success || done.stderr.contains("Control socket") {
        return Ok(());
    }
    Err("sshFailed")
}

pub(super) struct Completed {
    pub success: bool,
    #[cfg_attr(not(test), allow(dead_code))] // Read by `run`.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub timed_out: bool,
    #[cfg_attr(not(test), allow(dead_code))] // Read by `run`.
    pub truncated: bool,
}

/// Collects a stream in a thread. `done` fires at the end of the stream.
struct Capture {
    data: Arc<Mutex<Vec<u8>>>,
    over: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
}

fn capture(mut stream: impl Read + Send + 'static, cap: usize, stop_when_over: bool) -> Capture {
    let data = Arc::new(Mutex::new(Vec::new()));
    let over = Arc::new(AtomicBool::new(false));
    let (sender, done) = mpsc::channel();
    let (shared, flag) = (Arc::clone(&data), Arc::clone(&over));
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(read) = stream.read(&mut chunk) {
            if read == 0 {
                break;
            }
            let Ok(mut buffer) = shared.lock() else { break };
            let room = cap.saturating_sub(buffer.len());
            buffer.extend_from_slice(&chunk[..read.min(room)]);
            if read > room {
                flag.store(true, Ordering::Release);
                if stop_when_over {
                    break;
                }
            }
        }
        let _ = sender.send(());
    });
    Capture { data, over, done }
}

/// Runs the command with `input` on stdin. Output is capped, and the process is killed after
/// `timeout` or when stdout passes its cap.
pub(super) fn execute(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
) -> Result<Completed, &'static str> {
    if input.len() > MAX_INPUT {
        return Err("sshInputTooLarge");
    }
    let mut child = command.spawn().map_err(|_| "sshFailed")?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_vec();
        // A thread writes, so a child that does not read cannot block us.
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let stdout = capture(child.stdout.take().ok_or("sshFailed")?, STDOUT_CAP, true);
    let stderr = capture(child.stderr.take().ok_or("sshFailed")?, STDERR_CAP, false);
    let start = Instant::now();
    let (mut timed_out, mut truncated) = (false, false);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                return Err("sshFailed");
            }
        }
        truncated = stdout.over.load(Ordering::Acquire);
        timed_out = start.elapsed() >= timeout;
        if truncated || timed_out {
            let _ = child.kill();
            break child.wait().map_err(|_| "sshFailed")?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    // A background master may hold the pipes for a moment, so the wait for their end is short.
    let _ = stdout.done.recv_timeout(Duration::from_secs(2));
    let _ = stderr.done.recv_timeout(Duration::from_secs(2));
    let take = |capture: &Capture| {
        capture
            .data
            .lock()
            .map(|data| data.clone())
            .unwrap_or_default()
    };
    let truncated = truncated || stdout.over.load(Ordering::Acquire);
    Ok(Completed {
        success: status.success() && !timed_out && !truncated,
        code: status.code(),
        stdout: take(&stdout),
        stderr: String::from_utf8_lossy(&take(&stderr)).into_owned(),
        timed_out,
        truncated,
    })
}

/// Runs a command built by this module and returns its stdout. The error is a code only. The
/// text of stderr is never passed on.
#[cfg_attr(not(test), allow(dead_code))] // Only tests run a one-shot call. Sessions go through `client`.
pub fn run(command: Command, input: &[u8], timeout: Duration) -> Result<Vec<u8>, &'static str> {
    let done = execute(command, input, timeout)?;
    if done.timed_out {
        return Err("sshTimeout");
    }
    if done.truncated {
        return Err("sshOutputTooLarge");
    }
    if done.success {
        return Ok(done.stdout);
    }
    // 255 is ssh itself failing. Any other code is the remote program, whose text we do not read.
    Err(match done.code {
        Some(255) => classify_stderr(&done.stderr),
        _ => "sshFailed",
    })
}

/// Maps the error text of OpenSSH to a code.
pub fn classify_stderr(stderr: &str) -> &'static str {
    let text = stderr.to_ascii_lowercase();
    let has = |needle: &str| text.contains(needle);
    if has("host key is known") {
        "sshHostKeyUnknown"
    } else if has("host identification has changed")
        || has("host key verification failed")
        || has("offending")
    {
        "sshHostKeyChanged"
    } else if has("permission denied") {
        "sshDenied"
    } else if has("timed out") {
        "sshTimeout"
    } else if has("connection refused")
        || has("no route to host")
        || has("could not resolve")
        || has("network is unreachable")
        || has("host is down")
        || has("connection closed")
        || has("connection reset")
    {
        "sshUnreachable"
    } else {
        "sshFailed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{ensure_private_dir, testing::scratch};
    use std::{ffi::OsStr, path::PathBuf};

    fn peer() -> DeviceId {
        DeviceId::parse(&"ab".repeat(16)).unwrap()
    }

    fn layout_at(remote: &str, control: Option<&str>) -> Layout {
        Layout::for_paths(PathBuf::from(remote), control.map(PathBuf::from))
    }

    #[test]
    fn config_quotes_paths_and_escapes_percent() {
        let layout = layout_at(
            "/Users/me/Library/Application Support/app 100%/remote",
            Some("/tmp/lw-501"),
        );
        let text = render_config(&layout, Profile::Device, &[]).unwrap();
        assert!(text.contains(
            "    IdentityFile \"/Users/me/Library/Application Support/app 100%%/remote/id_ed25519\"\n"
        ));
        assert!(text.contains(
            "    UserKnownHostsFile \"/Users/me/Library/Application Support/app 100%%/remote/known_hosts\"\n"
        ));
        assert!(!text.contains("ControlPath"), "the socket path is per call");
        assert!(text.contains("    ControlMaster auto\n"));
        assert!(text.starts_with("# Written by Lazy Worker. Changes are replaced.\nHost *\n"));
    }

    #[test]
    fn the_pairing_config_offers_the_pairing_key_alone_and_shares_no_connection() {
        let layout = layout_at("/r s/100%", Some("/tmp/lw-501"));
        let text = render_config(&layout, Profile::Pairing, &[]).unwrap();
        assert_eq!(text.matches("IdentityFile").count(), 1);
        assert!(text.contains("    IdentityFile \"/r s/100%%/pairing_key\"\n"));
        assert!(!text.contains("id_ed25519"));
        assert!(text.contains("    IdentitiesOnly yes\n"));
        assert!(text.contains("    ControlMaster no\n"));
        assert!(!text.contains("ControlPersist") && !text.contains("ControlPath"));
        for kept in ["BatchMode yes", "StrictHostKeyChecking yes", "PasswordAuthentication no"] {
            assert!(text.contains(kept), "{kept}");
        }
    }

    #[test]
    fn config_without_a_socket_path_turns_multiplexing_off() {
        let text = render_config(&layout_at("/r", None), Profile::Device, &[]).unwrap();
        assert!(text.contains("    ControlMaster no\n"));
        assert!(!text.contains("ControlPath") && !text.contains("ControlPersist"));
    }

    #[test]
    fn config_refuses_paths_that_could_break_out() {
        for path in [
            "/a\"b",
            "/a\nb",
            "/a\\b",
            "/a$HOME",
            "/a\u{7}b",
            "relative/path",
            "~/x",
            "/a\rb",
        ] {
            assert_eq!(
                render_config(&layout_at(path, None), Profile::Device, &[]).unwrap_err(),
                "configPathInvalid",
                "{path:?}"
            );
        }
    }

    #[test]
    fn dropped_options_leave_the_config() {
        let layout = layout_at("/r", None);
        let dropped = [
            "usekeychain".to_owned(),
            "KBDINTERACTIVEAUTHENTICATION".to_owned(),
        ];
        let text = render_config(&layout, Profile::Device, &dropped).unwrap();
        assert!(!text.contains("UseKeychain") && !text.contains("KbdInteractive"));
        assert!(
            text.contains("BatchMode yes") && text.contains("ChallengeResponseAuthentication no")
        );
    }

    #[test]
    fn rejected_options_are_read_from_ssh_messages() {
        let stderr =
            "/r/ssh_config: line 18: Bad configuration option: KbdInteractiveAuthentication\n\
            /r/ssh_config: line 12: Unsupported option \"usekeychain\"\n\
            /r/ssh_config: terminating, 1 bad configuration options\n";
        assert_eq!(
            rejected_options(stderr),
            ["kbdinteractiveauthentication", "usekeychain"]
        );
        assert!(rejected_options("Warning: Permanently added\n").is_empty());
    }

    #[test]
    fn argv_is_exact() {
        let layout = layout_at("/r s", Some("/tmp/lw-501"));
        let destination = Destination::parse("me@[fe80::1%en0]:2222").unwrap();
        let args = |mode| -> Vec<String> {
            build(&layout, &destination, &peer(), mode)
                .unwrap()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        let expect = |middle: &[&str], word: Option<&str>| -> Vec<String> {
            let mut all: Vec<String> = ["-F", "/r s/ssh_config", "-o"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            all.push(format!("HostKeyAlias=lw-{}", "ab".repeat(16)));
            all.extend([
                "-o".to_owned(),
                format!("ControlPath=\"/tmp/lw-501/{}\"", "ab".repeat(16)),
            ]);
            all.extend(middle.iter().map(|s| s.to_string()));
            all.extend(
                ["-l", "me", "-p", "2222", "--", "fe80::1%en0"]
                    .iter()
                    .map(|s| s.to_string()),
            );
            all.extend(word.map(str::to_owned));
            all
        };
        assert_eq!(
            args(Mode::Remote(RemoteCommand::Dispatch)),
            expect(&[], Some("dispatch"))
        );
        assert_eq!(args(Mode::ControlExit), expect(&["-O", "exit"], None));
        assert_eq!(
            args(Mode::PrintConfig(Profile::Device)),
            expect(&["-G"], None)
        );
        // A pairing has its own config file and no socket path.
        let pairing: Vec<String> = [
            "-F",
            "/r s/ssh_config_pair",
            "-o",
            &format!("HostKeyAlias=lw-{}", "ab".repeat(16)),
            "-l",
            "me",
            "-p",
            "2222",
            "--",
            "fe80::1%en0",
            "pair",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(args(Mode::Remote(RemoteCommand::Pair)), pairing);
        // A shell asks for a terminal, never shares a connection, and has no socket path.
        let shell: Vec<String> = [
            "-F",
            "/r s/ssh_config",
            "-o",
            &format!("HostKeyAlias=lw-{}", "ab".repeat(16)),
            "-o",
            "ControlMaster=no",
            "-t",
            "-o",
            "RequestTTY=force",
            "-l",
            "me",
            "-p",
            "2222",
            "--",
            "fe80::1%en0",
            "shell",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(args(Mode::Remote(RemoteCommand::Shell)), shell);
        assert!(!shell.iter().any(|arg| arg == "AddressFamily=inet"));
        let command = command(&layout, &destination, &peer(), RemoteCommand::Dispatch).unwrap();
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/ssh"));
        let mut names: Vec<_> = command
            .get_envs()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert!(names == ["HOME", "PATH"] || names == ["PATH"], "{names:?}");
        let path = command
            .get_envs()
            .find(|(name, _)| *name == "PATH")
            .and_then(|(_, value)| value);
        assert_eq!(path, Some(OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin")));
    }

    #[test]
    fn a_local_name_connects_over_ipv4_in_every_profile() {
        let layout = layout_at("/r s", Some("/tmp/lw-501"));
        let modes = [
            Mode::Remote(RemoteCommand::Dispatch),
            Mode::Remote(RemoteCommand::Pair),
            Mode::Remote(RemoteCommand::Shell),
            Mode::ControlExit,
            Mode::PrintConfig(Profile::Device),
            Mode::PrintConfig(Profile::Pairing),
        ];
        let family = |input: &str, mode| -> Vec<String> {
            let destination = Destination::parse(input).unwrap();
            let args: Vec<String> = build(&layout, &destination, &peer(), mode)
                .unwrap()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            args.windows(2)
                .filter(|pair| pair[0] == "-o" && pair[1].starts_with("AddressFamily"))
                .map(|pair| pair[1].clone())
                .collect()
        };
        for mode in modes {
            for host in ["me@studio.local", "me@Studio.LOCAL.", "me@studio.local:2222"] {
                assert_eq!(family(host, mode), ["AddressFamily=inet"], "{host}");
            }
            for host in ["me@100.64.1.2", "me@[fe80::1%en0]", "me@studio.example.com", "me@mylocal", "me@local.example.com"] {
                assert!(family(host, mode).is_empty(), "{host}");
            }
        }
        // The option comes right after the host key alias, before the socket path.
        let args: Vec<String> = build(&layout, &Destination::parse("me@studio.local").unwrap(), &peer(), Mode::Remote(RemoteCommand::Dispatch))
            .unwrap()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(&args[4..7], ["-o", "AddressFamily=inet", "-o"]);
    }

    #[test]
    fn peers_at_one_destination_get_different_sockets_and_percent_is_escaped() {
        let destination = Destination::parse("me@imac.local").unwrap();
        let other = DeviceId::parse(&"cd".repeat(16)).unwrap();
        let socket = |layout: &Layout, peer: &DeviceId| -> String {
            let command = command(layout, &destination, peer, RemoteCommand::Dispatch).unwrap();
            let args: Vec<_> = command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            args.into_iter()
                .find(|a| a.starts_with("ControlPath="))
                .unwrap()
        };
        let layout = layout_at("/r", Some("/tmp/lw 100%"));
        let (a, b) = (socket(&layout, &peer()), socket(&layout, &other));
        assert_ne!(a, b);
        assert!(
            a.ends_with(&format!("/{}\"", "ab".repeat(16)))
                && b.ends_with(&format!("/{}\"", "cd".repeat(16)))
        );
        assert!(a.starts_with("ControlPath=\"/tmp/lw 100%%/"), "{a}");
        let none = command(
            &layout_at("/r", None),
            &destination,
            &peer(),
            RemoteCommand::Dispatch,
        )
        .unwrap();
        assert!(none
            .get_args()
            .all(|a| !a.to_string_lossy().starts_with("ControlPath")));
    }

    #[test]
    fn stderr_is_classified_into_codes() {
        let cases = [
            ("@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n@@@@\nHost key verification failed.", "sshHostKeyChanged"),
            ("Offending ED25519 key in /r/known_hosts:1\nHost key verification failed.", "sshHostKeyChanged"),
            ("No ED25519 host key is known for lw-ab and you have requested strict checking.\nHost key verification failed.", "sshHostKeyUnknown"),
            ("me@imac.local: Permission denied (publickey).", "sshDenied"),
            ("ssh: connect to host imac.local port 22: Operation timed out", "sshTimeout"),
            ("Connection timed out during banner exchange", "sshTimeout"),
            ("ssh: connect to host 10.0.0.9 port 22: Connection refused", "sshUnreachable"),
            ("ssh: connect to host 10.0.0.9 port 22: No route to host", "sshUnreachable"),
            ("ssh: Could not resolve hostname imac.local: nodename nor servname provided, or not known", "sshUnreachable"),
            ("Connection closed by 10.0.0.9 port 22", "sshUnreachable"),
            ("kex_exchange_identification: read: Connection reset by peer", "sshUnreachable"),
            ("", "sshFailed"),
            ("something else", "sshFailed"),
        ];
        for (text, code) in cases {
            assert_eq!(classify_stderr(text), code, "{text}");
        }
    }

    fn sh(script: &str) -> Command {
        let mut command = tool("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn execute_feeds_stdin_and_reads_stdout() {
        let echoed = run(tool("/bin/cat"), b"hello", Duration::from_secs(5)).unwrap();
        assert_eq!(echoed, b"hello");
    }

    #[test]
    fn execute_maps_exit_codes() {
        assert_eq!(
            run(sh("exit 3"), b"", Duration::from_secs(5)),
            Err("sshFailed")
        );
        let denied = sh("echo 'Permission denied (publickey).' >&2; exit 255");
        assert_eq!(run(denied, b"", Duration::from_secs(5)), Err("sshDenied"));
        // A remote program may print anything: only ssh's own failure code is read.
        let spoof = sh("echo 'Permission denied' >&2; exit 1");
        assert_eq!(run(spoof, b"", Duration::from_secs(5)), Err("sshFailed"));
    }

    #[test]
    fn execute_kills_after_the_timeout_and_caps_output() {
        let start = Instant::now();
        assert_eq!(
            run(sh("sleep 30"), b"", Duration::from_millis(200)),
            Err("sshTimeout")
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(
            run(tool("/usr/bin/yes"), b"", Duration::from_secs(10)),
            Err("sshOutputTooLarge")
        );
        let big = vec![0u8; MAX_INPUT + 1];
        assert_eq!(
            run(tool("/bin/cat"), &big, Duration::from_secs(1)),
            Err("sshInputTooLarge")
        );
    }

    #[test]
    fn stderr_is_capped() {
        let done = execute(
            sh("head -c 100000 /dev/zero | tr '\\0' x >&2"),
            b"",
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(done.stderr.len(), STDERR_CAP);
    }

    /// Renders the config, lets the local ssh check it, and prints the settings that apply.
    /// It connects to nothing.
    #[test]
    fn local_ssh_accepts_the_config() {
        if !Path::new(SSH).exists() {
            return;
        }
        let base = scratch("ssh g");
        let remote = base.join("100% remote");
        ensure_private_dir(&remote).unwrap();
        let layout = Layout::for_paths(remote, Some(PathBuf::from("/tmp/lw-test-control")));
        let dropped = write_config(&layout).unwrap();
        eprintln!("dropped by the local ssh: {dropped:?}");
        let destination = Destination::new("user", "example.invalid", Some(22)).unwrap();
        let output = run(
            build(&layout, &destination, &peer(), Mode::PrintConfig(Profile::Device)).unwrap(),
            b"",
            Duration::from_secs(5),
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        for expected in [
            "batchmode yes",
            "stricthostkeychecking true",
            "identitiesonly yes",
            "identityagent none",
            &format!("controlpath /tmp/lw-test-control/{}", "ab".repeat(16)),
            "passwordauthentication no",
            "preferredauthentications publickey",
            "clearallforwardings yes",
            "globalknownhostsfile /dev/null",
            &format!("hostkeyalias lw-{}", "ab".repeat(16)),
        ] {
            assert!(
                text.lines().any(|line| line.starts_with(expected)),
                "missing {expected}\n{text}"
            );
        }
        let known = text
            .lines()
            .find(|line| line.starts_with("userknownhostsfile "))
            .unwrap();
        assert!(
            known.contains("100% remote/known_hosts") && !known.contains("%%"),
            "{known}"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn local_ssh_accepts_the_pairing_config() {
        if !Path::new(SSH).exists() {
            return;
        }
        let base = scratch("ssh pair");
        let remote = base.join("100% remote");
        ensure_private_dir(&remote).unwrap();
        let layout = Layout::for_paths(remote, Some(PathBuf::from("/tmp/lw-test-control")));
        write_pairing_config(&layout).unwrap();
        let destination = Destination::new("user", "example.invalid", Some(22)).unwrap();
        let output = run(
            build(&layout, &destination, &peer(), Mode::PrintConfig(Profile::Pairing)).unwrap(),
            b"",
            Duration::from_secs(5),
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        let identities: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("identityfile "))
            .collect();
        assert_eq!(identities.len(), 1, "{text}");
        // `ssh -G` shows the identity file before it expands `%%`.
        assert!(identities[0].contains("100%% remote/pairing_key"), "{text}");
        for expected in ["identitiesonly yes", "controlmaster false", "stricthostkeychecking true"] {
            assert!(
                text.lines().any(|line| line.starts_with(expected)),
                "missing {expected}\n{text}"
            );
        }
        assert!(!text.lines().any(|line| line.starts_with("controlpath /")), "{text}");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn stop_master_succeeds_without_a_master() {
        if !Path::new(SSH).exists() {
            return;
        }
        let base = scratch("stop");
        let layout = Layout::for_paths(base.clone(), Some(PathBuf::from("/tmp/lw-test-none")));
        write_config(&layout).unwrap();
        let destination = Destination::new("user", "example.invalid", None).unwrap();
        assert_eq!(stop_master(&layout, &destination, &peer()), Ok(()));
        fs::remove_dir_all(base).unwrap();
    }
}
