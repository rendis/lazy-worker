//! The one command the webview can call, and the barriers in front of every operation.
//!
//! Every request declares its access in `Request::access`. The origin of a request comes from
//! the label of the window that sent it, never from data in the request.

use crate::{
    activity, apps, caffeinate,
    caffeinate::CaffeinateSettings,
    devices, lid,
    lid::LidMode,
    preferences::{Language, ManualDuration, Preferences, Schedule, Theme},
    proxy,
    proxy::{ProxyApp, Role},
    Controller, FirstRun, ToolId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, WebviewWindow};

/// What a request needs from its origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Access {
    /// Reads and settings that cannot run a command or change who may connect.
    Control,
    /// Starts a command the user wrote.
    Actions,
    /// Lists, draws, and opens the applications of this device.
    Applications,
    /// Settings that store a command, an address list, or a permission, and system prompts.
    LocalOnly,
    /// Needs only that the caller is a paired device. It reads nothing that a device did not already
    /// agree to share, and it changes nothing.
    Paired,
}

/// The version of the request set that two devices speak. A window that controls a device
/// whose number differs offers to update it.
pub const PROTOCOL: u32 = 1;

/// What a remote device may do. All off by default.
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(not(unix), allow(dead_code))]
pub struct Permissions {
    pub control: bool,
    pub applications: bool,
    pub actions: bool,
    /// The device that sent the request. A request that belongs to one device, such as a run, uses it.
    pub peer: Option<PeerId>,
}

/// The id of a paired device: 32 lowercase hexadecimal characters, kept by value so an origin stays `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
pub struct PeerId([u8; 32]);

#[cfg_attr(not(unix), allow(dead_code))]
impl PeerId {
    pub fn parse(text: &str) -> Option<Self> {
        let bytes: [u8; 32] = text.as_bytes().try_into().ok()?;
        bytes
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            .then_some(Self(bytes))
    }

    pub fn as_str(&self) -> &str {
        // The bytes were checked to be ASCII hexadecimal.
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

/// Where a request comes from.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(unix), allow(dead_code))] // Only the socket server builds a remote origin.
pub enum Origin {
    Local,
    Remote(Permissions),
}

impl Origin {
    /// The id of the device that sent the request, when a device did.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn peer(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Remote(permissions) => permissions.peer.as_ref().map(PeerId::as_str),
        }
    }

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
            // The server looked the peer up before it came here, so any remote origin is paired.
            Access::Paired => Ok(()),
            Access::Control if permissions.control => Ok(()),
            Access::Applications if permissions.applications => Ok(()),
            Access::Actions if permissions.actions => Ok(()),
            Access::LocalOnly => Err("localOnly"),
            Access::Control | Access::Applications | Access::Actions => Err("notPermitted"),
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
    /// What this app shows its viewer in: the language and the theme.
    GetViewer,
    /// Who answers, what the caller may do here, and the version.
    GetSession,
    GetStatuses,
    GetLidStatus,
    GetLoginLaunch,
    GetUpdateStatus,
    GetActivityAccessibility,
    GetProxyNetwork,
    GetProxyPeers,
    GetProxyLog,
    GetDevices,
    GetRemoteLog,
    DiscoverDevices,
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
    SetPinnedTools { tools: Vec<ToolId> },
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
    SetPeerPermissions {
        id: String,
        control: bool,
        applications: bool,
        actions: bool,
    },
    RemovePeer { id: String },
    OpenPairing { control: bool, applications: bool, actions: bool },
    ClosePairing,
    PairDevice { user: String, host: String, port: Option<u16>, code: String },
    ForgetDevice { id: String },
    CutAllAccess,
    AllowPasswordLogin,
    EnableRemoteAccess,
    OpenSharingSettings,
    /// The installed applications of this device, and which are open.
    ListApplications,
    /// The icon of an application of that list, as base64 text of one PNG.
    GetAppIcon { bundle_id: String },
    /// Opens an application of that list.
    OpenApplication { bundle_id: String },
    /// The actions of this device. A remote origin gets them without their commands.
    ListActions,
    /// Writing an action is for the person at this device only.
    CreateAction { name: String, command: String, timeout_seconds: Option<u32> },
    UpdateAction { id: String, name: String, command: String, timeout_seconds: Option<u32> },
    DeleteAction { id: String },
    /// Starts an action and returns the id of the run.
    RunAction { id: String },
    GetActionRun { run_id: String },
    StopActionRun { run_id: String },
    /// `schedule` is `{"kind": "once", "at"}`, `{"kind": "daily", "time"}` or `{"kind": "weekly", "day", "time"}`.
    ScheduleAction { id: String, schedule: Value, notify_on_failure: bool },
    ClearSchedule { id: String },
    /// The path of the cached icon of an application on a device that this one controls. It fetches the icon first when it is not cached.
    GetCachedIcon { id: String, bundle_id: String, version: String },
    /// Saves the applications that the Devices view shows first for a device that this one controls.
    SetPinnedApps { id: String, bundle_ids: Vec<String> },
    /// Opens the window that controls a paired device.
    OpenRemoteWindow { id: String },
    /// Grants a full shell to a device that controls this one, for `minutes` (1 to 1440) or until it is turned off.
    /// Only the person at this device may grant it, and pairing never does.
    GrantShell { id: String, minutes: Option<u32> },
    RevokeShell { id: String },
    /// Hides a note that `~/.ssh/authorized_keys` changed during a shell.
    DismissKeysWarning { id: u64 },
    /// Opens a shell on a device that this one controls, in Terminal. It exists only between the main window and this device.
    OpenShell { id: String },
    /// Sends one of a few requests to a paired device, for the Devices view.
    RemoteCall { id: String, request: Value },
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
            | Self::DetectProxyApps
            | Self::ListCaffeinateProcesses
            | Self::CheckForUpdate
            | Self::RestartToUpdate
            | Self::SetLanguage { .. }
            | Self::SetTheme { .. }
            | Self::SetDuration { .. }
            | Self::SetSchedule { .. }
            | Self::SetHideWhileSharing { .. }
            | Self::SetSeenVersion { .. }
            | Self::SetPinnedTools { .. }
            | Self::SaveCaffeinateSettings { .. }
            | Self::SetLoginLaunch { .. }
            | Self::SetLidMode { .. }
            | Self::CheckProxyServer { .. } => Access::Control,
            // Turning it on arms the next launch to open the proxy clients, which run commands. Only the person here does that.
            Self::SetStartToolsOnLaunch { enabled: true } => Access::LocalOnly,
            Self::SetStartToolsOnLaunch { enabled: false } => Access::Control,
            // Including the proxy arms Quick start and the next launch to open a LAN listener. Only the person here does that.
            Self::SetQuickStart {
                tool: ToolId::Proxy,
                include: true,
            } => Access::LocalOnly,
            Self::SetQuickStart { .. } => Access::Control,
            Self::GetSession => Access::Paired,
            Self::ListActions
            | Self::RunAction { .. }
            | Self::GetActionRun { .. }
            | Self::StopActionRun { .. }
            | Self::ScheduleAction { .. }
            | Self::ClearSchedule { .. } => Access::Actions,
            // An action holds a command. Only the device that holds it writes it.
            Self::CreateAction { .. } | Self::UpdateAction { .. } | Self::DeleteAction { .. } => {
                Access::LocalOnly
            }
            Self::ListApplications | Self::GetAppIcon { .. } | Self::OpenApplication { .. } => {
                Access::Applications
            }
            // Starting the proxy server opens a port on this Mac, so only the person here may do it.
            // Stopping it, and every other tool, stays remote control.
            Self::SetToolActive { tool, active } => match (tool, active) {
                (ToolId::Proxy, true) => Access::LocalOnly,
                (ToolId::Proxy, false) | (ToolId::Activity | ToolId::Caffeinate, _) => {
                    Access::Control
                }
            },
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
            Self::GetDevices
            | Self::GetRemoteLog
            | Self::DiscoverDevices
            | Self::SetPeerPermissions { .. }
            | Self::RemovePeer { .. }
            | Self::OpenPairing { .. }
            | Self::ClosePairing
            | Self::PairDevice { .. }
            | Self::ForgetDevice { .. }
            | Self::CutAllAccess
            | Self::AllowPasswordLogin
            | Self::EnableRemoteAccess
            | Self::OpenSharingSettings
            | Self::SaveProxyApps { .. }
            | Self::SaveProxyAllowed { .. }
            | Self::SetProxyRole { .. }
            | Self::SetProxyPort { .. }
            | Self::SetProxyServer { .. }
            | Self::GetProxyPeers
            | Self::GetProxyLog
            | Self::GetViewer
            | Self::GetCachedIcon { .. }
            | Self::SetPinnedApps { .. }
            | Self::OpenRemoteWindow { .. }
            | Self::RemoteCall { .. }
            // A shell can run any command as the user. Only the person at this device turns it on.
            | Self::GrantShell { .. }
            | Self::RevokeShell { .. }
            | Self::DismissKeysWarning { .. }
            | Self::OpenShell { .. }
            | Self::RequestActivityAccessibility => Access::LocalOnly,
        }
    }
}

