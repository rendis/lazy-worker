//! The one command the webview can call, and the barriers in front of every operation.
//!
//! Every request declares its access in `Request::access`. The origin of a request comes from
//! the label of the window that sent it, never from data in the request.

use crate::{
    activity, caffeinate,
    caffeinate::CaffeinateSettings,
    lid,
    lid::LidMode,
    preferences::{Language, ManualDuration, Preferences, Schedule, Theme},
    proxy,
    proxy::{ProxyApp, Role},
    Controller, FirstRun, ToolId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, State, WebviewWindow};

/// What a request needs from its origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Access {
    /// Reads and settings that cannot run a command or change who may connect.
    Control,
    /// Starts a command the user wrote.
    Actions,
    /// Settings that store a command, an address list, or a permission, and system prompts.
    LocalOnly,
}

/// What a remote device may do. All off by default.
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(not(test), allow(dead_code))] // Only tests build a remote origin until remote windows exist.
pub struct Permissions {
    pub control: bool,
    pub actions: bool,
}

/// Where a request comes from.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(test), allow(dead_code))] // Only tests build a remote origin until remote windows exist.
pub enum Origin {
    Local,
    Remote(Permissions),
}

impl Origin {
    /// Decides the origin from the label of the calling window. Only `main` is local.
    fn of_window(label: &str) -> Result<Self, &'static str> {
        match label {
            "main" => Ok(Self::Local),
            _ => Err("notPermitted"),
        }
    }

    /// Checks the access a request needs. Local is always permitted.
    fn permits(self, access: Access) -> Result<(), &'static str> {
        let Self::Remote(permissions) = self else {
            return Ok(());
        };
        match access {
            Access::Control if permissions.control => Ok(()),
            Access::Actions if permissions.actions => Ok(()),
            Access::LocalOnly => Err("localOnly"),
            Access::Control | Access::Actions => Err("notPermitted"),
        }
    }

    /// A remote origin may only check the server address that is saved.
    fn may_check_server(self, saved: &str, address: &str) -> bool {
        matches!(self, Self::Local) || address == saved
    }

    /// Decides whether a lid change opens the administrator prompt.
    /// A remote origin never opens it: the one-time permission is given at the Mac.
    fn lid_prompt(self, changes_system: bool, authorized: bool) -> Result<bool, &'static str> {
        match self {
            Self::Local => Ok(changes_system),
            Self::Remote(_) if !changes_system || authorized => Ok(false),
            Self::Remote(_) => Err("lidNeedsLocalPermission"),
        }
    }
}

#[derive(Deserialize)]
#[serde(
    tag = "command",
    content = "args",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Request {
    GetPreferences,
    GetFirstRun,
    GetSystemLanguage,
    GetStatuses,
    GetLidStatus,
    GetLoginLaunch,
    GetUpdateStatus,
    GetActivityAccessibility,
    GetProxyNetwork,
    GetProxyPeers,
    GetProxyLog,
    DetectProxyApps,
    ListCaffeinateProcesses,
    CheckForUpdate,
    RestartToUpdate,
    SetLanguage { language: Language },
    SetTheme { theme: Theme },
    SetDuration { duration: ManualDuration },
    SetSchedule { schedule: Schedule },
    SetQuickStart { tool: ToolId, include: bool },
    SetStartToolsOnLaunch { enabled: bool },
    SetHideWhileSharing { enabled: bool },
    SetSeenVersion { version: Option<String> },
    SaveCaffeinateSettings { settings: CaffeinateSettings },
    SetProxyRole { role: Role },
    SetProxyPort { port: u16 },
    SetProxyServer { server: String },
    SetToolActive { tool: ToolId, active: bool },
    SetLoginLaunch { enabled: bool },
    SetLidMode { mode: LidMode },
    CheckProxyServer { address: String },
    LaunchProxyApp { id: String },
    SaveProxyApps { apps: Vec<ProxyApp> },
    SaveProxyAllowed { allowed: Vec<String> },
    RequestActivityAccessibility,
}

