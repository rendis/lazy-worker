//! Screen Sharing, the screen service that macOS has. We use it as it is: macOS asks for the
//! password of the account and decides who gets in, so Lazy Worker adds no permission of its own.
//! This module reads the state of the service, turns it on through one administrator prompt, and
//! builds the address that opens the screen of another device.

use super::{
    access::{current_user, group_allows, group_step, run_as_administrator},
    destination::Destination,
};
use crate::lid::safe_user;
use serde::Serialize;
use std::{
    net::{SocketAddr, TcpStream},
    process::{Command, Stdio},
    time::Duration,
};

const GROUP: &str = "com.apple.access_screensharing";
const SERVICE: &str = "system/com.apple.screensharing";
const SERVICE_PLIST: &str = "/System/Library/LaunchDaemons/com.apple.screensharing.plist";
const SHARING_URL: &str = "x-apple.systempreferences:com.apple.preferences.sharing?Services_ScreenSharing";

/// The state of Screen Sharing on a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenSharing {
    /// True while the service listens for connections.
    pub on: bool,
    /// Whether the system lets this user in. Nothing when it could not be read.
    pub user_allowed: Option<bool>,
}

/// Reads the state. It starts a process, so call it off the main thread.
pub fn status() -> ScreenSharing {
    ScreenSharing {
        on: listening(),
        user_allowed: current_user().and_then(|user| group_allows(&user, GROUP)),
    }
}

/// The service listens on port 5900 only while it is on, and the system starts it on the first connection.
fn listening() -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], 5900));
    TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok()
}

/// Turns the service on, then lets this user in when the system keeps an access list.
/// The text holds no double quote and no backslash, so it needs no AppleScript escaping. The user
/// name is the one value that comes from outside, and `safe_user` has checked it. Every step can
/// run again, and a service that is already started is no error.
fn enable_script(user: &str) -> Option<String> {
    if !safe_user(user) {
        return None;
    }
    Some(format!(
        "/bin/launchctl enable {SERVICE}; \
         /bin/launchctl bootstrap system {SERVICE_PLIST} > /dev/null 2>&1; \
         {}; exit 0",
        group_step(user, GROUP)
    ))
}

/// Turns on Screen Sharing with one administrator prompt. Returns the new state, or the first
/// thing that is still wrong. A system that wants the switch in Settings leaves it off, and the
/// view then shows the guide.
pub fn enable() -> Result<ScreenSharing, &'static str> {
    let script = current_user()
        .and_then(|user| enable_script(&user))
        .ok_or("screenChangeFailed")?;
    let prompt = "Lazy Worker needs your permission to turn on screen sharing on this device.";
    run_as_administrator(&script, prompt, "screenChangeFailed")?;
    // The service needs a moment to listen after it starts.
    let mut state = status();
    for _ in 0..6 {
        if state.on {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
        state = status();
    }
    verdict(state).map(|()| state)
}

fn verdict(state: ScreenSharing) -> Result<(), &'static str> {
    if !state.on {
        Err("screenStillOff")
    } else if state.user_allowed == Some(false) {
        Err("userStillNotAllowed")
    } else {
        Ok(())
    }
}

/// Opens Sharing in System Settings at Screen Sharing.
pub fn open_settings() -> Result<(), &'static str> {
    let opened = Command::new("/usr/bin/open")
        .arg(SHARING_URL)
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if opened { Ok(()) } else { Err("sharingSettingsFailed") }
}

/// The address that opens the screen of a device: its user and host, and nothing else. The
/// destination holds only a safe user and a host that is an address or a DNS name, so no part of
/// it can end the address or start a path. An IPv6 address goes in brackets, with `%25` for the zone.
fn vnc_url(destination: &Destination) -> String {
    let host = destination.host();
    let host = if host.contains(':') { format!("[{}]", host.replace('%', "%25")) } else { host.to_owned() };
    format!("vnc://{}@{host}", destination.user())
}