impl Request {
    /// True for a request that only reads state. A remote read that works is not logged.
    /// There is no default arm: a new request must say whether it reads.
    pub fn is_read(&self) -> bool {
        match self {
            Self::GetPreferences
            | Self::GetFirstRun
            | Self::GetSystemLanguage
            | Self::GetViewer
            | Self::GetSession
            | Self::GetStatuses
            | Self::GetLidStatus
            | Self::GetLoginLaunch
            | Self::GetUpdateStatus
            | Self::GetActivityAccessibility
            | Self::GetProxyNetwork
            | Self::GetProxyPeers
            | Self::GetProxyLog
            | Self::GetDevices
            | Self::GetRemoteLog
            | Self::DiscoverDevices
            | Self::DetectProxyApps
            | Self::ListCaffeinateProcesses
            | Self::ListApplications
            | Self::GetAppIcon { .. }
            | Self::GetCachedIcon { .. }
            | Self::ListActions
            | Self::GetActionRun { .. } => true,
            Self::CheckForUpdate
            | Self::RestartToUpdate
            | Self::SetLanguage { .. }
            | Self::SetTheme { .. }
            | Self::SetDuration { .. }
            | Self::SetSchedule { .. }
            | Self::SetQuickStart { .. }
            | Self::SetStartToolsOnLaunch { .. }
            | Self::SetHideWhileSharing { .. }
            | Self::SetSeenVersion { .. }
            | Self::SetPinnedTools { .. }
            | Self::SaveCaffeinateSettings { .. }
            | Self::SetProxyRole { .. }
            | Self::SetProxyPort { .. }
            | Self::SetProxyServer { .. }
            | Self::SetToolActive { .. }
            | Self::SetLoginLaunch { .. }
            | Self::SetLidMode { .. }
            | Self::CheckProxyServer { .. }
            | Self::LaunchProxyApp { .. }
            | Self::SaveProxyApps { .. }
            | Self::SaveProxyAllowed { .. }
            | Self::RequestActivityAccessibility
            | Self::SetPeerPermissions { .. }
            | Self::RemovePeer { .. }
            | Self::OpenPairing { .. }
            | Self::ClosePairing
            | Self::PairDevice { .. }
            | Self::ForgetDevice { .. }
            | Self::CutAllAccess
            | Self::AllowPasswordLogin
            | Self::EnableRemoteAccess
            | Self::OpenSharingSettings
            | Self::OpenApplication { .. }
            | Self::CreateAction { .. }
            | Self::UpdateAction { .. }
            | Self::DeleteAction { .. }
            | Self::RunAction { .. }
            | Self::StopActionRun { .. }
            | Self::ScheduleAction { .. }
            | Self::ClearSchedule { .. }
            | Self::SetPinnedApps { .. }
            | Self::OpenRemoteWindow { .. }
            | Self::GrantShell { .. }
            | Self::RevokeShell { .. }
            | Self::DismissKeysWarning { .. }
            | Self::OpenShell { .. }
            | Self::RemoteCall { .. } => false,
        }
    }

    /// True for a request that is about the person at this window, not about a device. It always
    /// runs on this device, also from a window that controls another one. There is no default
    /// arm: a new request must say where it runs.
    pub fn runs_on_viewer(&self) -> bool {
        match self {
            Self::GetSystemLanguage | Self::GetViewer => true,
            Self::GetPreferences
            | Self::GetFirstRun
            | Self::GetSession
            | Self::GetStatuses
            | Self::GetLidStatus
            | Self::GetLoginLaunch
            | Self::GetUpdateStatus
            | Self::GetActivityAccessibility
            | Self::GetProxyNetwork
            | Self::GetProxyPeers
            | Self::GetProxyLog
            | Self::GetDevices
            | Self::GetRemoteLog
            | Self::DiscoverDevices
            | Self::DetectProxyApps
            | Self::ListCaffeinateProcesses
            | Self::ListApplications
            | Self::GetAppIcon { .. }
            | Self::GetCachedIcon { .. }
            | Self::ListActions
            | Self::GetActionRun { .. }
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
            | Self::SetPinnedTools { .. }
            | Self::SaveCaffeinateSettings { .. }
            | Self::SetProxyRole { .. }
            | Self::SetProxyPort { .. }
            | Self::SetProxyServer { .. }
            | Self::SetToolActive { .. }
            | Self::SetLoginLaunch { .. }
            | Self::SetLidMode { .. }
            | Self::CheckProxyServer { .. }
            | Self::LaunchProxyApp { .. }
            | Self::SaveProxyApps { .. }
            | Self::SaveProxyAllowed { .. }
            | Self::RequestActivityAccessibility
            | Self::SetPeerPermissions { .. }
            | Self::RemovePeer { .. }
            | Self::OpenPairing { .. }
            | Self::ClosePairing
            | Self::PairDevice { .. }
            | Self::ForgetDevice { .. }
            | Self::CutAllAccess
            | Self::AllowPasswordLogin
            | Self::EnableRemoteAccess
            | Self::OpenSharingSettings
            | Self::OpenApplication { .. }
            | Self::CreateAction { .. }
            | Self::UpdateAction { .. }
            | Self::DeleteAction { .. }
            | Self::RunAction { .. }
            | Self::StopActionRun { .. }
            | Self::ScheduleAction { .. }
            | Self::ClearSchedule { .. }
            | Self::SetPinnedApps { .. }
            | Self::OpenRemoteWindow { .. }
            | Self::GrantShell { .. }
            | Self::RevokeShell { .. }
            | Self::DismissKeysWarning { .. }
            | Self::OpenShell { .. }
            | Self::RemoteCall { .. } => false,
        }
    }