impl Request {
    /// The access this request needs. There is no default arm: a new request must declare its access.
    fn access(&self, preferences: &Preferences) -> Access {
        match self {
            Self::GetPreferences
            | Self::GetFirstRun
            | Self::GetSystemLanguage
            | Self::GetStatuses
            | Self::GetLidStatus
            | Self::GetLoginLaunch
            | Self::GetUpdateStatus
            | Self::GetActivityAccessibility
            | Self::GetProxyNetwork
            | Self::GetProxyPeers
            | Self::GetProxyLog
            | Self::DetectProxyApps
            | Self::ListCaffeinateProcesses
            | Self::CheckForUpdate
            | Self::RestartToUpdate
            | Self::SetLanguage { .. }
            | Self::SetTheme { .. }
            | Self::SetDuration { .. }
            | Self::SetSchedule { .. }
            | Self::SetQuickStart { .. }
            | Self::SetStartToolsOnLaunch { .. }
            | Self::SetHideWhileSharing { .. }
            | Self::SetSeenVersion { .. }
            | Self::SaveCaffeinateSettings { .. }
            | Self::SetProxyRole { .. }
            | Self::SetProxyPort { .. }
            | Self::SetProxyServer { .. }
            | Self::SetToolActive { .. }
            | Self::SetLoginLaunch { .. }
            | Self::SetLidMode { .. }
            | Self::CheckProxyServer { .. } => Access::Control,
            Self::LaunchProxyApp { id } => {
                let own_command = preferences
                    .proxy
                    .apps
                    .iter()
                    .any(|app| app.id == *id && app.command.is_some());
                if own_command {
                    Access::Actions
                } else {
                    Access::Control
                }
            }
            Self::SaveProxyApps { .. }
            | Self::SaveProxyAllowed { .. }
            | Self::RequestActivityAccessibility => Access::LocalOnly,
        }
    }
}

#[tauri::command]
pub async fn dispatch(
    window: WebviewWindow,
    controller: State<'_, Controller>,
    request: Request,
) -> Result<Value, &'static str> {
    let origin = Origin::of_window(window.label())?;
    let preferences = controller
        .lock()
        .map_err(|_| "workerUnavailable")?
        .activity
        .preferences();
    origin.permits(request.access(&preferences))?;
    run(request, origin, window.app_handle(), &controller).await
}

fn json<T: Serialize>(value: T) -> Result<Value, &'static str> {
    serde_json::to_value(value).map_err(|_| "workerUnavailable")
}

/// Saves one change to the preferences. Server-owned fields stay as they are, because only the changed field is written.
fn update(
    app: &AppHandle,
    controller: &Controller,
    change: impl FnOnce(&mut Preferences),
) -> Result<Value, &'static str> {
    let mut tools = controller.lock().map_err(|_| "saveFailed")?;
    let mut preferences = tools.activity.preferences();
    let hide_while_sharing = preferences.hide_while_sharing;
    change(&mut preferences);
    preferences.proxy.sanitize();
    let proxy = preferences.proxy.clone();
    let presentation_changed = preferences.hide_while_sharing != hide_while_sharing;
    tools.activity.save_preferences(preferences)?;
    tools.proxy.apply_settings(&proxy);
    drop(tools);
    #[cfg(target_os = "macos")]
    if presentation_changed {
        crate::presentation::refresh(app);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, presentation_changed);
    Ok(Value::Null)
}

