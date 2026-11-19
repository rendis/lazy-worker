use super::{CaffeinateMode, CaffeinateSettings, RunningProcess};
use std::{
    ffi::{c_int, c_uint, c_void},
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
    process::{self, Child, Command, Stdio},
    ptr,
};

pub const SUPPORTED: bool = true;

pub fn list_processes() -> Result<Vec<RunningProcess>, &'static str> {
    // Darwin ps defaults to the current user. comm contains the executable name without arguments.
    let child = Command::new("/bin/ps")
        .args(["-x", "-ww", "-o", "pid=,comm="])
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "caffeinateProcessListFailed")?;
    let transient_pid = child.id();
    let output = child
        .wait_with_output()
        .map_err(|_| "caffeinateProcessListFailed")?;
    if !output.status.success() {
        return Err("caffeinateProcessListFailed");
    }
    Ok(parse_processes(
        &String::from_utf8_lossy(&output.stdout),
        transient_pid,
    ))
}

fn parse_processes(output: &str, transient_pid: u32) -> Vec<RunningProcess> {
    let mut processes: Vec<_> = output
        .lines()
        .filter_map(|line| {
            let (pid, executable) = line.trim_start().split_once(char::is_whitespace)?;
            let pid = pid.parse().ok()?;
            if pid == 0 || pid == transient_pid {
                return None;
            }
            let name = Path::new(executable.trim())
                .file_name()?
                .to_str()?
                .to_owned();
            Some(RunningProcess { pid, name })
        })
        .collect();
    processes.sort_unstable_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then(a.pid.cmp(&b.pid))
    });
    processes
}

pub struct Worker {
    assertion: Assertion,
    target: Target,
}

enum Target {
    None,
    Process(ProcessExit),
}

impl Worker {
    pub fn start(settings: &CaffeinateSettings) -> Result<Self, &'static str> {
        let target = Target::watch_process(settings)?;
        let assertion = Assertion::start(settings)?;
        Ok(Self { assertion, target })
    }

    pub fn reconfigure(
        &mut self,
        settings: &CaffeinateSettings,
        target_changed: bool,
    ) -> Result<(), &'static str> {
        let target = target_changed
            .then(|| Target::watch_process(settings))
            .transpose()?;
        // Prepare the new assertion before replacing the one that protects the current target.
        let mut assertion = Assertion::start(settings)?;
        if !assertion.is_running()? {
            return Err("caffeinateExited");
        }
        self.assertion = assertion;
        if let Some(target) = target {
            self.target = target;
        }
        Ok(())
    }

    pub fn is_running(&mut self) -> Result<bool, &'static str> {
        if !self.assertion.is_running()? {
            return Ok(false);
        }

        match &self.target {
            Target::None => Ok(true),
            Target::Process(process) => process.is_running(),
        }
    }
}

impl Target {
    fn watch_process(settings: &CaffeinateSettings) -> Result<Self, &'static str> {
        if settings.mode == CaffeinateMode::Process {
            let pid = settings.pid.ok_or("caffeinateInvalidPid")?;
            return Ok(Self::Process(ProcessExit::watch(pid)?));
        }
        Ok(Self::None)
    }
}

struct Assertion {
    child: Child,
    timed: bool,
}

impl Assertion {
    fn start(settings: &CaffeinateSettings) -> Result<Self, &'static str> {
        let timed = settings.mode == CaffeinateMode::Timer;
        let mut command = Command::new("/usr/bin/caffeinate");
        // Watch the app itself so a forced exit also releases the assertion.
        command
            .args(settings.flags.enabled())
            .arg("-w")
            .arg(process::id().to_string());
        if timed {
            command.arg("-t").arg(settings.seconds.to_string());
        }
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "caffeinateStartFailed")?;
        Ok(Self { child, timed })
    }

    fn is_running(&mut self) -> Result<bool, &'static str> {
        match self.child.try_wait() {
            Ok(None) => Ok(true),
            Ok(Some(status)) if self.timed && status.success() => Ok(false),
            _ => Err("caffeinateExited"),
        }
    }
}