    /// True for a request that exists only between the main window and a device that it controls.
    /// A window that controls a device never sends one.
    fn is_controller_side(&self) -> bool {
        matches!(
            self,
            Self::OpenRemoteWindow { .. }
                | Self::RemoteCall { .. }
                | Self::GetCachedIcon { .. }
                | Self::SetPinnedApps { .. }
                | Self::OpenShell { .. }
        )
    }

    /// The requests that the Devices view may send to a device through `RemoteCall`.
    fn may_be_called_remotely(&self) -> bool {
        matches!(
            self,
            Self::GetStatuses
                | Self::SetToolActive { .. }
                | Self::GetSession
                | Self::ListApplications
                | Self::GetAppIcon { .. }
                | Self::OpenApplication { .. }
                | Self::ListActions
                | Self::RunAction { .. }
                | Self::GetActionRun { .. }
                | Self::StopActionRun { .. }
                | Self::ScheduleAction { .. }
                | Self::ClearSchedule { .. }
        )
    }
}

/// Where the window that sent a request is: this device's own panel, or a window that controls a device.
#[derive(Debug, PartialEq)]
enum Route {
    Local,
    Device(String),
}

impl Route {
    /// Decides by the label of the window, and by nothing that the request holds. A device label
    /// is `remote-` and a device id, and the id must be one that this device controls.
    fn of_window(label: &str, controls: impl Fn(&str) -> bool) -> Result<Self, &'static str> {
        if Origin::of_window(label).is_ok() {
            return Ok(Self::Local);
        }
        match label.strip_prefix("remote-") {
            Some(id) if is_device_id(id) && controls(id) => Ok(Self::Device(id.to_owned())),
            _ => Err("notPermitted"),
        }
    }
}

/// 32 lowercase hexadecimal characters.
fn is_device_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// What happens to a request from a window.
#[derive(Debug, PartialEq)]
enum Plan {
    /// It runs on this device, with the rights of the person here.
    Here,
    /// It goes to the device with this id, which checks it as for any paired device.
    Forward(String),
    Refuse,
}

fn plan(route: &Route, request: &Request) -> Plan {
    match route {
        Route::Local => Plan::Here,
        Route::Device(_) if request.is_controller_side() => Plan::Refuse,
        Route::Device(_) if request.runs_on_viewer() => Plan::Here,
        Route::Device(id) => Plan::Forward(id.clone()),
    }
}

#[tauri::command]
pub async fn dispatch(window: WebviewWindow, request: Value) -> Result<Value, &'static str> {
    let app = window.app_handle();
    let route = Route::of_window(window.label(), |id| controls(app, id))?;
    let parsed: Request =
        serde_json::from_value(request.clone()).map_err(|_| "badRequest")?;
    match plan(&route, &parsed) {
        Plan::Here => handle(app, Origin::Local, parsed).await,
        Plan::Forward(id) => forward(app, &id, request).await,
        Plan::Refuse => Err("notPermitted"),
    }
}

/// True when this device controls the device with this id.
fn controls(app: &AppHandle, id: &str) -> bool {
    #[cfg(unix)]
    return app
        .try_state::<crate::remote::service::Service>()
        .is_some_and(|service| service.device(id).is_some());
    #[cfg(not(unix))]
    {
        let _ = (app, id);
        false
    }
}

/// Sends a request to a device that this one controls and returns its answer as it came.
async fn forward(app: &AppHandle, id: &str, request: Value) -> Result<Value, &'static str> {
    #[cfg(unix)]
    {
        let (app, id) = (app.clone(), id.to_owned());
        return tauri::async_runtime::spawn_blocking(move || {
            app.state::<crate::remote::service::Service>()
                .forward(&id, &request)
        })
        .await
        .map_err(|_| "workerUnavailable")?;
    }
    #[cfg(not(unix))]
    {
        let _ = (app, id, request);
        Err("unsupported")
    }
}

/// Checks that the origin may send the request.
pub fn authorize(
    origin: Origin,
    request: &Request,
    preferences: &Preferences,
) -> Result<(), &'static str> {
    origin.permits(request.access(preferences))
}

/// Runs one request for an origin. The webview and every remote device share this path.
pub async fn handle(
    app: &AppHandle,
    origin: Origin,
    request: Request,
) -> Result<Value, &'static str> {
    let controller = app.state::<Controller>();
    let preferences = controller
        .lock()
        .map_err(|_| "workerUnavailable")?
        .activity
        .preferences();
    authorize(origin, &request, &preferences)?;
    run(request, origin, app, &controller).await
}