async fn run(
    request: Request,
    origin: Origin,
    app: &AppHandle,
    controller: &Controller,
) -> Result<Value, &'static str> {
    let locked = |code: &'static str| controller.lock().map_err(move |_| code);
    match request {
        Request::GetPreferences => json(locked("workerUnavailable")?.activity.preferences()),
        Request::GetFirstRun => json(app.state::<FirstRun>().0),
        Request::GetSystemLanguage => {
            #[cfg(target_os = "macos")]
            return json(Some(crate::preferences::system_language()));
            #[cfg(not(target_os = "macos"))]
            json(None::<Language>)
        }
        Request::GetStatuses => json(locked("workerUnavailable")?.statuses()),
        Request::GetLidStatus => json(locked("workerUnavailable")?.lid.status()),
        Request::GetLoginLaunch => {
            #[cfg(target_os = "macos")]
            return json(crate::login_launch::get_login_launch(app.clone())?);
            #[cfg(not(target_os = "macos"))]
            Err("unsupported")
        }
        Request::GetUpdateStatus => {
            #[cfg(target_os = "macos")]
            return json(crate::updates::get_update_status(app.clone()));
            #[cfg(not(target_os = "macos"))]
            Err("unsupported")
        }
        Request::GetActivityAccessibility => json(
            tauri::async_runtime::spawn_blocking(activity::accessibility_status)
                .await
                .unwrap_or("unavailable"),
        ),
        Request::GetProxyNetwork => json(
            tauri::async_runtime::spawn_blocking(proxy::network)
                .await
                .map_err(|_| "workerUnavailable")?,
        ),
        Request::GetProxyPeers => json(locked("workerUnavailable")?.proxy.peers()),
        Request::GetProxyLog => json(locked("workerUnavailable")?.proxy.log()),
        Request::DetectProxyApps => json(
            tauri::async_runtime::spawn_blocking(proxy::detect)
                .await
                .map_err(|_| "workerUnavailable")?,
        ),
        Request::ListCaffeinateProcesses => json(
            tauri::async_runtime::spawn_blocking(caffeinate::list_processes)
                .await
                .map_err(|_| "caffeinateProcessListFailed")??,
        ),
        Request::CheckForUpdate => {
            #[cfg(target_os = "macos")]
            return json(crate::updates::check_for_update(app.clone()).await?);
            #[cfg(not(target_os = "macos"))]
            Err("unsupported")
        }
        Request::RestartToUpdate => {
            #[cfg(target_os = "macos")]
            return json(crate::updates::restart_to_update(app.clone()).await?);
            #[cfg(not(target_os = "macos"))]
            Err("unsupported")
        }
        Request::SetLanguage { language } => update(app, controller, |p| p.language = language),
        Request::SetTheme { theme } => update(app, controller, |p| p.theme = theme),
        Request::SetDuration { duration } => update(app, controller, |p| p.duration = duration),
        Request::SetSchedule { schedule } => update(app, controller, |p| p.schedule = schedule),
        Request::SetQuickStart { tool, include } => update(app, controller, |p| match tool {
            ToolId::Activity => p.quick_start.activity = include,
            ToolId::Caffeinate => p.quick_start.caffeinate = include,
            ToolId::Proxy => p.quick_start.proxy = include,
        }),
        Request::SetStartToolsOnLaunch { enabled } => {
            update(app, controller, |p| p.start_tools_on_launch = enabled)
        }
        Request::SetHideWhileSharing { enabled } => {
            update(app, controller, |p| p.hide_while_sharing = enabled)
        }
        Request::SetSeenVersion { version } => {
            update(app, controller, |p| p.seen_version = version)
        }
        Request::SaveCaffeinateSettings { settings } => {
            json(locked("saveFailed")?.save_caffeinate_settings(settings)?)
        }
        Request::SetProxyRole { role } => update(app, controller, |p| p.proxy.role = role),
        Request::SetProxyPort { port } => update(app, controller, |p| p.proxy.port = port),
        Request::SetProxyServer { server } => update(app, controller, |p| p.proxy.server = server),
        Request::SetToolActive { tool, active } => {
            let mut tools = locked("workerUnavailable")?;
            tools.set_active(tool, active);
            json(tools.statuses())
        }
        Request::SetLoginLaunch { enabled } => {
            #[cfg(target_os = "macos")]
            return json(crate::login_launch::set_login_launch(app.clone(), enabled)?);
            #[cfg(not(target_os = "macos"))]
            {
                let _ = enabled;
                Err("unsupported")
            }
        }
        Request::SetLidMode { mode } => set_lid_mode(origin, controller, mode).await,
        Request::CheckProxyServer { address } => {
            let saved = locked("workerUnavailable")?
                .activity
                .preferences()
                .proxy
                .server;
            if !origin.may_check_server(&saved, &address) {
                return Err("notPermitted");
            }
            json(
                tauri::async_runtime::spawn_blocking(move || proxy::check_server(&address))
                    .await
                    .unwrap_or(false),
            )
        }
        Request::LaunchProxyApp { id } => {
            let settings = locked("proxyLaunchFailed")?.activity.preferences().proxy;
            json(
                tauri::async_runtime::spawn_blocking(move || proxy::launch(&settings, &id))
                    .await
                    .map_err(|_| "proxyLaunchFailed")??,
            )
        }
        Request::SaveProxyApps { apps } => update(app, controller, |p| p.proxy.apps = apps),
        Request::SaveProxyAllowed { allowed } => {
            update(app, controller, |p| p.proxy.allowed = allowed)
        }
        Request::RequestActivityAccessibility => json(
            tauri::async_runtime::spawn_blocking(activity::request_accessibility)
                .await
                .map_err(|_| "accessibilityRequestFailed")??,
        ),
    }
}