impl Drop for Assertion {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

// Layout and values from Darwin's sys/event.h.
#[repr(C)]
struct Kevent {
    ident: u64,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: i64,
    udata: u64,
    ext: [u64; 2],
}

const EVFILT_PROC: i16 = -5;
const EV_ADD: u16 = 0x0001;
const NOTE_EXIT: u32 = 0x8000_0000;
const KEVENT_FLAG_IMMEDIATE: c_uint = 0x000001;

extern "C" {
    fn kqueue() -> c_int;
    fn kevent64(
        queue: c_int,
        changes: *const Kevent,
        change_count: c_int,
        events: *mut Kevent,
        event_count: c_int,
        flags: c_uint,
        timeout: *const c_void,
    ) -> c_int;
}

struct ProcessExit(OwnedFd);

impl ProcessExit {
    fn watch(pid: u32) -> Result<Self, &'static str> {
        // SAFETY: kqueue has no arguments and returns a new owned descriptor on success.
        let queue = unsafe { kqueue() };
        if queue < 0 {
            return Err("caffeinateStartFailed");
        }
        // SAFETY: Ownership of the new descriptor passes to OwnedFd.
        let queue = unsafe { OwnedFd::from_raw_fd(queue) };
        let change = Kevent {
            ident: pid.into(),
            filter: EVFILT_PROC,
            flags: EV_ADD,
            fflags: NOTE_EXIT,
            data: 0,
            udata: 0,
            ext: [0; 2],
        };
        // SAFETY: The change list has one valid entry. There is no output list or timeout.
        let registered = unsafe {
            kevent64(
                queue.as_raw_fd(),
                &change,
                1,
                ptr::null_mut(),
                0,
                KEVENT_FLAG_IMMEDIATE,
                ptr::null(),
            )
        };
        if registered < 0 {
            return Err("caffeinateProcessUnavailable");
        }
        Ok(Self(queue))
    }

    fn is_running(&self) -> Result<bool, &'static str> {
        // SAFETY: The output list has space for one event. There is no change list or timeout.
        let ready = unsafe {
            kevent64(
                self.0.as_raw_fd(),
                ptr::null(),
                0,
                std::mem::MaybeUninit::<Kevent>::uninit().as_mut_ptr(),
                1,
                KEVENT_FLAG_IMMEDIATE,
                ptr::null(),
            )
        };
        match ready {
            0 => Ok(true),
            1 => Ok(false),
            _ if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => Ok(true),
            _ => Err("caffeinateExited"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_watcher_reports_exit_without_signaling_the_process() {
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let watcher = ProcessExit::watch(child.id()).unwrap();
        assert_eq!(watcher.is_running(), Ok(true));
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        assert_eq!(watcher.is_running(), Ok(false));
    }

    #[test]
    fn reconfiguring_a_process_keeps_the_target_and_handles_failed_replacement() {
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let settings = CaffeinateSettings {
            mode: CaffeinateMode::Process,
            pid: Some(child.id()),
            ..CaffeinateSettings::default()
        };
        let mut worker = Worker::start(&settings).unwrap();
        let old_assertion = worker.assertion.child.id();
        worker.reconfigure(&settings, false).unwrap();
        assert_ne!(worker.assertion.child.id(), old_assertion);

        let missing_process = CaffeinateSettings {
            pid: Some(i32::MAX as u32),
            ..settings
        };
        let new_assertion = worker.assertion.child.id();
        assert_eq!(
            worker.reconfigure(&missing_process, true),
            Err("caffeinateProcessUnavailable")
        );
        assert_eq!(worker.assertion.child.id(), new_assertion);
        assert_eq!(worker.is_running(), Ok(true));

        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        assert_eq!(worker.is_running(), Ok(false));
    }

    #[test]
    fn process_list_uses_only_executable_names_and_skips_its_own_probe() {
        let processes = parse_processes(
            " 30 /Applications/My App.app/Contents/MacOS/My App\n 8 /sbin/Alpha\n 12 /bin/ps\n 0 kernel\nunknown\n",
            12,
        );
        assert_eq!(
            processes,
            vec![
                RunningProcess {
                    pid: 8,
                    name: "Alpha".into()
                },
                RunningProcess {
                    pid: 30,
                    name: "My App".into()
                }
            ]
        );
    }
}
