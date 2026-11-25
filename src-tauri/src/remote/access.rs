//! The state of the system SSH server on this device, and the one change we make to it.
//!
//! Other devices reach this one over Remote Login. We never turn it on: the user does that in
//! System Settings. We only read its state, and we keep passwords out of it with one drop-in file
//! of our own in `sshd_config.d`. The file is written through the administrator prompt with a
//! fixed script, and the server's own `sshd -t` must accept it before it stays.

use super::ssh::tool;
use crate::{
    devices::{PasswordLogin, RemoteAccess, RemoteLogin},
    lid::safe_user,
};
use std::{
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

const SSH: &str = "/usr/bin/ssh";
const SSHD_CONFIG: &str = "/etc/ssh/sshd_config";
const DROP_IN_DIR: &str = "/etc/ssh/sshd_config.d";
const DROP_IN: &str = "/etc/ssh/sshd_config.d/000-lazy-worker.conf";
const DROP_IN_LINES: [&str; 3] = [
    "# Lazy Worker: only keys can log in over SSH. Remove this file to allow passwords again.",
    "PasswordAuthentication no",
    "AuthenticationMethods publickey",
];
const GROUP: &str = "com.apple.access_ssh";
const SERVICE: &str = "system/com.openssh.sshd";
const SERVICE_PLIST: &str = "/System/Library/LaunchDaemons/ssh.plist";
const SHARING_URL: &str = "x-apple.systempreferences:com.apple.preferences.sharing?Services_RemoteLogin";

/// Reads the state. It starts processes, so call it off the main thread.
pub fn status() -> RemoteAccess {
    let on = remote_login_on();
    let user = current_user();
    RemoteAccess {
        remote_login: if on { RemoteLogin::On } else { RemoteLogin::Off },
        password_login: match (&user, on) {
            (Some(user), true) => password_login(user),
            _ => PasswordLogin::Unknown,
        },
        user_allowed: user.as_deref().and_then(|user| group_allows(user, GROUP)),
        managed_file: Path::new(DROP_IN).exists(),
        macos_major: macos_major(),
    }
}

fn macos_major() -> Option<u32> {
    let output = tool("/usr/bin/sw_vers")
        .arg("-productVersion")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    parse_major(&String::from_utf8_lossy(&output.stdout))
}

/// `12.7.6` and `26.6.2` give 12 and 26.
fn parse_major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

fn remote_login_on() -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], 22));
    TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok()
}

/// The user of this process, or nothing when the name is not one that is safe to pass on.
pub(super) fn current_user() -> Option<String> {
    let output = tool("/usr/bin/id")
        .arg("-un")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let user = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && safe_user(&user)).then_some(user)
}

/// Asks the local server which methods it offers, without trying any.
fn password_login(user: &str) -> PasswordLogin {
    let output = tool(SSH)
        .args(["-F", "/dev/null"])
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "PreferredAuthentications=none"])
        .args(["-o", "StrictHostKeyChecking=no"])
        .args(["-o", "UserKnownHostsFile=/dev/null"])
        .args(["-o", "GlobalKnownHostsFile=/dev/null"])
        .args(["-o", "ConnectTimeout=3"])
        .args(["-o", "LogLevel=ERROR"])
        .args(["-l", user, "--", "127.0.0.1", "true"])
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(output) => parse_methods(&String::from_utf8_lossy(&output.stderr)),
        Err(_) => PasswordLogin::Unknown,
    }
}

/// Reads `Permission denied (publickey,password).` from the client.
fn parse_methods(stderr: &str) -> PasswordLogin {
    let Some(start) = stderr.find("Permission denied (") else {
        return PasswordLogin::Unknown;
    };
    let rest = &stderr[start + "Permission denied (".len()..];
    let Some(end) = rest.find(')') else {
        return PasswordLogin::Unknown;
    };
    let methods: Vec<&str> = rest[..end].split(',').map(str::trim).collect();
    if methods.iter().any(|m| matches!(*m, "password" | "keyboard-interactive")) {
        PasswordLogin::Allowed
    } else if methods == ["publickey"] {
        PasswordLogin::Blocked
    } else {
        PasswordLogin::Unknown
    }
}

