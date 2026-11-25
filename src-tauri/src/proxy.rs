mod apps;
mod server;

pub use apps::{check_server, detect, launch, network};
pub use server::{LogEntry, Peer};

use serde::{Deserialize, Serialize};
use server::Server;
use std::{net::IpAddr, time::Instant};

const DEFAULT_PORT: u16 = 8899;
const MAX_ALLOWED: usize = 64;
const MAX_APPS: usize = 32;
const MAX_COMMAND: usize = 2000;

#[derive(Clone, Copy, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    #[default]
    Server,
    Client,
}

/// The form of a built-in tool: its desktop app or its command-line program.
#[derive(Clone, Copy, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Variant {
    #[default]
    App,
    Code,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyApp {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// `None` keeps the built-in command. `{server}` stands for the server address.
    pub command: Option<String>,
    #[serde(default)]
    pub quick_start: bool,
    #[serde(default)]
    pub variant: Variant,
}

impl ProxyApp {
    fn built_in(id: &str, name: &str, icon: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            icon: icon.into(),
            command: None,
            quick_start: false,
            variant: Variant::default(),
        }
    }
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProxySettings {
    pub role: Role,
    pub port: u16,
    pub allowed: Vec<String>,
    pub server: String,
    pub apps: Vec<ProxyApp>,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            role: Role::default(),
            port: DEFAULT_PORT,
            allowed: Vec::new(),
            server: String::new(),
            apps: vec![
                ProxyApp::built_in("claude", "Claude", "sparkles"),
                ProxyApp::built_in("codex", "Codex", "terminal"),
            ],
        }
    }
}

fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

impl ProxySettings {
    /// Keeps only values the server and the launcher can use.
    pub fn sanitize(&mut self) {
        if self.port < 1024 {
            self.port = DEFAULT_PORT;
        }
        let mut allowed: Vec<String> = Vec::new();
        for address in self.allowed_addresses() {
            let address = address.to_string();
            if !allowed.contains(&address) && allowed.len() < MAX_ALLOWED {
                allowed.push(address);
            }
        }
        self.allowed = allowed;
        self.server = self.server.trim().chars().take(255).collect();

        let mut apps: Vec<ProxyApp> = Vec::new();
        for mut app in std::mem::take(&mut self.apps) {
            app.name = app.name.trim().chars().take(60).collect();
            app.command = app
                .command
                .map(|command| command.trim().to_owned())
                .filter(|command| !command.is_empty() && command.len() <= MAX_COMMAND);
            let usable = is_slug(&app.id)
                && is_slug(&app.icon)
                && !app.name.is_empty()
                && (app.command.is_some() || apps::built_in(&app.id, app.variant).is_some())
                && apps.iter().all(|other| other.id != app.id);
            if usable && apps.len() < MAX_APPS {
                apps.push(app);
            }
        }
        for built_in in Self::default().apps.into_iter().rev() {
            if apps.iter().all(|app| app.id != built_in.id) {
                apps.insert(0, built_in);
            }
        }
        self.apps = apps;
    }

    fn allowed_addresses(&self) -> Vec<IpAddr> {
        self.allowed
            .iter()
            .filter_map(|address| address.trim().parse::<IpAddr>().ok())
            .map(|address| address.to_canonical())
            .collect()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub mode: &'static str,
    pub supported: bool,
    pub seconds_left: Option<u64>,
    pub elapsed_seconds: Option<u64>,
    pub error: Option<&'static str>,
}

struct Session {
    server: Server,
    started_at: Instant,
}

pub struct Proxy {
    session: Option<Session>,
    error: Option<&'static str>,
}

impl Proxy {
    pub fn new() -> Self {
        Self {
            session: None,
            error: None,
        }
    }

    pub fn status(&mut self) -> Status {
        if self
            .session
            .as_ref()
            .is_some_and(|session| !session.server.is_running())
        {
            self.session = None;
            self.error = Some("proxyExited");
        }
        Status {
            mode: if self.session.is_some() { "manual" } else { "off" },
            supported: true,
            seconds_left: None,
            elapsed_seconds: self
                .session
                .as_ref()
                .map(|session| session.started_at.elapsed().as_secs()),
            error: self.error,
        }
    }