/// Opens Screen Sharing on the screen of the device. It starts a program, so call it off the main thread.
pub fn open(destination: &Destination) -> Result<(), &'static str> {
    let opened = Command::new("/usr/bin/open")
        .arg(vnc_url(destination))
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if opened { Ok(()) } else { Err("screenOpenFailed") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_enable_script_is_fixed_and_safe_for_applescript() {
        let script = enable_script("first.last-2").unwrap();
        assert!(!script.contains('"') && !script.contains('\\'));
        let order = [
            "/bin/launchctl enable system/com.apple.screensharing",
            "/bin/launchctl bootstrap system /System/Library/LaunchDaemons/com.apple.screensharing.plist",
            "dseditgroup -o read com.apple.access_screensharing",
            "dseditgroup -o edit -a first.last-2 -t user com.apple.access_screensharing",
        ];
        let at: Vec<usize> = order.iter().map(|part| script.find(part).unwrap()).collect();
        assert!(at.windows(2).all(|pair| pair[0] < pair[1]));
        // Nothing else: no ARD agent, no other group, no settings tool.
        for banned in ["kickstart", "systemsetup", "sudo", "access_ssh"] {
            assert!(!script.contains(banned), "{banned}");
        }
        assert_eq!(script.matches("-a ").count(), 1);
        // Without the user name the script is the same for any user.
        assert_eq!(script.replace("first.last-2", "x"), enable_script("x").unwrap());
        assert!(enable_script("a; reboot").is_none());
        assert!(enable_script("a\"b").is_none());
        assert!(enable_script("").is_none());
        let check = Command::new("/bin/sh").args(["-n", "-c", &script]).status().unwrap();
        assert!(check.success());
    }

    #[test]
    fn the_verdict_names_what_is_still_wrong() {
        let good = ScreenSharing { on: true, user_allowed: Some(true) };
        assert_eq!(verdict(good), Ok(()));
        assert_eq!(verdict(ScreenSharing { user_allowed: None, ..good }), Ok(()));
        assert_eq!(verdict(ScreenSharing { on: false, ..good }), Err("screenStillOff"));
        assert_eq!(verdict(ScreenSharing { user_allowed: Some(false), ..good }), Err("userStillNotAllowed"));
    }

    #[test]
    fn the_address_holds_the_user_and_the_host_only() {
        let url = |user, host, port| vnc_url(&Destination::new(user, host, port).unwrap());
        assert_eq!(url("alex", "studio.local", None), "vnc://alex@studio.local");
        // The SSH port is not the screen port.
        assert_eq!(url("alex", "192.168.1.20", Some(2222)), "vnc://alex@192.168.1.20");
        assert_eq!(url("first.last-2", "Studio.Local", None), "vnc://first.last-2@studio.local");
        assert_eq!(url("alex", "fd00::1", None), "vnc://alex@[fd00::1]");
        assert_eq!(url("alex", "[fe80::1%en0]", None), "vnc://alex@[fe80::1%25en0]");
    }

    #[test]
    fn nothing_invalid_becomes_an_address() {
        for (user, host) in [
            ("alex", "host/../x"),
            ("alex", "host?x=1"),
            ("alex", "host#x"),
            ("alex", "a@b"),
            ("alex", "-oProxyCommand=x"),
            ("alex", "host name"),
            ("alex", "[1.2.3.4]"),
            ("alex", "fe80::1%en0 x"),
            ("alex", "2001:db8::1%en0"),
            ("a/b", "host"),
            ("a@b", "host"),
            ("a:b", "host"),
            ("-x", "host"),
            ("", "host"),
        ] {
            assert!(Destination::new(user, host, None).is_err(), "{user}@{host}");
        }
    }

    #[test]
    fn the_address_has_no_character_that_ends_or_changes_it() {
        for (user, host) in [("alex", "studio.local"), ("a_b.c-d", "10.0.0.2"), ("alex", "fe80::1%en0")] {
            let url = vnc_url(&Destination::new(user, host, None).unwrap());
            let rest = url.strip_prefix("vnc://").unwrap();
            assert_eq!(rest.matches('@').count(), 1);
            assert!(!rest.contains(['/', '?', '#', ' ', '\\', '"', '\'', '\n']));
        }
    }
}
