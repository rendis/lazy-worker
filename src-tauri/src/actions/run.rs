//! Runs one shell command in its own process group and keeps what it prints.
//!
//! The command runs as `$SHELL -l -c <command>` with stdin closed and the home folder as its working
//! folder. Output and errors share one pipe, so they stay in the order they were written. Stop and the
//! time limit send SIGTERM to the whole group, and SIGKILL after a short wait.

use serde::Serialize;
use std::{
    io::Read,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

/// The most output that a run keeps. The tail stays, and the run says that it was cut.
pub const MAX_OUTPUT: usize = 256 * 1024;
const FALLBACK_SHELL: &str = "/bin/zsh";
/// How long a command that is told to end has before it is killed.
const KILL_AFTER: Duration = Duration::from_secs(3);
/// How long the end of the output is awaited after the command ended, when a process that it started still holds the pipe.
/// The longest wait for the last output after the command ended. A normal run is done long before.
const DRAIN: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(20);

/// How a command is started. Tests replace it so they need no login shell.
#[derive(Clone, Debug)]
pub struct Runner {
    pub shell: PathBuf,
    pub home: PathBuf,
    /// Passes `-l`, so the command sees the PATH that the user's terminal has.
    pub login: bool,
    pub kill_after: Duration,
}

extern "C" {
    fn geteuid() -> u32;
}

impl Runner {
    /// The shell and home of the user that runs this app, from the user database.
    pub fn system() -> Self {
        let (shell, home) = passwd();
        let shell = shell
            .map(PathBuf::from)
            .filter(|shell| shell.is_absolute() && shell.is_file())
            .unwrap_or_else(|| PathBuf::from(FALLBACK_SHELL));
        let home = home
            .map(PathBuf::from)
            .filter(|home| home.is_absolute() && home.is_dir())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute()))
            .unwrap_or_else(|| PathBuf::from("/"));
        Self {
            shell,
            home,
            login: true,
            kill_after: KILL_AFTER,
        }
    }
}

/// The login shell and the home folder in the user database.
fn passwd() -> (Option<String>, Option<String>) {
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 8192];
    // SAFETY: the entry and the buffer outlive the call, and the length is the buffer's own.
    let status = unsafe { libc::getpwuid_r(geteuid(), &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found) };
    if status != 0 || found.is_null() {
        return (None, None);
    }
    let text = |pointer: *const libc::c_char| {
        // SAFETY: a non-null field of a found entry points to a text that ends with a zero inside the buffer.
        (!pointer.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(pointer) }.to_string_lossy().into_owned())
    };
    (text(entry.pw_shell), text(entry.pw_dir))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Running,
    Finished,
    Stopped,
    TimedOut,
    Failed,
}

struct Inner {
    state: State,
    exit_code: Option<i32>,
    output: Vec<u8>,
    truncated: bool,
    started: Instant,
    ended: Option<Instant>,
}

struct Shared {
    inner: Mutex<Inner>,
    stop: std::sync::atomic::AtomicBool,
}

/// What a run looks like at one moment.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub state: State,
    pub exit_code: Option<i32>,
    pub seconds: u64,
    pub output: String,
    pub truncated: bool,
}

/// What is left when a run ends, for the record.
#[derive(Clone, Copy, Debug)]
pub struct Done {
    pub state: State,
    pub exit_code: Option<i32>,
}

impl Done {
    pub fn timed_out(self) -> bool {
        self.state == State::TimedOut
    }

    pub fn ok(self) -> bool {
        self.state == State::Finished && self.exit_code == Some(0)
    }
}

#[derive(Clone)]
pub struct Handle(Arc<Shared>);

impl Handle {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn state(&self) -> State {
        self.lock().state
    }

    pub fn view(&self) -> View {
        let inner = self.lock();
        let end = inner.ended.unwrap_or_else(Instant::now);
        View {
            state: inner.state,
            exit_code: inner.exit_code,
            seconds: end.duration_since(inner.started).as_secs(),
            output: String::from_utf8_lossy(&inner.output).into_owned(),
            truncated: inner.truncated,
        }
    }