/// The preferences as one origin may read them. Every proxy app says whether it has its own command
/// (`custom`). A remote origin never gets the command text, because it can hold a secret.
fn preferences_view(preferences: &Preferences, origin: Origin) -> Result<Value, &'static str> {
    let mut value = json(preferences)?;
    let apps = value
        .pointer_mut("/proxy/apps")
        .and_then(Value::as_array_mut)
        .ok_or("workerUnavailable")?;
    for app in apps {
        let Some(app) = app.as_object_mut() else {
            continue;
        };
        let custom = app.get("command").is_some_and(|command| !command.is_null());
        app.insert("custom".into(), Value::Bool(custom));
        if matches!(origin, Origin::Remote(_)) {
            app.insert("command".into(), Value::Null);
        }
    }
    Ok(value)
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
        Request::GetPreferences => {
            let preferences = locked("workerUnavailable")?.activity.preferences();
            preferences_view(&preferences, origin)
        }
        Request::GetFirstRun => json(app.state::<FirstRun>().0),
        Request::GetSystemLanguage => {
            #[cfg(target_os = "macos")]
            return json(Some(crate::preferences::system_language()));
            #[cfg(not(target_os = "macos"))]
            json(None::<Language>)
        }
        Request::GetViewer => {
            let preferences = locked("workerUnavailable")?.activity.preferences();
            json(serde_json::json!({
                "language": preferences.language,
                "theme": preferences.theme,
            }))
        }
        Request::GetSession => {
            #[cfg(unix)]
            return session(app, origin).await;
            #[cfg(not(unix))]
            Err("unsupported")
        }
        Request::OpenRemoteWindow { id } => {
            #[cfg(unix)]
            return crate::remote::window::open(app, &id).map(|()| Value::Null);
            #[cfg(not(unix))]
            {
                let _ = id;
                Err("unsupported")
            }
        }
        Request::GrantShell { id, minutes } => {
            #[cfg(unix)]
            return json(app.state::<crate::remote::service::Service>().grant_shell(&id, minutes)?);
            #[cfg(not(unix))]
            {
                let _ = (id, minutes);
                Err("unsupported")
            }
        }
        Request::RevokeShell { id } => {
            #[cfg(unix)]
            return tauri::async_runtime::spawn_blocking({
                let app = app.clone();
                move || app.state::<crate::remote::service::Service>().revoke_shell(&id)
            })
            .await
            .map_err(|_| "workerUnavailable")?
            .map(|()| Value::Null);
            #[cfg(not(unix))]
            {
                let _ = id;
                Err("unsupported")
            }
        }
        Request::DismissKeysWarning { id } => {
            #[cfg(unix)]
            return app
                .state::<crate::remote::service::Service>()
                .dismiss_warning(id)
                .map(|()| Value::Null);
            #[cfg(not(unix))]
            {
                let _ = id;
                Err("unsupported")
            }
        }
        Request::OpenShell { id } => {
            #[cfg(unix)]
            return tauri::async_runtime::spawn_blocking({
                let app = app.clone();
                move || app.state::<crate::remote::service::Service>().open_shell(&id)
            })
            .await
            .map_err(|_| "terminalFailed")?
            .map(|()| Value::Null);
            #[cfg(not(unix))]
            {
                let _ = id;
                Err("unsupported")
            }
        }
        Request::RemoteCall { id, request } => {
            let parsed: Request =
                serde_json::from_value(request.clone()).map_err(|_| "badRequest")?;
            if !parsed.may_be_called_remotely() {
                return Err("notPermitted");
            }
            forward(app, &id, request).await
        }
        Request::ListApplications => json(
            tauri::async_runtime::spawn_blocking(apps::list)
                .await
                .map_err(|_| "workerUnavailable")??,
        ),
        Request::GetAppIcon { bundle_id } => json(serde_json::json!({
            "png": tauri::async_runtime::spawn_blocking(move || apps::icon_base64(&bundle_id))
                .await
                .map_err(|_| "appIconFailed")??,
        })),
        Request::OpenApplication { bundle_id } => {
            tauri::async_runtime::spawn_blocking(move || apps::open(&bundle_id))
                .await
                .map_err(|_| "appOpenFailed")?
                .map(|()| Value::Null)
        }
        request @ (Request::ListActions
        | Request::CreateAction { .. }
        | Request::UpdateAction { .. }
        | Request::DeleteAction { .. }
        | Request::RunAction { .. }
        | Request::GetActionRun { .. }
        | Request::StopActionRun { .. }
        | Request::ScheduleAction { .. }
        | Request::ClearSchedule { .. }) => {
            #[cfg(unix)]
            return crate::actions::handle(app, origin, request);
            #[cfg(not(unix))]
            {
                let _ = request;
                Err("unsupported")
            }
        }
        Request::GetCachedIcon { id, bundle_id, version } => {
            #[cfg(unix)]
            {
                let app = app.clone();
                let path = tauri::async_runtime::spawn_blocking(move || {
                    app.state::<crate::remote::service::Service>()
                        .cached_icon(&id, &bundle_id, &version)
                })
                .await
                .map_err(|_| "workerUnavailable")??;
                json(serde_json::json!({ "path": path }))
            }
            #[cfg(not(unix))]
            {
                let _ = (id, bundle_id, version);
                Err("unsupported")
            }
        }
        Request::SetPinnedApps { id, bundle_ids } => {
            #[cfg(unix)]
            return app
                .state::<crate::remote::service::Service>()
                .set_pinned(&id, &bundle_ids)
                .map(|()| Value::Null);
            #[cfg(not(unix))]
            {
                let _ = (id, bundle_ids);
                Err("unsupported")
            }
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
        Request::GetDevices => {
            #[cfg(unix)]
            let stores = {
                let service = app.state::<crate::remote::service::Service>();
                devices::Stores {
                    controllers: service.controllers(),
                    socket: service.socket(),
                    pairing: service.pairing_state(),
                    devices: service.remote_devices(),
                    warnings: service.warnings(),
                }
            };
            #[cfg(not(unix))]
            let stores = devices::Stores {
                controllers: Vec::new(),
                socket: devices::Socket::unavailable("unsupported"),
                pairing: devices::Pairing::closed(),
                devices: Vec::new(),
                warnings: Vec::new(),
            };
            json(
                tauri::async_runtime::spawn_blocking(move || {
                    #[cfg(unix)]
                    let access = crate::remote::access::status();
                    #[cfg(not(unix))]
                    let access = devices::RemoteAccess::unavailable();
                    devices::overview(stores, access)
                })
                .await
                .map_err(|_| "workerUnavailable")?,
            )
        }
        Request::AllowPasswordLogin => {
            #[cfg(unix)]
            return json(
                tauri::async_runtime::spawn_blocking(crate::remote::access::allow_password_login)
                    .await
                    .map_err(|_| "remoteAccessChangeFailed")??,
            );
            #[cfg(not(unix))]
            Err("unsupported")
        }
        Request::EnableRemoteAccess => {
            #[cfg(unix)]
            return json(
                tauri::async_runtime::spawn_blocking(crate::remote::access::enable_remote_access)
                    .await
                    .map_err(|_| "remoteAccessChangeFailed")??,
            );
            #[cfg(not(unix))]
            Err("unsupported")
        }
        Request::OpenSharingSettings => {
            #[cfg(unix)]
            return tauri::async_runtime::spawn_blocking(crate::remote::access::open_sharing_settings)
                .await
                .map_err(|_| "sharingSettingsFailed")?
                .map(|()| Value::Null);
            #[cfg(not(unix))]
            Err("unsupported")
        }
        Request::DiscoverDevices => {
            #[cfg(unix)]
            return json(
                tauri::async_runtime::spawn_blocking(crate::remote::discovery::discover)
                    .await
                    .map_err(|_| "workerUnavailable")??,
            );
            #[cfg(not(unix))]
            Err("unsupported")
        }
        Request::GetRemoteLog => {
            #[cfg(unix)]
            return json(app.state::<crate::remote::service::Service>().log()?);
            #[cfg(not(unix))]
            Err("unsupported")
        }
        Request::SetPeerPermissions { id, control, applications, actions } => {
            #[cfg(unix)]
            return app
                .state::<crate::remote::service::Service>()
                .set_permissions(&id, control, applications, actions)
                .map(|()| Value::Null);
            #[cfg(not(unix))]
            {
                let _ = (id, control, applications, actions);
                Err("unsupported")
            }
        }
        Request::RemovePeer { id } => {
            #[cfg(unix)]
            return tauri::async_runtime::spawn_blocking({
                let app = app.clone();
                move || {
                    let removed = app.state::<crate::remote::service::Service>().remove_peer(&id);
                    // The schedules of a device go with its access, even when its line could not be removed.
                    crate::actions::forget_peer(&app, Some(&id));
                    removed
                }
            })
            .await
            .map_err(|_| "workerUnavailable")?
            .map(|()| Value::Null);
            #[cfg(not(unix))]
            {
                let _ = id;
                Err("unsupported")
            }
        }
        Request::OpenPairing { control, applications, actions } => {
            #[cfg(unix)]
            return json(
                tauri::async_runtime::spawn_blocking({
                    let app = app.clone();
                    move || {
                        app.state::<crate::remote::service::Service>()
                            .open_pairing(control, applications, actions)
                    }
                })
                .await
                .map_err(|_| "workerUnavailable")??,
            );
            #[cfg(not(unix))]
            {
                let _ = (control, applications, actions);
                Err("unsupported")
            }
        }
        Request::ClosePairing => {
            #[cfg(unix)]
            return tauri::async_runtime::spawn_blocking({
                let app = app.clone();
                move || app.state::<crate::remote::service::Service>().close_pairing()
            })
            .await
            .map_err(|_| "workerUnavailable")?
            .map(|()| Value::Null);
            #[cfg(not(unix))]
            Err("unsupported")
        }
        Request::PairDevice { user, host, port, code } => {
            #[cfg(unix)]
            return json(
                tauri::async_runtime::spawn_blocking({
                    let app = app.clone();
                    move || {
                        app.state::<crate::remote::service::Service>()
                            .pair_device(&user, &host, port, &code)
                    }
                })
                .await
                .map_err(|_| "workerUnavailable")??,
            );
            #[cfg(not(unix))]
            {
                let _ = (user, host, port, code);
                Err("unsupported")
            }
        }
        Request::ForgetDevice { id } => {
            #[cfg(unix)]
            return json(
                tauri::async_runtime::spawn_blocking({
                    let app = app.clone();
                    move || {
                        let forgotten = app
                            .state::<crate::remote::service::Service>()
                            .forget_device(&id);
                        // The window of a device that is forgotten has nothing left to talk to.
                        crate::remote::window::close(&app, &id);
                        forgotten
                    }
                })
                .await
                .map_err(|_| "workerUnavailable")??,
            );
            #[cfg(not(unix))]
            {
                let _ = id;
                Err("unsupported")
            }
        }
        Request::CutAllAccess => {
            #[cfg(unix)]
            return tauri::async_runtime::spawn_blocking({
                let app = app.clone();
                move || {
                    let cut = app.state::<crate::remote::service::Service>().cut_all();
                    crate::actions::forget_peer(&app, None);
                    cut
                }
            })
            .await
            .map_err(|_| "workerUnavailable")?
            .map(|()| Value::Null);
            #[cfg(not(unix))]
            Err("unsupported")
        }
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
        Request::SetPinnedTools { tools } => {
            // Known tools only (the type), each once, and no more than there are tools.
            if tools.len() > 3 || tools.iter().enumerate().any(|(at, tool)| tools[..at].contains(tool)) {
                return Err("saveFailed");
            }
            let names = tools
                .iter()
                .map(|tool| match tool {
                    ToolId::Activity => "activity",
                    ToolId::Caffeinate => "caffeinate",
                    ToolId::Proxy => "proxy",
                })
                .map(String::from)
                .collect();
            update(app, controller, |p| p.pinned = names)
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

/// Who answers, what the caller may do here, and the version. It runs commands that name this
/// device, so it stays off the main thread.
#[cfg(unix)]
async fn session(app: &AppHandle, origin: Origin) -> Result<Value, &'static str> {
    let permissions = match origin {
        Origin::Local => Permissions {
            control: true,
            applications: true,
            actions: true,
            peer: None,
        },
        Origin::Remote(permissions) => permissions,
    };
    // The shell of the caller, as the device that answers granted it. This device has no shell on itself.
    let shell = origin
        .peer()
        .and_then(|peer| app.state::<crate::remote::service::Service>().shell_of(peer));
    let (own, version) = (
        app.state::<crate::remote::service::Service>().own_device(),
        app.package_info().version.to_string(),
    );
    let own = tauri::async_runtime::spawn_blocking(move || own())
        .await
        .map_err(|_| "workerUnavailable")??;
    json(serde_json::json!({
        "device": { "id": own.id.as_str(), "name": own.name, "kind": own.kind },
        "permissions": {
            "control": permissions.control,
            "applications": permissions.applications,
            "actions": permissions.actions,
        },
        "shell": shell,
        "version": version,
        "protocol": PROTOCOL,
    }))
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
        applications: false,
        actions: false,
        peer: None,
    });
    const CONTROL: Origin = Origin::Remote(Permissions {
        control: true,
        applications: false,
        actions: false,
        peer: None,
    });
    const APPLICATIONS: Origin = Origin::Remote(Permissions {
        control: false,
        applications: true,
        actions: false,
        peer: None,
    });
    const ACTIONS: Origin = Origin::Remote(Permissions {
        control: false,
        applications: false,
        actions: true,
        peer: None,
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
    fn device_management_is_local_only() {
        let preferences = Preferences::default();
        let devices = request(json!({"command": "getDevices"}));
        assert_eq!(allowed(CONTROL, &devices, &preferences), Err("localOnly"));
        assert_eq!(allowed(ACTIONS, &devices, &preferences), Err("localOnly"));
        assert_eq!(allowed(Origin::Local, &devices, &preferences), Ok(()));
    }

    #[test]
    fn every_device_management_request_is_local_only() {
        let preferences = Preferences::default();
        let id = "0123456789abcdef0123456789abcdef";
        for value in [
            json!({"command": "getDevices"}),
            json!({"command": "getRemoteLog"}),
            json!({"command": "setPeerPermissions", "args": {"id": id, "control": true, "applications": true, "actions": true}}),
            json!({"command": "removePeer", "args": {"id": id}}),
            json!({"command": "openPairing", "args": {"control": true, "applications": true, "actions": true}}),
            json!({"command": "closePairing"}),
            json!({"command": "pairDevice", "args": {"user": "me", "host": "studio.local", "port": 22, "code": "123456"}}),
            json!({"command": "pairDevice", "args": {"user": "me", "host": "studio.local", "code": "123456"}}),
            json!({"command": "forgetDevice", "args": {"id": id}}),
            json!({"command": "cutAllAccess"}),
            json!({"command": "allowPasswordLogin"}),
            json!({"command": "enableRemoteAccess"}),
            json!({"command": "openSharingSettings"}),
        ] {
            let parsed = request(value);
            for origin in [NONE, CONTROL, ACTIONS, Origin::Remote(Permissions { control: true, applications: true, actions: true, peer: None })] {
                assert_eq!(allowed(origin, &parsed, &preferences), Err("localOnly"));
            }
            assert_eq!(allowed(Origin::Local, &parsed, &preferences), Ok(()));
        }
    }

    #[test]
    fn a_remote_origin_cannot_start_the_proxy_or_change_where_it_listens() {
        let preferences = Preferences::default();
        for value in [
            json!({"command": "setToolActive", "args": {"tool": "proxy", "active": true}}),
            json!({"command": "setProxyRole", "args": {"role": "server"}}),
            json!({"command": "setProxyPort", "args": {"port": 9000}}),
            json!({"command": "setProxyServer", "args": {"server": "10.0.0.2:8899"}}),
            json!({"command": "getProxyPeers"}),
            json!({"command": "getProxyLog"}),
        ] {
            let parsed = request(value);
            let both = Origin::Remote(Permissions { control: true, applications: true, actions: true, peer: None });
            assert_eq!(allowed(CONTROL, &parsed, &preferences), Err("localOnly"));
            assert_eq!(allowed(both, &parsed, &preferences), Err("localOnly"));
            assert_eq!(allowed(Origin::Local, &parsed, &preferences), Ok(()));
        }
    }

    #[test]
    fn a_remote_origin_can_stop_the_proxy_and_control_other_tools() {
        let preferences = Preferences::default();
        for value in [
            json!({"command": "setToolActive", "args": {"tool": "proxy", "active": false}}),
            json!({"command": "setToolActive", "args": {"tool": "activity", "active": true}}),
            json!({"command": "setToolActive", "args": {"tool": "caffeinate", "active": true}}),
        ] {
            let parsed = request(value);
            assert_eq!(allowed(CONTROL, &parsed, &preferences), Ok(()));
            assert_eq!(allowed(NONE, &parsed, &preferences), Err("notPermitted"));
        }
    }

    #[test]
    fn a_remote_read_hides_stored_commands_and_says_which_apps_have_one() {
        let secret = "curl -H 'Authorization: sk-secret-123' {server}";
        let mut preferences = Preferences::default();
        preferences.proxy.apps[0].command = Some(secret.into());
        let remote = preferences_view(&preferences, CONTROL).unwrap();
        assert!(!remote.to_string().contains("sk-secret-123"));
        let apps = remote["proxy"]["apps"].as_array().unwrap();
        assert_eq!(apps[0]["command"], Value::Null);
        assert_eq!(apps[0]["custom"], json!(true));
        assert_eq!(apps[1]["custom"], json!(false));
        let local = preferences_view(&preferences, Origin::Local).unwrap();
        assert_eq!(local["proxy"]["apps"][0]["command"], json!(secret));
        assert_eq!(local["proxy"]["apps"][0]["custom"], json!(true));
        assert_eq!(local["proxy"]["apps"][1]["custom"], json!(false));
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
        assert!(matches!(
            request(json!({"command": "setPinnedTools", "args": {"tools": ["proxy", "activity"]}})),
            Request::SetPinnedTools { ref tools } if tools == &[ToolId::Proxy, ToolId::Activity]
        ));
        assert!(serde_json::from_value::<Request>(
            json!({"command": "setPinnedTools", "args": {"tools": ["nothing"]}})
        )
        .is_err());
        assert!(serde_json::from_value::<Request>(
            json!({"command": "savePreferences", "args": {}})
        )
        .is_err());
    }

    const DEVICE: &str = "0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210";

    fn route(label: &str) -> Result<Route, &'static str> {
        Route::of_window(label, |id| id == DEVICE)
    }

    #[test]
    fn a_window_is_routed_by_its_label_and_by_the_devices_this_one_controls() {
        assert_eq!(route("main"), Ok(Route::Local));
        assert_eq!(route(&format!("remote-{DEVICE}")), Ok(Route::Device(DEVICE.into())));
        // A device this one does not control, and every label that is not exactly ours.
        for label in [
            format!("remote-{OTHER}"),
            "remote-".to_owned(),
            "remote-short".to_owned(),
            format!("remote-{}", DEVICE.to_uppercase()),
            format!("remote-{DEVICE}0"),
            format!("remote-{DEVICE} "),
            format!("Remote-{DEVICE}"),
            format!("remote-../{}", &DEVICE[..29]),
            format!("main-{DEVICE}"),
            String::new(),
            "main2".to_owned(),
            "other".to_owned(),
        ] {
            assert_eq!(route(&label), Err("notPermitted"), "{label}");
        }
    }

    #[test]
    fn a_remote_window_forwards_everything_but_what_is_about_the_viewer() {
        let device = Route::Device(DEVICE.into());
        let forwarded = [
            json!({"command": "getStatuses"}),
            json!({"command": "getPreferences"}),
            json!({"command": "setToolActive", "args": {"tool": "proxy", "active": true}}),
            json!({"command": "getDevices"}),
            json!({"command": "forgetDevice", "args": {"id": OTHER}}),
            json!({"command": "getSession"}),
            json!({"command": "saveProxyApps", "args": {"apps": []}}),
        ];
        for value in forwarded {
            assert_eq!(plan(&device, &request(value)), Plan::Forward(DEVICE.into()));
        }
        for value in [json!({"command": "getSystemLanguage"}), json!({"command": "getViewer"})] {
            assert_eq!(plan(&device, &request(value)), Plan::Here);
        }
    }

    #[test]
    fn the_main_window_runs_everything_here() {
        for value in [
            json!({"command": "getStatuses"}),
            json!({"command": "getViewer"}),
            json!({"command": "openRemoteWindow", "args": {"id": DEVICE}}),
        ] {
            assert_eq!(plan(&Route::Local, &request(value)), Plan::Here);
        }
    }

    #[test]
    fn a_remote_window_cannot_open_windows_or_call_other_devices() {
        let device = Route::Device(DEVICE.into());
        for value in [
            json!({"command": "openRemoteWindow", "args": {"id": DEVICE}}),
            json!({"command": "remoteCall", "args": {"id": OTHER, "request": {"command": "getStatuses"}}}),
        ] {
            assert_eq!(plan(&device, &request(value)), Plan::Refuse);
        }
    }

    #[test]
    fn only_the_language_questions_run_on_the_viewer() {
        // A new request that runs on the viewer must be added here on purpose.
        let viewer = [
            json!({"command": "getSystemLanguage"}),
            json!({"command": "getViewer"}),
        ];
        for value in viewer {
            assert!(request(value).runs_on_viewer());
        }
        for value in [
            json!({"command": "getPreferences"}),
            json!({"command": "setLanguage", "args": {"language": "es"}}),
            json!({"command": "setTheme", "args": {"theme": "dark"}}),
            json!({"command": "getSession"}),
            json!({"command": "getUpdateStatus"}),
            json!({"command": "checkForUpdate"}),
        ] {
            assert!(!request(value).runs_on_viewer());
        }
    }

    #[test]
    fn the_viewer_and_the_windows_are_not_something_a_remote_device_may_ask() {
        let preferences = Preferences::default();
        let both = Origin::Remote(Permissions { control: true, applications: true, actions: true, peer: None });
        for value in [
            json!({"command": "getViewer"}),
            json!({"command": "openRemoteWindow", "args": {"id": DEVICE}}),
            json!({"command": "remoteCall", "args": {"id": DEVICE, "request": {"command": "getStatuses"}}}),
        ] {
            let parsed = request(value);
            assert_eq!(allowed(both, &parsed, &preferences), Err("localOnly"));
            assert_eq!(allowed(Origin::Local, &parsed, &preferences), Ok(()));
        }
        assert!(request(json!({"command": "getViewer"})).is_read());
    }

    #[test]
    fn a_paired_device_with_no_permission_may_ask_who_answers() {
        let preferences = Preferences::default();
        let session = request(json!({"command": "getSession"}));
        assert_eq!(session.access(&preferences), Access::Paired);
        assert_eq!(allowed(NONE, &session, &preferences), Ok(()));
        assert_eq!(allowed(CONTROL, &session, &preferences), Ok(()));
        assert_eq!(allowed(Origin::Local, &session, &preferences), Ok(()));
        assert!(session.is_read());
        // Paired is not control: the next request of the same device is still refused.
        let status = request(json!({"command": "getStatuses"}));
        assert_eq!(allowed(NONE, &status, &preferences), Err("notPermitted"));
    }

    fn application_requests() -> [Value; 3] {
        [
            json!({"command": "listApplications"}),
            json!({"command": "getAppIcon", "args": {"bundleId": "com.apple.Safari"}}),
            json!({"command": "openApplication", "args": {"bundleId": "com.apple.Safari"}}),
        ]
    }

    #[test]
    fn applications_need_their_own_permission_and_control_alone_is_refused() {
        let preferences = Preferences::default();
        let both = Origin::Remote(Permissions { control: true, applications: true, actions: true, peer: None });
        for value in application_requests() {
            let parsed = request(value);
            assert_eq!(parsed.access(&preferences), Access::Applications);
            assert_eq!(allowed(NONE, &parsed, &preferences), Err("notPermitted"));
            assert_eq!(allowed(CONTROL, &parsed, &preferences), Err("notPermitted"));
            assert_eq!(allowed(ACTIONS, &parsed, &preferences), Err("notPermitted"));
            // Applications alone is enough.
            assert_eq!(allowed(APPLICATIONS, &parsed, &preferences), Ok(()));
            assert_eq!(allowed(both, &parsed, &preferences), Ok(()));
            assert_eq!(allowed(Origin::Local, &parsed, &preferences), Ok(()));
        }
        // And Applications does not open Control.
        let status = request(json!({"command": "getStatuses"}));
        assert_eq!(allowed(APPLICATIONS, &status, &preferences), Err("notPermitted"));
    }

    #[test]
    fn only_opening_an_application_is_not_a_read() {
        let [list, icon, open] = application_requests().map(request);
        assert!(list.is_read() && icon.is_read() && !open.is_read());
        for parsed in [&list, &icon, &open] {
            // They run on the device that is asked, also from the window of a device.
            assert!(!parsed.runs_on_viewer());
            assert!(!parsed.is_controller_side());
            assert!(parsed.may_be_called_remotely());
        }
    }

    #[test]
    fn the_icon_cache_and_the_pins_belong_to_the_controller() {
        let preferences = Preferences::default();
        let device = Route::Device(DEVICE.into());
        for value in [
            json!({"command": "getCachedIcon", "args": {"id": DEVICE, "bundleId": "com.apple.Safari", "version": "17.0"}}),
            json!({"command": "setPinnedApps", "args": {"id": DEVICE, "bundleIds": ["com.apple.Safari"]}}),
        ] {
            let parsed = request(value);
            assert!(parsed.is_controller_side());
            assert!(!parsed.may_be_called_remotely());
            assert_eq!(plan(&device, &parsed), Plan::Refuse);
            assert_eq!(plan(&Route::Local, &parsed), Plan::Here);
            for origin in [NONE, CONTROL, APPLICATIONS, ACTIONS] {
                assert_eq!(allowed(origin, &parsed, &preferences), Err("localOnly"));
            }
            assert_eq!(allowed(Origin::Local, &parsed, &preferences), Ok(()));
        }
    }

    #[test]
    fn the_devices_view_may_call_only_a_few_requests_on_a_device() {
        for value in [
            json!({"command": "getStatuses"}),
            json!({"command": "setToolActive", "args": {"tool": "caffeinate", "active": false}}),
            json!({"command": "getSession"}),
        ].into_iter().chain(application_requests()) {
            assert!(request(value).may_be_called_remotely());
        }
        for value in [
            json!({"command": "getPreferences"}),
            json!({"command": "saveProxyApps", "args": {"apps": []}}),
            json!({"command": "forgetDevice", "args": {"id": DEVICE}}),
            json!({"command": "remoteCall", "args": {"id": DEVICE, "request": {"command": "getStatuses"}}}),
            json!({"command": "restartToUpdate"}),
        ] {
            assert!(!request(value).may_be_called_remotely());
        }
    }

    fn read_requests() -> [Value; 2] {
        [
            json!({"command": "listActions"}),
            json!({"command": "getActionRun", "args": {"runId": "0123456789abcdef"}}),
        ]
    }

    fn actions_requests() -> Vec<Value> {
        let id = "0123456789abcdef0123456789abcdef";
        let mut all = read_requests().to_vec();
        all.extend([
            json!({"command": "runAction", "args": {"id": id}}),
            json!({"command": "stopActionRun", "args": {"runId": "0123456789abcdef"}}),
            json!({"command": "scheduleAction", "args": {"id": id, "schedule": {"kind": "daily", "time": "02:00"}, "notifyOnFailure": true}}),
            json!({"command": "clearSchedule", "args": {"id": id}}),
        ]);
        all
    }

    fn writing_requests() -> [Value; 3] {
        let id = "0123456789abcdef0123456789abcdef";
        [
            json!({"command": "createAction", "args": {"name": "x", "command": "echo hi"}}),
            json!({"command": "updateAction", "args": {"id": id, "name": "x", "command": "echo hi", "timeoutSeconds": 10}}),
            json!({"command": "deleteAction", "args": {"id": id}}),
        ]
    }

    #[test]
    fn remote_control_is_refused_for_every_action_request() {
        let preferences = Preferences::default();
        let all = Origin::Remote(Permissions { control: true, applications: true, actions: false, peer: None });
        for value in actions_requests().into_iter().chain(writing_requests()) {
            let parsed = request(value.clone());
            for origin in [NONE, CONTROL, APPLICATIONS, all] {
                assert!(allowed(origin, &parsed, &preferences).is_err(), "{value}");
            }
            assert_eq!(allowed(Origin::Local, &parsed, &preferences), Ok(()));
        }
    }

    #[test]
    fn remote_actions_can_run_and_schedule_but_not_write_an_action() {
        let preferences = Preferences::default();
        for value in actions_requests() {
            let parsed = request(value.clone());
            assert_eq!(parsed.access(&preferences), Access::Actions, "{value}");
            assert_eq!(allowed(ACTIONS, &parsed, &preferences), Ok(()), "{value}");
            assert_eq!(allowed(NONE, &parsed, &preferences), Err("notPermitted"), "{value}");
        }
        for value in writing_requests() {
            let parsed = request(value.clone());
            assert_eq!(parsed.access(&preferences), Access::LocalOnly, "{value}");
            let both = Origin::Remote(Permissions { control: true, applications: true, actions: true, peer: None });
            for origin in [NONE, CONTROL, ACTIONS, both] {
                assert_eq!(allowed(origin, &parsed, &preferences), Err("localOnly"), "{value}");
            }
        }
    }

    #[test]
    fn the_action_requests_say_what_they_read_and_where_they_run() {
        for value in read_requests() {
            assert!(request(value).is_read());
        }
        for value in actions_requests().into_iter().skip(2).chain(writing_requests()) {
            assert!(!request(value).is_read());
        }
        for value in actions_requests().into_iter().chain(writing_requests()) {
            let parsed = request(value);
            // An action belongs to the device that is asked, also from the window of a device.
            assert!(!parsed.runs_on_viewer() && !parsed.is_controller_side());
            assert_eq!(plan(&Route::Device(DEVICE.into()), &parsed), Plan::Forward(DEVICE.into()));
        }
    }

    #[test]
    fn the_devices_view_may_run_and_schedule_actions_but_not_write_them() {
        for value in actions_requests() {
            assert!(request(value).may_be_called_remotely());
        }
        for value in writing_requests() {
            assert!(!request(value).may_be_called_remotely());
        }
    }

    #[test]
    fn turning_on_the_start_of_the_tools_at_launch_is_for_the_person_here() {
        let preferences = Preferences::default();
        let on = request(json!({"command": "setStartToolsOnLaunch", "args": {"enabled": true}}));
        let off = request(json!({"command": "setStartToolsOnLaunch", "args": {"enabled": false}}));
        let both = Origin::Remote(Permissions { control: true, applications: true, actions: true, peer: None });
        for origin in [NONE, CONTROL, ACTIONS, both] {
            assert_eq!(allowed(origin, &on, &preferences), Err("localOnly"));
        }
        assert_eq!(allowed(Origin::Local, &on, &preferences), Ok(()));
        // Turning it off stays remote control.
        assert_eq!(allowed(CONTROL, &off, &preferences), Ok(()));
        assert_eq!(allowed(NONE, &off, &preferences), Err("notPermitted"));
        assert!(!on.is_read() && !off.is_read());
    }

    #[test]
    fn pinning_tools_is_remote_control_and_not_a_read() {
        let preferences = Preferences::default();
        let pin = request(json!({"command": "setPinnedTools", "args": {"tools": ["caffeinate"]}}));
        assert_eq!(allowed(CONTROL, &pin, &preferences), Ok(()));
        assert_eq!(allowed(NONE, &pin, &preferences), Err("notPermitted"));
        assert_eq!(allowed(Origin::Local, &pin, &preferences), Ok(()));
        assert!(!pin.is_read() && !pin.runs_on_viewer());
    }

    #[test]
    fn only_the_person_here_adds_the_proxy_to_quick_start() {
        let preferences = Preferences::default();
        let proxy_on = request(json!({"command": "setQuickStart", "args": {"tool": "proxy", "include": true}}));
        let proxy_off = request(json!({"command": "setQuickStart", "args": {"tool": "proxy", "include": false}}));
        let caffeinate_on = request(json!({"command": "setQuickStart", "args": {"tool": "caffeinate", "include": true}}));
        assert_eq!(allowed(CONTROL, &proxy_on, &preferences), Err("localOnly"));
        assert_eq!(allowed(Origin::Local, &proxy_on, &preferences), Ok(()));
        assert_eq!(allowed(CONTROL, &proxy_off, &preferences), Ok(()));
        assert_eq!(allowed(CONTROL, &caffeinate_on, &preferences), Ok(()));
    }

    #[test]
    fn the_id_of_a_device_in_an_origin_is_exact() {
        let id = "0123456789abcdef0123456789abcdef";
        let origin = Origin::Remote(Permissions { peer: PeerId::parse(id), ..Permissions::default() });
        assert_eq!(origin.peer(), Some(id));
        assert_eq!(Origin::Local.peer(), None);
        for bad in ["", "short", &id.to_uppercase(), &format!("{id}0"), "../0123456789abcdef0123456789abcd"] {
            assert!(PeerId::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn no_shell_request_is_accepted_from_any_remote_origin() {
        let preferences = Preferences::default();
        let id = "0123456789abcdef0123456789abcdef";
        let all = Origin::Remote(Permissions { control: true, applications: true, actions: true, peer: PeerId::parse(id) });
        for value in [
            json!({"command": "grantShell", "args": {"id": id, "minutes": 60}}),
            json!({"command": "grantShell", "args": {"id": id, "minutes": null}}),
            json!({"command": "revokeShell", "args": {"id": id}}),
            json!({"command": "dismissKeysWarning", "args": {"id": 1}}),
            json!({"command": "openShell", "args": {"id": id}}),
        ] {
            let parsed = request(value.clone());
            for origin in [NONE, CONTROL, APPLICATIONS, ACTIONS, all] {
                assert_eq!(allowed(origin, &parsed, &preferences), Err("localOnly"), "{value}");
            }
            assert_eq!(allowed(Origin::Local, &parsed, &preferences), Ok(()));
            // The Devices view cannot send one through `RemoteCall` to another device either.
            assert!(!parsed.may_be_called_remotely(), "{value}");
            assert!(!parsed.is_read() && !parsed.runs_on_viewer());
        }
    }

    #[test]
    fn a_window_that_controls_a_device_cannot_open_a_shell_and_a_grant_goes_to_that_device_to_be_refused() {
        let id = "0123456789abcdef0123456789abcdef";
        let route = Route::Device(id.to_owned());
        assert_eq!(plan(&route, &request(json!({"command": "openShell", "args": {"id": id}}))), Plan::Refuse);
        // A grant from such a window is not run here. It goes to the device, which answers `localOnly`.
        let grant = request(json!({"command": "grantShell", "args": {"id": id, "minutes": 5}}));
        assert_eq!(plan(&route, &grant), Plan::Forward(id.to_owned()));
        assert_eq!(plan(&Route::Local, &grant), Plan::Here);
        assert_eq!(plan(&Route::Local, &request(json!({"command": "openShell", "args": {"id": id}}))), Plan::Here);
        // A label that is not a controlled device gets nothing.
        assert_eq!(Route::of_window("remote-0123", |_| true), Err("notPermitted"));
        assert_eq!(Route::of_window("other", |_| true), Err("notPermitted"));
    }
}
