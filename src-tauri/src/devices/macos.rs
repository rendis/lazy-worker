use super::{kind_from_power_report, name_from_output, Device};
use std::process::{Command, Stdio};

/// Runs a system tool with a fixed path and no shell. Its output, or nothing when it fails.
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

pub fn this_device() -> Device {
    let name = output("/usr/sbin/scutil", &["--get", "ComputerName"])
        .and_then(|name| name_from_output(&name))
        .or_else(|| output("/bin/hostname", &[]).and_then(|name| name_from_output(&name)))
        .unwrap_or_default();
    // `hw.model` is `MacNN,N` on Apple silicon, so the battery tells a laptop from a desktop.
    let kind = output("/usr/bin/pmset", &["-g", "batt"])
        .map(|report| kind_from_power_report(&report))
        .unwrap_or(super::Kind::Desktop);
    Device { name, kind }
}