/// Whether the system lets `user` use a service that has this access group. Without the group, it lets everyone.
pub(super) fn group_allows(user: &str, group: &str) -> Option<bool> {
    let output = Command::new("/usr/sbin/dseditgroup")
        .args(["-o", "checkmember", "-m", user, group])
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .ok()?;
    parse_membership(
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// `yes <user> is a member of ...` or `no <user> is NOT a member of ...`. Without the group,
/// the system lets every user in.
pub(super) fn parse_membership(stdout: &str, stderr: &str) -> Option<bool> {
    match stdout.split_whitespace().next() {
        Some("yes") => Some(true),
        Some("no") => Some(false),
        _ if stdout.contains("Group not found") || stderr.contains("Group not found") => Some(true),
        _ => None,
    }
}

/// True when the server configuration reads the drop-in folder.
fn includes_drop_ins() -> bool {
    std::fs::read_to_string(SSHD_CONFIG).is_ok_and(|text| config_includes_drop_ins(&text))
}

fn config_includes_drop_ins(config: &str) -> bool {
    config.lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next().is_some_and(|word| word.eq_ignore_ascii_case("Include"))
            && words.any(|word| word.starts_with(DROP_IN_DIR))
    })
}

/// The steps that write our drop-in. The text holds no double quote and no backslash, so it
/// needs no AppleScript escaping, and no data from outside. The file is copied in place, `sshd -t`
/// checks it, and the file goes again when the server refuses it. The steps end with `status`
/// at 0 only when the file stays.
fn drop_in_steps() -> String {
    let lines = DROP_IN_LINES
        .iter()
        .map(|line| format!("echo '{line}'"))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "/usr/bin/grep -Eqi '^[[:space:]]*Include[[:space:]].*{DROP_IN_DIR}/' {SSHD_CONFIG} || exit 3; \
         [ -d {DROP_IN_DIR} ] || /bin/mkdir -m 755 {DROP_IN_DIR} || exit 1; \
         tmp=$(/usr/bin/mktemp) || exit 1; \
         {{ {lines}; }} > $tmp; \
         /usr/bin/install -m 0644 -o root -g wheel $tmp {DROP_IN}; status=$?; \
         /bin/rm -f $tmp; \
         [ $status -eq 0 ] && /usr/sbin/sshd -t; status=$?; \
         [ $status -eq 0 ] || /bin/rm -f {DROP_IN}; "
    )
}

/// Adds `user` to the access group when the group exists and the user is not in it. It changes no other user.
/// The caller has checked the user name with `safe_user`.
pub(super) fn group_step(user: &str, group: &str) -> String {
    format!(
        "if /usr/sbin/dseditgroup -o read {group} > /dev/null 2>&1; then \
         /usr/sbin/dseditgroup -o checkmember -m {user} {group} > /dev/null 2>&1 \
         || /usr/sbin/dseditgroup -o edit -a {user} -t user {group} || exit 4; fi"
    )
}

/// Keys only, then the user in the access group, then Remote Login itself. Every step can run
/// again. `systemsetup` is not used because it needs Full Disk Access. The user name is the one
/// value that comes from outside, and `safe_user` has checked it.
fn enable_script(user: &str) -> Option<String> {
    if !safe_user(user) {
        return None;
    }
    let group = group_step(user, GROUP);
    Some(format!(
        "{} [ $status -eq 0 ] || exit $status; {group}; \
         /bin/launchctl enable {SERVICE}; \
         /bin/launchctl bootstrap system {SERVICE_PLIST} > /dev/null 2>&1; exit 0",
        drop_in_steps()
    ))
}

/// Removes only our file.
fn allow_script() -> String {
    format!("/bin/rm -f {DROP_IN}")
}

