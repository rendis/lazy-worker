//! What runs when sshd starts `<app> remote --peer <id>` or `<app> remote --pair`.
//!
//! The helper relays bounded lines between its input and the app socket, and nothing else. It
//! starts before any window code, it passes nothing to a shell, and it prints no detail to stderr.
//! The one exception is the word `shell`: while the user at this device has granted a full shell,
//! the helper starts that user's own login shell in the terminal that sshd made, and ends it when
//! the grant ends. It never hands the shell any text from the request.

use super::{
    identity::DeviceId,
    our_uid, peer_uid,
    wire::{error_line, read_line, LineError, MAX_REQUEST_LINE, MAX_RESPONSE_LINE},
};
use std::{
    ffi::OsString,
    fs,
    io::{BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// The bundle identifier in `tauri.conf.json`. A test keeps them equal.
pub const IDENTIFIER: &str = "com.rendis.lazyworker";

pub const EXIT_APP_NOT_RUNNING: i32 = 1;
pub const EXIT_REFUSED: i32 = 2;
pub const EXIT_TOO_LONG: i32 = 3;

/// Where the app socket is, from the home directory. The app config directory is its usual
/// place on macOS, and a path that is too long moves it to `/tmp/lw-<uid>`. See `super::socket_path`.
pub fn socket_path(home: &Path) -> PathBuf {
    super::socket_path(
        &home
            .join("Library/Application Support")
            .join(IDENTIFIER)
            .join("remote"),
        our_uid(),
    )
}

/// Runs the helper when the process was started as `remote ...`. Returns its exit code, or
/// nothing when this is an ordinary start. Call it first in `main`.
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_none_or(|word| word != "remote") {
        return None;
    }
    let output = Output::new(std::io::stdout());
    let command = std::env::var("SSH_ORIGINAL_COMMAND").ok();
    let is_shell = command.as_deref() == Some("shell");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    Some(
        match parse_args(&args[2..]).and_then(|role| {
            check_command(&role, command.as_deref())?;
            Ok(role)
        }) {
            Ok(Role::Peer(peer)) if is_shell => match home {
                Some(home) if home.is_absolute() => shell_session(
                    &socket_path(&home),
                    &peer,
                    // SAFETY: `isatty` only reads the state of descriptor 0.
                    unsafe { libc::isatty(0) } == 1,
                    login_shell(),
                    &output,
                ),
                _ => {
                    output.line(&shell_message("remoteUnavailable"));
                    EXIT_REFUSED
                }
            },
            Ok(role) => match home {
                Some(home) if home.is_absolute() => {
                    session(&socket_path(&home), &role, std::io::stdin(), output)
                }
                _ => {
                    output.line(&error_line("remoteUnavailable"));
                    EXIT_REFUSED
                }
            },
            Err(code) => {
                output.line(&error_line(code));
                EXIT_REFUSED
            }
        },
    )
}

/// Who the helper speaks for: a paired device, or a device that is pairing.
#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    Peer(DeviceId),
    Pair,
}

/// The arguments after `remote`: exactly `--peer <id>` or exactly `--pair`.
pub fn parse_args(args: &[OsString]) -> Result<Role, &'static str> {
    match args {
        [flag, id] if flag == "--peer" => DeviceId::parse(id.to_str().ok_or("argumentsInvalid")?)
            .map(Role::Peer)
            .map_err(|_| "argumentsInvalid"),
        [flag] if flag == "--pair" => Ok(Role::Pair),
        _ => Err("argumentsInvalid"),
    }
}

/// The commands that each role may run. `shell` is for a peer only, and the app decides whether it is granted.
pub fn check_command(role: &Role, original: Option<&str>) -> Result<(), &'static str> {
    match (role, original) {
        (Role::Peer(_), Some("dispatch" | "shell")) | (Role::Pair, Some("pair")) => Ok(()),
        _ => Err("commandNotAllowed"),
    }
}

/// The writer that the two relay threads share, so their lines never mix.
#[derive(Clone)]
pub struct Output(Arc<Mutex<Box<dyn Write + Send>>>);

impl Output {
    pub fn new(writer: impl Write + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Box::new(writer))))
    }

    fn line(&self, line: &str) {
        let mut writer = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let _ = writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.flush());
    }

    fn bytes(&self, line: &[u8]) {
        let mut writer = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let _ = writer
            .write_all(line)
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush());
    }
}