    /// Asks a running command to end. It does nothing once the command ended.
    pub fn stop(&self) {
        self.0.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn append(inner: &mut Inner, chunk: &[u8]) {
    inner.output.extend_from_slice(chunk);
    if inner.output.len() > MAX_OUTPUT {
        let extra = inner.output.len() - MAX_OUTPUT;
        inner.output.drain(..extra);
        inner.truncated = true;
    }
}

fn signal_group(group: i32, signal: i32) {
    // SAFETY: `kill` takes numbers only. A negative number names the process group, which this run made.
    unsafe {
        libc::kill(-group, signal);
    }
}

/// Starts the command and returns at once. `done` is called from the thread that watches it, after the state is final.
pub fn spawn(
    runner: &Runner,
    command: &str,
    limit: Duration,
    done: impl FnOnce(Done) + Send + 'static,
) -> Handle {
    let shared = Arc::new(Shared {
        inner: Mutex::new(Inner {
            state: State::Running,
            exit_code: None,
            output: Vec::new(),
            truncated: false,
            started: Instant::now(),
            ended: None,
        }),
        stop: std::sync::atomic::AtomicBool::new(false),
    });
    let handle = Handle(shared);
    let (runner, command, watcher) = (runner.clone(), command.to_owned(), handle.clone());
    thread::spawn(move || {
        let result = watch(&runner, &command, limit, &watcher);
        let mut inner = watcher.lock();
        inner.state = result.state;
        inner.exit_code = result.exit_code;
        inner.ended = Some(Instant::now());
        drop(inner);
        done(result);
    });
    handle
}

fn failed() -> Done {
    Done {
        state: State::Failed,
        exit_code: None,
    }
}

fn watch(runner: &Runner, command: &str, limit: Duration, handle: &Handle) -> Done {
    let Ok((mut reader, writer)) = std::io::pipe() else {
        return failed();
    };
    let Ok(second) = writer.try_clone() else {
        return failed();
    };
    let mut process = Command::new(&runner.shell);
    if runner.login {
        process.arg("-l");
    }
    process
        .arg("-c")
        .arg(command)
        .env("HOME", &runner.home)
        .current_dir(&runner.home)
        .stdin(Stdio::null())
        .stdout(second)
        .stderr(writer)
        .process_group(0);
    let spawned = process.spawn();
    // The command holds the only write ends now. Without this, the reader would never see the end.
    drop(process);
    let Ok(mut child) = spawned else {
        return failed();
    };
    let group = i32::try_from(child.id()).unwrap_or(0);

    let sink = handle.clone();
    let output = thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(read) = reader.read(&mut chunk) {
            if read == 0 {
                break;
            }
            append(&mut sink.lock(), &chunk[..read]);
        }
    });