/// Runs a script through the macOS administrator prompt.
/// `failed` is the error for any failure that is not a cancelled prompt.
pub(super) fn run_as_administrator(script: &str, prompt: &str, failed: &'static str) -> Result<(), &'static str> {
    let result = Command::new("/usr/bin/osascript")
        .args([
            "-e",
            &format!("do shell script \"{script}\" with prompt \"{prompt}\" with administrator privileges"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .map_err(|_| failed)?;
    if result.status.success() {
        Ok(())
    } else if String::from_utf8_lossy(&result.stderr).contains("-128") {
        Err("adminCanceled")
    } else {
        Err(failed)
    }
}

/// Checks that the device ended up as `enable_remote_access` meant.
fn verdict(status: &RemoteAccess) -> Result<(), &'static str> {
    if status.remote_login != RemoteLogin::On {
        Err("remoteLoginStillOff")
    } else if status.user_allowed == Some(false) {
        Err("userStillNotAllowed")
    } else if status.password_login != PasswordLogin::Blocked {
        Err("passwordLoginStillAllowed")
    } else {
        Ok(())
    }
}

/// Turns on remote access with one administrator prompt: keys only, this user allowed, Remote
/// Login on. Returns the new state, or the first thing that is still wrong.
pub fn enable_remote_access() -> Result<RemoteAccess, &'static str> {
    if !includes_drop_ins() {
        return Err("sshdNoInclude");
    }
    let script = current_user()
        .and_then(|user| enable_script(&user))
        .ok_or("remoteAccessChangeFailed")?;
    let prompt = "Lazy Worker needs your permission to turn on remote access to this device, for keys only.";
    run_as_administrator(&script, prompt, "remoteAccessChangeFailed")?;
    // The server needs a moment to listen after it starts.
    let mut status = status();
    for _ in 0..6 {
        if status.remote_login == RemoteLogin::On {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
        status = self::status();
    }
    verdict(&status).map(|()| status)
}

/// Removes our file, so the server applies its own settings again.
pub fn allow_password_login() -> Result<RemoteAccess, &'static str> {
    let prompt = "Lazy Worker needs your permission to allow passwords for SSH log in to this device again.";
    run_as_administrator(&allow_script(), prompt, "remoteAccessChangeFailed")?;
    Ok(status())
}

/// Opens Sharing in System Settings. On macOS 13 and later the system turns this older address
/// into the Sharing pane with the Remote Login sheet open.
pub fn open_sharing_settings() -> Result<(), &'static str> {
    let opened = Command::new("/usr/bin/open")
        .arg(SHARING_URL)
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if opened { Ok(()) } else { Err("sharingSettingsFailed") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::testing::scratch;
    use std::fs;

    #[test]
    fn reads_the_methods_a_server_offers() {
        // OpenSSH 8.6 on macOS 12 with its defaults.
        let open = "alex@127.0.0.1: Permission denied (publickey,password,keyboard-interactive).\n";
        assert_eq!(parse_methods(open), PasswordLogin::Allowed);
        // OpenSSH 10 with the drop-in.
        assert_eq!(parse_methods("alex@127.0.0.1: Permission denied (publickey).\n"), PasswordLogin::Blocked);
        assert_eq!(parse_methods("Permission denied (publickey,password).\n"), PasswordLogin::Allowed);
        assert_eq!(parse_methods("Permission denied (keyboard-interactive).\n"), PasswordLogin::Allowed);
        assert_eq!(parse_methods("Permission denied (gssapi-with-mic).\n"), PasswordLogin::Unknown);
        assert_eq!(parse_methods("Permission denied (publickey,gssapi-with-mic).\n"), PasswordLogin::Unknown);
        assert_eq!(parse_methods("ssh: connect to host 127.0.0.1 port 22: Connection refused\n"), PasswordLogin::Unknown);
        assert_eq!(parse_methods("Permission denied (publickey"), PasswordLogin::Unknown);
        assert_eq!(parse_methods(""), PasswordLogin::Unknown);
    }

    #[test]
    fn reads_the_group_answer() {
        assert_eq!(parse_membership("yes alex is a member of com.apple.access_ssh\n", ""), Some(true));
        assert_eq!(parse_membership("no alex is NOT a member of com.apple.access_ssh\n", ""), Some(false));
        assert_eq!(parse_membership("Group not found.\n", ""), Some(true));
        assert_eq!(parse_membership("", "Group not found.\n"), Some(true));
        assert_eq!(parse_membership("Unable to find the user record\n", ""), None);
        assert_eq!(parse_membership("", ""), None);
    }

    #[test]
    fn finds_the_include_line_and_ignores_comments() {
        assert!(config_includes_drop_ins("Include /etc/ssh/sshd_config.d/*\nUsePAM yes\n"));
        assert!(config_includes_drop_ins("  include /etc/ssh/sshd_config.d/*\n"));
        assert!(!config_includes_drop_ins("# Include /etc/ssh/sshd_config.d/*\n"));
        assert!(!config_includes_drop_ins("UsePAM yes\n"));
    }

    #[test]
    fn the_scripts_are_fixed_and_safe_for_applescript() {
        let block = enable_script("first.last-2").unwrap();
        for script in [&block, &allow_script()] {
            assert!(!script.contains('"') && !script.contains('\\'));
            assert!(script.contains(DROP_IN));
        }
        assert!(block.contains("/usr/sbin/sshd -t"));
        assert!(block.contains("-o root -g wheel"));
        for line in DROP_IN_LINES {
            assert!(block.contains(&format!("echo '{line}'")));
        }
        assert!(!block.contains("KbdInteractiveAuthentication"));
        assert_eq!(allow_script(), "/bin/rm -f /etc/ssh/sshd_config.d/000-lazy-worker.conf");
        // The shell accepts both as they are.
        for script in [block, allow_script()] {
            let check = Command::new("/bin/sh").args(["-n", "-c", &script]).status().unwrap();
            assert!(check.success());
        }
    }

    #[test]
    fn the_enable_script_has_one_interpolated_value_and_the_steps_in_order() {
        let script = enable_script("first.last-2").unwrap();
        assert!(!script.contains('"') && !script.contains('\\'));
        let order = [
            "/usr/sbin/sshd -t",
            "dseditgroup -o edit -a first.last-2 -t user com.apple.access_ssh",
            "/bin/launchctl enable system/com.openssh.sshd",
            "/bin/launchctl bootstrap system /System/Library/LaunchDaemons/ssh.plist",
        ];
        let at: Vec<usize> = order.iter().map(|part| script.find(part).unwrap()).collect();
        assert!(at.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!script.contains("systemsetup"));
        // Without the user name the script is the same as for any other user.
        assert_eq!(script.replace("first.last-2", "x"), enable_script("x").unwrap());
        assert!(enable_script("a; reboot").is_none());
        assert!(enable_script("").is_none());
        let check = Command::new("/bin/sh").args(["-n", "-c", &script]).status().unwrap();
        assert!(check.success());
    }

    #[test]
    fn the_verdict_names_the_first_thing_still_wrong() {
        let good = RemoteAccess {
            remote_login: RemoteLogin::On,
            password_login: PasswordLogin::Blocked,
            user_allowed: Some(true),
            managed_file: true,
            macos_major: Some(26),
        };
        assert_eq!(verdict(&good), Ok(()));
        let off = RemoteAccess { remote_login: RemoteLogin::Off, password_login: PasswordLogin::Unknown, ..good };
        assert_eq!(verdict(&off), Err("remoteLoginStillOff"));
        let user = RemoteAccess { user_allowed: Some(false), password_login: PasswordLogin::Allowed, ..good };
        assert_eq!(verdict(&user), Err("userStillNotAllowed"));
        let open = RemoteAccess { password_login: PasswordLogin::Allowed, ..good };
        assert_eq!(verdict(&open), Err("passwordLoginStillAllowed"));
        assert_eq!(verdict(&RemoteAccess { user_allowed: None, ..good }), Ok(()));
    }

    #[test]
    fn reads_the_macos_major_version() {
        assert_eq!(parse_major("12.7.6\n"), Some(12));
        assert_eq!(parse_major("26.6.2"), Some(26));
        assert_eq!(parse_major(""), None);
    }

    #[test]
    fn the_sshd_test_accepts_the_drop_in_and_applies_it() {
        if !Path::new("/usr/sbin/sshd").exists() {
            return;
        }
        let dir = scratch("sshd");
        let key = dir.join("host_key");
        let keygen = Command::new("/usr/bin/ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", ""])
            .arg("-f")
            .arg(&key)
            .status()
            .unwrap();
        assert!(keygen.success());
        let drop_ins = dir.join("sshd_config.d");
        fs::create_dir(&drop_ins).unwrap();
        fs::write(drop_ins.join("000-lazy-worker.conf"), DROP_IN_LINES.join("\n") + "\n").unwrap();
        let config = dir.join("sshd_config");
        fs::write(
            &config,
            format!("Include {}/*\nPasswordAuthentication yes\nUsePAM no\n", drop_ins.display()),
        )
        .unwrap();
        let sshd = |flag: &str| {
            Command::new("/usr/sbin/sshd")
                .arg(flag)
                .arg("-f")
                .arg(&config)
                .arg("-h")
                .arg(&key)
                .output()
                .unwrap()
        };
        let checked = sshd("-t");
        assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
        let effective = String::from_utf8_lossy(&sshd("-T").stdout).to_lowercase();
        assert!(effective.contains("passwordauthentication no"));
        assert!(effective.contains("authenticationmethods publickey"));
        fs::remove_dir_all(dir).unwrap();
    }
}