/// Checks that the socket is ours before anything is sent to it: its directory is private and
/// ours, and the socket itself is ours and not a link.
fn check_socket(socket: &Path) -> Result<(), &'static str> {
    let missing = |error: std::io::Error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "appNotRunning"
        } else {
            "remoteUnavailable"
        }
    };
    let directory = socket.parent().ok_or("remoteUnavailable")?;
    let meta = fs::symlink_metadata(directory).map_err(missing)?;
    if !meta.is_dir() || meta.uid() != our_uid() || meta.mode() & 0o077 != 0 {
        return Err("remoteUnavailable");
    }
    let meta = fs::symlink_metadata(socket).map_err(missing)?;
    if !meta.file_type().is_socket() || meta.uid() != our_uid() {
        return Err("remoteUnavailable");
    }
    Ok(())
}

/// The first line for the app: who is on the other end.
fn hello(role: &Role) -> serde_json::Value {
    match role {
        Role::Peer(peer) => serde_json::json!({ "hello": { "peer": peer.as_str() } }),
        Role::Pair => serde_json::json!({ "hello": { "pair": true } }),
    }
}

/// The first line of a shell session. The app answers `ok` only while the shell is granted to this peer.
fn shell_hello(peer: &DeviceId) -> serde_json::Value {
    serde_json::json!({ "hello": { "peer": peer.as_str(), "shell": true } })
}

/// Opens the app socket after the checks that it is ours. The code and the exit code on failure.
fn connect(socket: &Path) -> Result<UnixStream, (&'static str, i32)> {
    check_socket(socket).map_err(|code| (code, EXIT_APP_NOT_RUNNING))?;
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(("appNotRunning", EXIT_APP_NOT_RUNNING))
        }
        Err(_) => return Err(("remoteUnavailable", EXIT_APP_NOT_RUNNING)),
    };
    if peer_uid(&stream) != Some(our_uid()) {
        return Err(("remoteUnavailable", EXIT_APP_NOT_RUNNING));
    }
    Ok(stream)
}

/// Relays between `input` and the app socket until one side ends. Returns the exit code.
pub fn session(
    socket: &Path,
    role: &Role,
    input: impl Read + Send + 'static,
    output: Output,
) -> i32 {
    let fail = |code: &str, exit: i32| {
        output.line(&error_line(code));
        exit
    };
    let stream = match connect(socket) {
        Ok(stream) => stream,
        Err((code, exit)) => return fail(code, exit),
    };
    let (Ok(mut writer), Ok(reader)) = (stream.try_clone(), stream.try_clone()) else {
        return fail("remoteUnavailable", EXIT_APP_NOT_RUNNING);
    };
    let hello = format!("{}\n", hello(role));
    if writer.write_all(hello.as_bytes()).is_err() {
        return fail("remoteUnavailable", EXIT_APP_NOT_RUNNING);
    }

    // The input side may block on a terminal that never ends, so it is left behind when the app ends the session.
    let too_long = Arc::new(Mutex::new(false));
    {
        let (output, too_long) = (output.clone(), too_long.clone());
        thread::spawn(move || {
            let mut input = BufReader::new(input);
            loop {
                match read_line(&mut input, MAX_REQUEST_LINE) {
                    Ok(Some(mut line)) => {
                        line.push(b'\n');
                        if writer.write_all(&line).is_err() {
                            break;
                        }
                    }
                    Err(LineError::TooLong) => {
                        *too_long.lock().unwrap_or_else(|error| error.into_inner()) = true;
                        output.line(&error_line("lineTooLong"));
                        let _ = writer.shutdown(std::net::Shutdown::Both);
                        return;
                    }
                    Ok(None) | Err(LineError::Io) => break,
                }
            }
            // The end of the input ends the session. The app answers what is pending, then closes.
            let _ = writer.shutdown(std::net::Shutdown::Write);
        });
    }

    let mut responses = BufReader::new(reader);
    loop {
        match read_line(&mut responses, MAX_RESPONSE_LINE) {
            Ok(Some(line)) => output.bytes(&line),
            Ok(None) | Err(LineError::Io) => break,
            Err(LineError::TooLong) => {
                output.line(&error_line("responseTooLong"));
                return EXIT_TOO_LONG;
            }
        }
    }
    if *too_long.lock().unwrap_or_else(|error| error.into_inner()) {
        EXIT_TOO_LONG
    } else {
        0
    }
}

/// How long the shell group has to end after the first signal, before it is killed.
const KILL_AFTER: Duration = Duration::from_secs(3);
/// How long the app has to answer the shell hello.
const SHELL_ANSWER: Duration = Duration::from_secs(10);

