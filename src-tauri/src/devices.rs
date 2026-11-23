//! What this device shows in the Devices view: its name and its kind.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
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

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteAccess {
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Pairing {
    Closed,
}

/// The list the Devices view draws. Controllers, actions, and other devices come in later steps.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub this_device: ThisDevice,
    pub devices: Vec<()>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThisDevice {
    #[serde(flatten)]
    pub device: Device,
    pub remote_access: RemoteAccess,
    pub pairing: Pairing,
    pub controllers: Vec<()>,
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

/// Reads the system, so call it off the main thread.
pub fn overview() -> Overview {
    Overview {
        this_device: ThisDevice {
            device: platform::this_device(),
            remote_access: RemoteAccess::Unknown,
            pairing: Pairing::Closed,
            controllers: Vec::new(),
            actions: Vec::new(),
        },
        devices: Vec::new(),
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
                remote_access: RemoteAccess::Unknown,
                pairing: Pairing::Closed,
                controllers: Vec::new(),
                actions: Vec::new(),
            },
            devices: Vec::new(),
        })
        .unwrap();
        assert_eq!(value["thisDevice"]["name"], "Studio");
        assert_eq!(value["thisDevice"]["kind"], "laptop");
        assert_eq!(value["thisDevice"]["remoteAccess"], "unknown");
        assert_eq!(value["thisDevice"]["pairing"], "closed");
        assert_eq!(value["devices"], serde_json::json!([]));
    }
}
