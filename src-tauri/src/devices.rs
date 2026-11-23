//! What this device shows in the Devices view: its name and its kind.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Laptop,
    Desktop,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Empty when the system gives no name. The webview then shows its own words.
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteLogin {
    Off,
    On,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PasswordLogin {
    Blocked,
    Allowed,
    Unknown,
}

/// The state of the system SSH server, which other devices use to reach this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccess {
    pub remote_login: RemoteLogin,
    pub password_login: PasswordLogin,
    /// Whether the system lets this user log in over SSH. Nothing when it could not be read.
    pub user_allowed: Option<bool>,
    /// True while our own drop-in file is in the SSH server configuration.
    pub managed_file: bool,
    /// The major version of macOS, which decides what the System Settings guide shows.
    pub macos_major: Option<u32>,
}

impl RemoteAccess {
    /// For a platform that has no system SSH server to read.
    #[cfg(not(unix))]
    pub fn unavailable() -> Self {
        Self {
            remote_login: RemoteLogin::Off,
            password_login: PasswordLogin::Unknown,
            user_allowed: None,
            managed_file: false,
            macos_major: None,
        }
    }
}

/// Whether this device lets another one pair with it now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub open: bool,
    /// Unix seconds when the window closes, or nothing while it is closed.
    pub expires: Option<u64>,
    pub attempts_left: u8,
}

impl Pairing {
    pub fn closed() -> Self {
        Self {
            open: false,
            expires: None,
            attempts_left: 0,
        }
    }
}

/// A device that this one controls, as the Devices view shows it. The lists after `online` come
/// in later steps and are empty until then.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub user: String,
    pub host: String,
    pub port: u16,
    pub added: u64,
    /// Nothing until the first reachability check ends.
    pub online: Option<bool>,
    pub tools: Vec<()>,
    pub apps: RemoteApps,
    pub actions: Vec<()>,
    pub shell: Option<()>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RemoteApps {
    pub pinned: Vec<()>,
    pub total: u32,
    pub open: u32,
}

#[cfg(unix)]
impl Remote {
    pub fn new(device: &crate::remote::controlled::Controlled, online: Option<bool>) -> Self {
        Self {
            id: device.id.clone(),
            name: device.name.clone(),
            kind: device.kind,
            user: device.user.clone(),
            host: device.host.clone(),
            port: device.port,
            added: device.added,
            online,
            tools: Vec::new(),
            apps: RemoteApps::default(),
            actions: Vec::new(),
            shell: None,
        }
    }
}

/// A device that may control this one, as the Devices view shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Controller {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Unix seconds, or nothing before its first request.
    pub last_access: Option<u64>,
    /// True while it has an open connection.
    pub connected: bool,
    pub permissions: ControllerPermissions,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerPermissions {
    pub control: bool,
    pub applications: bool,
    pub actions: bool,
    pub shell: bool,
}

#[cfg(unix)]
impl Controller {
    pub fn new(peer: &crate::remote::peers::Peer, connected: bool) -> Self {
        Self {
            id: peer.id.clone(),
            name: peer.name.clone(),
            kind: peer.kind,
            last_access: peer.last_access,
            connected,
            permissions: ControllerPermissions {
                control: peer.permissions.control,
                applications: peer.permissions.applications,
                actions: peer.permissions.actions,
                shell: peer.permissions.shell,
            },
        }
    }
}

/// Whether the app socket that other devices reach is listening, and why not when it is not.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Socket {
    pub listening: bool,
    pub error: Option<String>,
}

impl Socket {
    #[cfg(unix)]
    pub fn listening() -> Self {
        Self { listening: true, error: None }
    }

    pub fn unavailable(error: &str) -> Self {
        Self { listening: false, error: Some(error.to_owned()) }
    }
}

/// The list the Devices view draws. Actions and other devices come in later steps.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub this_device: ThisDevice,
    pub devices: Vec<Remote>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThisDevice {
    #[serde(flatten)]
    pub device: Device,
    pub remote_access: RemoteAccess,
    pub pairing: Pairing,
    pub controllers: Vec<Controller>,
    pub socket: Socket,
    pub actions: Vec<()>,
}