/// What the terminal shows when a shell cannot start. Plain words, for the person at the terminal.
fn shell_message(code: &str) -> String {
    let text = match code {
        "notPermitted" | "notPaired" => "Full shell is off on this device.",
        "appNotRunning" => "Lazy Worker is not running on this device.",
        "noTerminal" => "A shell needs a terminal.",
        "shellUnavailable" => "This device has no login shell for this user.",
        _ => "The shell could not start.",
    };
    format!("{text}\r\n")
}

/// The login shell of this user from the user database. It must be an absolute path to a file.
fn login_shell() -> Option<PathBuf> {
    // SAFETY: the entry and the buffer outlive the call, and the length is the buffer's own.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 8192];
    // SAFETY: as above. `geteuid` takes no arguments.
    let status = unsafe {
        libc::getpwuid_r(libc::geteuid(), &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found)
    };
    if status != 0 || found.is_null() || entry.pw_shell.is_null() {
        return None;
    }
    // SAFETY: a found entry has a shell text that ends with a zero inside the buffer.
    let text = unsafe { std::ffi::CStr::from_ptr(entry.pw_shell) }.to_str().ok()?;
    let path = PathBuf::from(text);
    (path.is_absolute() && path.is_file()).then_some(path)
}

/// The user's login shell, interactive, in its own process group, which becomes the foreground
/// group of the terminal. Nothing from the request reaches it.
fn shell_command(shell: &Path) -> Command {
    let mut command = Command::new(shell);
    command
        .args(["-l", "-i"])
        .env_remove("SSH_ORIGINAL_COMMAND")
        .process_group(0);
    // SAFETY: the closure calls only `signal`, `tcsetpgrp` and `getpid`, which are safe after fork.
    unsafe {
        command.pre_exec(|| {
            // The group is new and in the background of the terminal, so the shell would stop at its first read.
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(0, libc::getpid());
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            Ok(())
        });
    }
    command
}

/// True while any process is in the group.
fn group_alive(group: i32) -> bool {
    // SAFETY: signal 0 only checks that the group exists.
    unsafe { libc::kill(-group, 0) == 0 }
}

