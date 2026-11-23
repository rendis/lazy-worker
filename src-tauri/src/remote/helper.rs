//! What runs when sshd starts `<app> remote --peer <id>` or `<app> remote --pair`.
//!
//! The helper relays bounded lines between its input and the app socket, and nothing else. It
//! starts before any window code, it passes nothing to a shell, and it prints no detail to stderr.

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
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

/// The bundle identifier in `tauri.conf.json`. A test keeps them equal.
pub const IDENTIFIER: &str = "com.rendis.lazyworker";

pub const EXIT_APP_NOT_RUNNING: i32 = 1;
pub const EXIT_REFUSED: i32 = 2;
pub const EXIT_TOO_LONG: i32 = 3;

/// Where the app socket is, from the home directory. It is the app config directory on macOS.
pub fn socket_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support")
        .join(IDENTIFIER)
        .join("remote/app.sock")
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
    let home = std::env::var_os("HOME").map(PathBuf::from);
    Some(
        match parse_args(&args[2..]).and_then(|role| {
            check_command(&role, command.as_deref())?;
            Ok(role)
        }) {
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

/// The one command that each role may run. The shell comes in a later step.
pub fn check_command(role: &Role, original: Option<&str>) -> Result<(), &'static str> {
    match (role, original) {
        (Role::Peer(_), Some("dispatch")) | (Role::Pair, Some("pair")) => Ok(()),
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
    if let Err(code) = check_socket(socket) {
        return fail(code, EXIT_APP_NOT_RUNNING);
    }
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return fail("appNotRunning", EXIT_APP_NOT_RUNNING)
        }
        Err(_) => return fail("remoteUnavailable", EXIT_APP_NOT_RUNNING),
    };
    if peer_uid(&stream) != Some(our_uid()) {
        return fail("remoteUnavailable", EXIT_APP_NOT_RUNNING);
    }
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
        assert_eq!(check_command(&Role::Pair, Some("pair")), Ok(()));
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
                Some("shell"),
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
}
