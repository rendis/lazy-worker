use std::{
    io::{self, BufRead, BufReader},
    process::{Child, Command, Stdio},
};

pub const SUPPORTED: bool = true;

fn helper_command() -> io::Result<Command> {
    let executable = std::env::current_exe()?.with_file_name("activity-helper");
    Ok(Command::new(executable))
}

pub fn accessibility_status() -> &'static str {
    let Ok(output) = helper_command().and_then(|mut command| {
        command
            .arg("--accessibility-status")
            .stderr(Stdio::null())
            .output()
    }) else {
        return "unavailable";
    };

    match (output.status.success(), output.stdout.as_slice()) {
        (true, b"granted\n") => "granted",
        (true, b"required\n") => "required",
        _ => "unavailable",
    }
}

pub fn request_accessibility() -> Result<(), &'static str> {
    let status = Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if status.is_ok_and(|status| status.success()) {
        Ok(())
    } else {
        Err("accessibilityRequestFailed")
    }
}

pub struct Worker(Child);

impl Worker {
    pub fn start() -> Result<Self, &'static str> {
        let child = helper_command()
            .map_err(|_| "workerUnavailable")?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "workerUnavailable")?;

        let mut worker = Self(child);
        let mut response = String::new();
        let Some(stdout) = worker.0.stdout.take() else {
            return Err("workerUnavailable");
        };
        if BufReader::new(stdout).read_line(&mut response).is_err() {
            return Err("workerUnavailable");
        }

        match response.trim() {
            "ready" => Ok(worker),
            "permissionRequired" => Err("permissionRequired"),
            "idleMonitorUnavailable" => Err("idleMonitorUnavailable"),
            _ => Err("workerUnavailable"),
        }
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.0.stdin.take();
        if self.is_running() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