    let started = Instant::now();
    let mut asked: Option<Instant> = None;
    let mut by_stop = false;
    let mut timed_out = false;
    let mut killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        let now = Instant::now();
        if asked.is_none() {
            let stopping = handle.0.stop.load(std::sync::atomic::Ordering::SeqCst);
            if stopping || now.duration_since(started) >= limit {
                by_stop = stopping;
                timed_out = !stopping;
                asked = Some(now);
                signal_group(group, libc::SIGTERM);
            }
        } else if !killed && asked.is_some_and(|at| now.duration_since(at) >= runner.kill_after) {
            killed = true;
            signal_group(group, libc::SIGKILL);
        }
        thread::sleep(POLL);
    };

    // A process that the command started can hold the pipe after the command itself ended.
    // After a stop or a time limit it gets the same short wait, and then it is killed with the rest.
    let drain_until = match asked {
        Some(at) => at + runner.kill_after,
        None => Instant::now() + DRAIN,
    };
    while !output.is_finished() && Instant::now() < drain_until {
        thread::sleep(POLL);
    }
    if asked.is_some() && !output.is_finished() {
        signal_group(group, libc::SIGKILL);
        let wait = Instant::now() + DRAIN;
        while !output.is_finished() && Instant::now() < wait {
            thread::sleep(POLL);
        }
    }
    let exit_code = status.and_then(|status| status.code());
    let state = if status.is_none() {
        State::Failed
    } else if by_stop {
        State::Stopped
    } else if timed_out {
        State::TimedOut
    } else {
        State::Finished
    };
    Done { state, exit_code }
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// `/bin/sh` without a login shell, in a folder that the test owns.
    pub fn runner(home: PathBuf) -> Runner {
        Runner {
            shell: PathBuf::from("/bin/sh"),
            home,
            login: false,
            kill_after: Duration::from_millis(300),
        }
    }

    pub fn wait_done(handle: &Handle) -> View {
        let until = Instant::now() + Duration::from_secs(20);
        while handle.state() == State::Running {
            assert!(Instant::now() < until, "the run did not end");
            thread::sleep(Duration::from_millis(10));
        }
        handle.view()
    }

    pub fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};
    use crate::remote::testing::scratch;
    use std::{fs, sync::mpsc};

    fn run(runner: &Runner, command: &str, seconds: u64) -> View {
        let handle = spawn(runner, command, Duration::from_secs(seconds), |_| {});
        wait_done(&handle)
    }

    #[test]
    fn output_and_errors_keep_their_order_and_the_exit_code_is_kept() {
        let home = scratch("run-order");
        let view = run(&runner(home.clone()), "echo one; echo two 1>&2; echo three; exit 3", 10);
        assert_eq!(view.state, State::Finished);
        assert_eq!(view.exit_code, Some(3));
        assert_eq!(view.output, "one\ntwo\nthree\n");
        assert!(!view.truncated);
        let ok = run(&runner(home.clone()), "true", 10);
        assert_eq!((ok.state, ok.exit_code), (State::Finished, Some(0)));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn the_command_runs_in_the_home_folder_with_no_input() {
        let home = scratch("run-home");
        let real = fs::canonicalize(&home).unwrap();
        let view = run(&runner(home.clone()), "pwd; echo \"$HOME\"; cat; echo end", 10);
        assert_eq!(view.output, format!("{0}\n{1}\nend\n", real.display(), home.display()));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_command_that_cannot_start_fails() {
        let home = scratch("run-fail");
        let mut bad = runner(home.clone());
        bad.shell = PathBuf::from("/nonexistent/shell");
        let view = run(&bad, "echo hi", 10);
        assert_eq!((view.state, view.exit_code), (State::Failed, None));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn output_past_the_cap_keeps_the_tail_and_says_it_was_cut() {
        let home = scratch("run-cap");
        let view = run(&runner(home.clone()), "seq 1 200000", 20);
        assert_eq!(view.state, State::Finished);
        assert!(view.truncated);
        assert!(view.output.len() <= MAX_OUTPUT);
        assert!(view.output.ends_with("199999\n200000\n"));
        let short = run(&runner(home.clone()), "seq 1 10", 10);
        assert!(!short.truncated);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn the_time_limit_ends_the_whole_group() {
        let home = scratch("run-timeout");
        let started = Instant::now();
        // The child process is in the group of the shell. Its id is printed so the test can look for it.
        let view = run(&runner(home.clone()), "sleep 30 & echo $!; wait", 1);
        assert_eq!(view.state, State::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
        let child: i32 = view.output.trim().parse().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while alive(child) && Instant::now() < until {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(child), "the child of the shell is still running");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_command_that_ignores_the_request_to_end_is_killed_after_a_wait() {
        let home = scratch("run-kill");
        let started = Instant::now();
        let view = run(&runner(home.clone()), "trap '' TERM; sleep 30 & echo $!; wait; echo never", 1);
        assert_eq!(view.state, State::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!view.output.contains("never"));
        let child: i32 = view.output.lines().next().unwrap().parse().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while alive(child) && Instant::now() < until {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(child));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn stop_ends_a_running_command_and_does_nothing_after() {
        let home = scratch("run-stop");
        let (sender, receiver) = mpsc::channel();
        let handle = spawn(&runner(home.clone()), "echo started; sleep 30", Duration::from_secs(60), move |done| {
            sender.send(done).unwrap();
        });
        let until = Instant::now() + Duration::from_secs(5);
        while !handle.view().output.contains("started") {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(handle.state(), State::Running, "output shows before the end");
        handle.stop();
        let view = wait_done(&handle);
        assert_eq!(view.state, State::Stopped);
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap().state, State::Stopped);
        handle.stop();
        assert_eq!(handle.state(), State::Stopped);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_background_process_that_holds_the_pipe_does_not_hold_the_run() {
        let home = scratch("run-background");
        let started = Instant::now();
        let view = run(&runner(home.clone()), "sleep 5 & echo done", 20);
        assert_eq!((view.state, view.exit_code), (State::Finished, Some(0)));
        assert!(started.elapsed() < Duration::from_secs(3));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn the_system_runner_has_an_absolute_shell_and_home() {
        let runner = Runner::system();
        assert!(runner.shell.is_absolute() && runner.home.is_absolute());
        assert!(runner.login);
    }
}