/// Saves the closed-lid choice. A choice that changes the system asks for the one-time permission first.
async fn set_lid_mode(
    origin: Origin,
    controller: &Controller,
    mode: LidMode,
) -> Result<Value, &'static str> {
    // A choice of Sleep still changes the system when the switch is on, by the user or by this app.
    let changes_system = {
        let tools = controller.lock().map_err(|_| "workerUnavailable")?;
        mode != LidMode::Sleep || tools.lid.status().external || tools.lid.managed()
    };
    if origin.lid_prompt(changes_system, lid::Lid::is_authorized())? {
        // The administrator prompt can stay open for a long time, so it runs without the lock.
        tauri::async_runtime::spawn_blocking(lid::Lid::authorize)
            .await
            .map_err(|_| "lidAuthorizationFailed")??;
    }
    let mut tools = controller.lock().map_err(|_| "workerUnavailable")?;
    let mut preferences = tools.activity.preferences();
    preferences.lid_awake = mode;
    tools.activity.save_preferences(preferences)?;
    tools.lid.adopt();
    if !tools.reconcile_lid() {
        return Err("lidChangeFailed");
    }
    json(tools.lid.status())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NONE: Origin = Origin::Remote(Permissions {
        control: false,
        actions: false,
    });
    const CONTROL: Origin = Origin::Remote(Permissions {
        control: true,
        actions: false,
    });
    const ACTIONS: Origin = Origin::Remote(Permissions {
        control: false,
        actions: true,
    });

    fn request(value: Value) -> Request {
        serde_json::from_value(value).unwrap()
    }

    fn own_command_preferences() -> Preferences {
        let mut preferences = Preferences::default();
        preferences.proxy.apps[0].command = Some("echo {server}".into());
        preferences
    }

    fn allowed(
        origin: Origin,
        request: &Request,
        preferences: &Preferences,
    ) -> Result<(), &'static str> {
        origin.permits(request.access(preferences))
    }

    #[test]
    fn a_remote_origin_without_permissions_is_refused_at_every_level() {
        let preferences = Preferences::default();
        let control = request(json!({"command": "getStatuses"}));
        let actions = request(json!({"command": "launchProxyApp", "args": {"id": "claude"}}));
        let local_only = request(json!({"command": "requestActivityAccessibility"}));
        let own = own_command_preferences();
        assert_eq!(allowed(NONE, &control, &preferences), Err("notPermitted"));
        assert_eq!(allowed(NONE, &actions, &preferences), Err("notPermitted"));
        let own_launch = request(json!({"command": "launchProxyApp", "args": {"id": "claude"}}));
        assert_eq!(allowed(NONE, &own_launch, &own), Err("notPermitted"));
        assert_eq!(allowed(NONE, &local_only, &preferences), Err("localOnly"));
        assert_eq!(allowed(Origin::Local, &local_only, &preferences), Ok(()));
    }

    #[test]
    fn remote_control_cannot_save_commands_address_lists_or_ask_for_accessibility() {
        let preferences = Preferences::default();
        for value in [
            json!({"command": "saveProxyApps", "args": {"apps": []}}),
            json!({"command": "saveProxyAllowed", "args": {"allowed": ["10.0.0.1"]}}),
            json!({"command": "requestActivityAccessibility"}),
        ] {
            assert_eq!(
                allowed(CONTROL, &request(value), &preferences),
                Err("localOnly")
            );
        }
    }

    #[test]
    fn a_proxy_client_with_its_own_command_needs_actions() {
        let launch = request(json!({"command": "launchProxyApp", "args": {"id": "claude"}}));
        let built_in = Preferences::default();
        assert_eq!(allowed(CONTROL, &launch, &built_in), Ok(()));
        let own = own_command_preferences();
        assert_eq!(allowed(CONTROL, &launch, &own), Err("notPermitted"));
        assert_eq!(allowed(ACTIONS, &launch, &own), Ok(()));
        // An id that does not exist falls to the launcher's own error.
        let unknown = request(json!({"command": "launchProxyApp", "args": {"id": "nope"}}));
        assert_eq!(unknown.access(&own), Access::Control);
    }

    #[test]
    fn actions_alone_does_not_grant_control() {
        let preferences = Preferences::default();
        let settings = request(json!({"command": "setTheme", "args": {"theme": "dark"}}));
        assert_eq!(
            allowed(ACTIONS, &settings, &preferences),
            Err("notPermitted")
        );
        assert_eq!(allowed(CONTROL, &settings, &preferences), Ok(()));
    }

    #[test]
    fn a_remote_origin_checks_only_the_saved_server() {
        assert!(CONTROL.may_check_server("host:8899", "host:8899"));
        assert!(!CONTROL.may_check_server("host:8899", "other:22"));
        assert!(!CONTROL.may_check_server("", "other:22"));
        assert!(Origin::Local.may_check_server("host:8899", "other:22"));
    }

    #[test]
    fn a_remote_lid_change_never_opens_the_administrator_prompt() {
        assert_eq!(
            CONTROL.lid_prompt(true, false),
            Err("lidNeedsLocalPermission")
        );
        assert_eq!(CONTROL.lid_prompt(true, true), Ok(false));
        assert_eq!(CONTROL.lid_prompt(false, false), Ok(false));
        assert_eq!(Origin::Local.lid_prompt(true, false), Ok(true));
        assert_eq!(Origin::Local.lid_prompt(false, false), Ok(false));
    }

    #[test]
    fn only_the_main_window_is_local() {
        assert!(matches!(Origin::of_window("main"), Ok(Origin::Local)));
        for label in ["remote-x", "", "Main", "main2"] {
            assert!(Origin::of_window(label).is_err());
        }
    }

    #[test]
    fn requests_use_the_shape_the_webview_sends() {
        assert!(matches!(
            request(json!({"command": "getPreferences"})),
            Request::GetPreferences
        ));
        assert!(matches!(
            request(json!({"command": "getPreferences", "args": null})),
            Request::GetPreferences
        ));
        assert!(matches!(
            request(
                json!({"command": "setQuickStart", "args": {"tool": "proxy", "include": true}})
            ),
            Request::SetQuickStart {
                tool: ToolId::Proxy,
                include: true
            }
        ));
        assert!(matches!(
            request(json!({"command": "setSeenVersion", "args": {"version": null}})),
            Request::SetSeenVersion { version: None }
        ));
        assert!(serde_json::from_value::<Request>(
            json!({"command": "savePreferences", "args": {}})
        )
        .is_err());
    }
}