#[cfg(target_os = "macos")]
#[path = "devices/macos.rs"]
mod platform;

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{Device, Kind};

    pub fn this_device() -> Device {
        let name = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_default();
        Device {
            name: name.trim().to_owned(),
            kind: Kind::Desktop,
        }
    }
}

/// What the Devices view reads from the stores, before the system is asked anything.
pub struct Stores {
    pub controllers: Vec<Controller>,
    pub socket: Socket,
    pub pairing: Pairing,
    pub devices: Vec<Remote>,
}

/// This device by its name and kind. It runs commands, so call it off the main thread.
#[cfg(unix)]
pub fn this_device() -> Device {
    platform::this_device()
}

/// Reads the system, so call it off the main thread.
pub fn overview(stores: Stores, remote_access: RemoteAccess) -> Overview {
    Overview {
        this_device: ThisDevice {
            device: platform::this_device(),
            remote_access,
            pairing: stores.pairing,
            controllers: stores.controllers,
            socket: stores.socket,
            actions: Vec::new(),
        },
        devices: stores.devices,
    }
}

/// `pmset -g batt` lists an `InternalBattery` item on a laptop. A desktop lists only its power source.
#[cfg(any(target_os = "macos", test))]
fn kind_from_power_report(report: &str) -> Kind {
    if report.contains("InternalBattery") {
        Kind::Laptop
    } else {
        Kind::Desktop
    }
}

/// The name a command printed: one trimmed line, or nothing when it is empty.
#[cfg(any(target_os = "macos", test))]
fn name_from_output(output: &str) -> Option<String> {
    let name = output.lines().next()?.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_power_report_with_an_internal_battery_is_a_laptop() {
        let report = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1) 87%; discharging; 5:12 remaining present: true\n";
        assert_eq!(kind_from_power_report(report), Kind::Laptop);
    }

    #[test]
    fn a_power_report_without_a_battery_is_a_desktop() {
        assert_eq!(
            kind_from_power_report("Now drawing from 'AC Power'\n"),
            Kind::Desktop
        );
        assert_eq!(kind_from_power_report(""), Kind::Desktop);
    }

    #[test]
    fn a_name_is_the_first_line_without_its_edges() {
        assert_eq!(
            name_from_output("  Reynaldo’s MacBook Pro \n"),
            Some("Reynaldo’s MacBook Pro".into())
        );
        assert_eq!(name_from_output("\n"), None);
        assert_eq!(name_from_output(""), None);
    }

    #[test]
    fn the_overview_uses_the_names_the_webview_reads() {
        let value = serde_json::to_value(Overview {
            this_device: ThisDevice {
                device: Device {
                    name: "Studio".into(),
                    kind: Kind::Laptop,
                },
                remote_access: RemoteAccess {
                    remote_login: RemoteLogin::On,
                    password_login: PasswordLogin::Blocked,
                    user_allowed: None,
                    managed_file: true,
                    macos_major: Some(26),
                },
                pairing: Pairing {
                    open: true,
                    expires: Some(1_800_000_000),
                    attempts_left: 2,
                },
                controllers: Vec::new(),
                socket: Socket::unavailable("x"),
                actions: Vec::new(),
            },
            devices: Vec::new(),
        })
        .unwrap();
        assert_eq!(value["thisDevice"]["name"], "Studio");
        assert_eq!(value["thisDevice"]["kind"], "laptop");
        assert_eq!(
            value["thisDevice"]["remoteAccess"],
            serde_json::json!({
                "remoteLogin": "on", "passwordLogin": "blocked", "userAllowed": null, "managedFile": true,
                "macosMajor": 26
            })
        );
        assert_eq!(
            value["thisDevice"]["pairing"],
            serde_json::json!({"open": true, "expires": 1_800_000_000u64, "attemptsLeft": 2})
        );
        assert_eq!(
            serde_json::to_value(Pairing::closed()).unwrap(),
            serde_json::json!({"open": false, "expires": null, "attemptsLeft": 0})
        );
        assert_eq!(value["devices"], serde_json::json!([]));
        assert_eq!(value["thisDevice"]["socket"]["listening"], false);
        assert_eq!(value["thisDevice"]["socket"]["error"], "x");
    }
}