/// Ends every process of the shell's group: hang up, then kill what is left after `kill_after`.
/// The child is reaped. Returns its exit code.
fn end_group(child: &mut Child, kill_after: Duration) -> i32 {
    let Ok(group) = i32::try_from(child.id()) else { return 1 };
    // SAFETY: `killpg` takes a group id and a signal number.
    unsafe { libc::killpg(group, libc::SIGHUP) };
    let deadline = Instant::now() + kill_after;
    let mut code = None;
    loop {
        if code.is_none() {
            if let Ok(Some(status)) = child.try_wait() {
                code = Some(exit_code(status));
            }
        }
        if code.is_some() && !group_alive(group) {
            break;
        }
        if Instant::now() >= deadline {
            // SAFETY: as above.
            unsafe { libc::killpg(group, libc::SIGKILL) };
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    code.unwrap_or_else(|| child.wait().map_or(1, exit_code))
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.code().or_else(|| status.signal().map(|signal| 128 + signal)).unwrap_or(1)
}

/// Waits for the shell to exit or for `closed` to fire or hang up, and then ends the whole group.
/// Returns the exit code of the shell.
fn supervise(child: &mut Child, closed: &mpsc::Receiver<()>, kill_after: Duration) -> i32 {
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => {}
        }
        match closed.recv_timeout(Duration::from_millis(50)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    end_group(child, kill_after)
}

/// Runs one shell session: asks the app, starts the login shell, and ends it with the grant.
/// `tty` says that descriptor 0 is a terminal. A shell never starts without one.
fn shell_session(
    socket: &Path,
    peer: &DeviceId,
    tty: bool,
    shell: Option<PathBuf>,
    output: &Output,
) -> i32 {
    shell_session_with(socket, peer, tty, shell, output, KILL_AFTER, shell_command)
}

pub(crate) fn shell_session_with(
    socket: &Path,
    peer: &DeviceId,
    tty: bool,
    shell: Option<PathBuf>,
    output: &Output,
    kill_after: Duration,
    build: impl FnOnce(&Path) -> Command,
) -> i32 {
    let fail = |code: &str| {
        output.line(&shell_message(code));
        EXIT_REFUSED
    };
    if !tty {
        return fail("noTerminal");
    }
    let stream = match connect(socket) {
        Ok(stream) => stream,
        Err((code, _)) => return fail(code),
    };
    let (Ok(mut writer), Ok(reader)) = (stream.try_clone(), stream.try_clone()) else {
        return fail("remoteUnavailable");
    };
    if writer.write_all(format!("{}\n", shell_hello(peer)).as_bytes()).is_err() {
        return fail("remoteUnavailable");
    }
    // The app answers at once, and only with `ok` while the grant lasts.
    let _ = reader.set_read_timeout(Some(SHELL_ANSWER));
    let mut answers = BufReader::new(reader);
    let answer = match read_line(&mut answers, MAX_RESPONSE_LINE) {
        Ok(Some(line)) => serde_json::from_slice::<serde_json::Value>(&line).ok(),
        _ => None,
    };
    let Some(answer) = answer else { return fail("remoteUnavailable") };
    if answer.get("ok").is_none() {
        let code = answer.get("error").and_then(|code| code.as_str()).unwrap_or("remoteUnavailable");
        return fail(code);
    }
    let Some(shell) = shell else { return fail("shellUnavailable") };
    let _ = answers.get_ref().set_read_timeout(None);
    let mut child = match build(&shell).spawn() {
        Ok(child) => child,
        Err(_) => return fail("shellUnavailable"),
    };
    // Anything the app sends after `ok` is ignored. The end of the connection ends the session.
    let (ended, closed) = mpsc::channel();
    thread::spawn(move || {
        let mut sink = [0u8; 256];
        while matches!(answers.read(&mut sink), Ok(count) if count > 0) {}
        let _ = ended.send(());
    });
    let code = supervise(&mut child, &closed, kill_after);
    // The app records the end when this connection closes.
    let _ = writer.shutdown(std::net::Shutdown::Both);
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_identifier_matches_the_bundle_configuration() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], IDENTIFIER);
    }

    #[test]
    fn the_socket_lives_in_the_app_config_directory() {
        assert_eq!(
            socket_path(Path::new("/Users/me")),
            Path::new(
                "/Users/me/Library/Application Support/com.rendis.lazyworker/remote/app.sock"
            )
        );
    }

    #[test]
    fn only_peer_with_a_valid_id_or_pair_alone_is_accepted() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_args(&args(&["--peer", id])).unwrap(),
            Role::Peer(DeviceId::parse(id).unwrap())
        );
        assert_eq!(parse_args(&args(&["--pair"])).unwrap(), Role::Pair);
        for bad in [
            args(&["--pair", id]),
            args(&["--pair", "--peer", id]),
            args(&["--peer", id, "--pair"]),
            args(&["--pair", "--pair"]),
            args(&["--Pair"]),
            args(&[]),
            args(&["--peer"]),
            args(&["--peer", "short"]),
            args(&["--peer", "0123456789ABCDEF0123456789ABCDEF"]),
            args(&["--peer", id, "extra"]),
            args(&["--shell", id]),
            args(&[id]),
        ] {
            assert_eq!(parse_args(&bad).err(), Some("argumentsInvalid"), "{bad:?}");
        }
    }

    #[test]
    fn a_peer_runs_only_dispatch_and_a_pairing_runs_only_pair() {
        let peer = Role::Peer(DeviceId::parse("0123456789abcdef0123456789abcdef").unwrap());
        assert_eq!(check_command(&peer, Some("dispatch")), Ok(()));
        assert_eq!(check_command(&peer, Some("shell")), Ok(()));
        assert_eq!(check_command(&Role::Pair, Some("pair")), Ok(()));
        // The pairing key never gets a shell, whatever the grants of the user are.
        assert_eq!(check_command(&Role::Pair, Some("shell")), Err("commandNotAllowed"));
        // Each role refuses the other's word.
        assert_eq!(check_command(&peer, Some("pair")), Err("commandNotAllowed"));
        assert_eq!(
            check_command(&Role::Pair, Some("dispatch")),
            Err("commandNotAllowed")
        );
        for role in [&peer, &Role::Pair] {
            for other in [
                None,
                Some(""),
                Some("shell "),
                Some("Shell"),
                Some("shell; id"),
                Some("shell\n"),
                Some("dispatch "),
                Some("pair "),
                Some("dispatch; id"),
                Some("pair; id"),
                Some("Dispatch"),
                Some("Pair"),
                Some("bash"),
            ] {
                assert_eq!(
                    check_command(role, other),
                    Err("commandNotAllowed"),
                    "{role:?} {other:?}"
                );
            }
        }
    }

    #[test]
    fn the_hello_names_the_peer_or_the_pairing() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            hello(&Role::Peer(DeviceId::parse(id).unwrap())).to_string(),
            format!("{{\"hello\":{{\"peer\":\"{id}\"}}}}")
        );
        assert_eq!(
            hello(&Role::Pair).to_string(),
            "{\"hello\":{\"pair\":true}}"
        );
    }

    #[test]
    fn without_the_app_the_helper_answers_app_not_running() {
        let dir = super::super::testing::scratch("helper-none");
        let sink = crate::remote::testing::Sink::default();
        let peer = Role::Peer(DeviceId::parse("0123456789abcdef0123456789abcdef").unwrap());
        let code = session(
            &dir.join("remote/app.sock"),
            &peer,
            std::io::empty(),
            Output::new(sink.clone()),
        );
        assert_eq!(code, EXIT_APP_NOT_RUNNING);
        assert_eq!(sink.text(), "{\"error\":\"appNotRunning\"}\n");
        fs::remove_dir_all(dir).unwrap();
    }

    use std::{io::BufRead, process::Stdio};

    /// A shell stand-in in its own group, with no terminal. It prints the id of a background `sleep`.
    fn group(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .process_group(0);
        command
    }

    fn first_line(child: &mut Child) -> i32 {
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        line.trim().parse().unwrap()
    }

    fn gone(pid: i32) -> bool {
        for _ in 0..100 {
            // SAFETY: signal 0 only checks that the process exists.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn closing_the_session_hangs_up_the_whole_process_group() {
        let mut child = group("sleep 60 & echo $!; wait").spawn().unwrap();
        let sleeper = first_line(&mut child);
        let (end, closed) = mpsc::channel();
        end.send(()).unwrap();
        supervise(&mut child, &closed, Duration::from_secs(3));
        assert!(gone(sleeper), "a process of the group is still running");
    }

    #[test]
    fn a_group_that_ignores_the_hangup_is_killed_after_the_wait() {
        let mut child = group("trap '' HUP; sleep 60 & echo $!; wait").spawn().unwrap();
        let sleeper = first_line(&mut child);
        let (end, closed) = mpsc::channel();
        // A connection that drops ends the session as well as a message does.
        drop(end);
        let started = Instant::now();
        supervise(&mut child, &closed, Duration::from_millis(400));
        assert!(started.elapsed() >= Duration::from_millis(400), "killed before the wait ended");
        assert!(gone(sleeper));
    }

    #[test]
    fn a_shell_that_exits_takes_what_it_left_behind() {
        let mut child = group("sleep 60 & echo $!").spawn().unwrap();
        let sleeper = first_line(&mut child);
        let (_end, closed) = mpsc::channel();
        assert_eq!(supervise(&mut child, &closed, Duration::from_secs(3)), 0);
        assert!(gone(sleeper));
        let mut failing = group("exit 7").spawn().unwrap();
        let (_end, closed) = mpsc::channel();
        assert_eq!(supervise(&mut failing, &closed, Duration::from_secs(3)), 7);
    }

    #[test]
    fn a_shell_needs_a_terminal_and_says_so_without_asking_the_app() {
        let sink = crate::remote::testing::Sink::default();
        let peer = DeviceId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let code = shell_session_with(
            Path::new("/nonexistent/app.sock"),
            &peer,
            false,
            Some(PathBuf::from("/bin/sh")),
            &Output::new(sink.clone()),
            KILL_AFTER,
            |_| panic!("no shell may start"),
        );
        assert_eq!(code, EXIT_REFUSED);
        assert_eq!(sink.text(), "A shell needs a terminal.\r\n");
    }

    #[test]
    fn the_login_shell_is_an_absolute_file_and_the_command_carries_no_request_text() {
        if let Some(path) = login_shell() {
            assert!(path.is_absolute() && path.is_file());
        }
        let command = shell_command(Path::new("/bin/zsh"));
        assert_eq!(command.get_program(), "/bin/zsh");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["-l", "-i"]);
        let removed = command
            .get_envs()
            .any(|(name, value)| name == "SSH_ORIGINAL_COMMAND" && value.is_none());
        assert!(removed);
        assert_eq!(shell_hello(&DeviceId::parse("0123456789abcdef0123456789abcdef").unwrap()).to_string(),
            "{\"hello\":{\"peer\":\"0123456789abcdef0123456789abcdef\",\"shell\":true}}");
    }
}