    pub fn set_active(&mut self, active: bool, settings: &ProxySettings) {
        if !active {
            self.session = None;
            self.error = None;
        } else if self.status().mode == "off" {
            self.start(settings, Instant::now());
        }
    }

    /// Applies saved settings to a running server. A new port needs a new listener.
    pub fn apply_settings(&mut self, settings: &ProxySettings) {
        let Some(session) = &self.session else {
            return;
        };
        if session.server.port() == settings.port {
            session.server.set_allowed(settings.allowed_addresses());
            return;
        }
        let started_at = session.started_at;
        self.session = None;
        self.start(settings, started_at);
    }

    pub fn peers(&self) -> Vec<Peer> {
        self.session
            .as_ref()
            .map(|session| session.server.peers())
            .unwrap_or_default()
    }

    pub fn log(&self) -> Vec<LogEntry> {
        self.session
            .as_ref()
            .map(|session| session.server.log())
            .unwrap_or_default()
    }

    fn start(&mut self, settings: &ProxySettings, started_at: Instant) {
        match Server::start(settings.port, settings.allowed_addresses()) {
            Ok(server) => {
                self.error = None;
                self.session = Some(Session { server, started_at });
            }
            Err(error) => self.error = Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn old_preferences_get_the_default_proxy_settings() {
        let settings: ProxySettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.port, 8899);
        assert!(settings.role == Role::Server);
        assert_eq!(
            settings.apps.iter().map(|app| app.id.as_str()).collect::<Vec<_>>(),
            ["claude", "codex"]
        );
    }

    #[test]
    fn sanitizing_drops_what_the_server_and_launcher_cannot_use() {
        let mut settings: ProxySettings = serde_json::from_str(
            r#"{
                "port": 80,
                "allowed": ["192.168.1.33", "not an address", "192.168.1.33", "::ffff:10.0.0.2"],
                "server": "  studio.local:8899 ",
                "apps": [
                    {"id": "custom-a", "name": " Gemini ", "icon": "bot", "command": " gemini "},
                    {"id": "custom-b", "name": "No command", "icon": "bot", "command": "  "},
                    {"id": "Bad Id", "name": "Bad", "icon": "bot", "command": "x"},
                    {"id": "codex", "name": "Codex", "icon": "terminal", "command": null, "quickStart": true, "variant": "code"}
                ]
            }"#,
        )
        .unwrap();
        settings.sanitize();
        assert_eq!(settings.port, 8899);
        assert_eq!(settings.allowed, ["192.168.1.33", "10.0.0.2"]);
        assert_eq!(settings.server, "studio.local:8899");
        let apps: Vec<_> = settings
            .apps
            .iter()
            .map(|app| (app.id.as_str(), app.name.as_str(), app.command.as_deref(), app.quick_start))
            .collect();
        assert_eq!(
            apps,
            [
                ("claude", "Claude", None, false),
                ("custom-a", "Gemini", Some("gemini"), false),
                ("codex", "Codex", None, true),
            ]
        );
        assert!(settings.apps[0].variant == Variant::App);
        assert!(settings.apps[2].variant == Variant::Code);
    }

    #[test]
    fn a_busy_port_is_reported_and_a_new_port_restarts_the_server() {
        let busy = TcpListener::bind(("::", 0))
            .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))
            .unwrap();
        let mut settings = ProxySettings {
            port: busy.local_addr().unwrap().port(),
            ..ProxySettings::default()
        };
        let mut proxy = Proxy::new();
        proxy.set_active(true, &settings);
        let status = proxy.status();
        assert_eq!((status.mode, status.error), ("off", Some("proxyPortInUse")));

        // Another test can take a port between its release and its reuse, so each step may retry.
        let free_port = || {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.local_addr().unwrap().port()
        };
        for _ in 0..5 {
            settings.port = free_port();
            proxy.set_active(true, &settings);
            if proxy.status().mode == "manual" {
                break;
            }
        }
        assert_eq!(proxy.status().mode, "manual");
        for _ in 0..5 {
            settings.port = free_port();
            proxy.apply_settings(&settings);
            if proxy.status().mode == "manual" {
                break;
            }
            proxy.set_active(true, &settings);
        }
        let status = proxy.status();
        assert_eq!((status.mode, status.error), ("manual", None));
        proxy.set_active(false, &settings);
        assert_eq!(proxy.status().mode, "off");
    }
}
