use super::{ProxySettings, Variant};
use serde::Serialize;
use std::{
    net::{TcpStream, ToSocketAddrs, UdpSocket},
    time::Duration,
};

const ENVIRONMENT: &str =
    "HTTPS_PROXY=http://{server} HTTP_PROXY=http://{server} NO_PROXY=localhost,127.0.0.1,::1";
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

const CHROMIUM_PROXY: &str =
    r#"--args --proxy-server="http://{server}" --proxy-bypass-list="localhost;127.0.0.1;::1""#;

/// The command of an app the user has not edited. Keep it equal to `src/proxy.ts`.
pub fn built_in(id: &str, variant: Variant) -> Option<String> {
    match (id, variant) {
        ("claude", Variant::App) => Some(format!("open -a Claude {CHROMIUM_PROXY}")),
        ("claude", Variant::Code) => Some(format!("{ENVIRONMENT} claude")),
        // The Codex app has a Chromium interface and a separate process that reads the environment.
        ("codex", Variant::App) => Some(format!(
            "open -b com.openai.codex --env HTTPS_PROXY=http://{{server}} --env HTTP_PROXY=http://{{server}} --env NO_PROXY=localhost,127.0.0.1,::1 {CHROMIUM_PROXY}"
        )),
        ("codex", Variant::Code) => Some(format!("{ENVIRONMENT} codex")),
        _ => None,
    }
}

/// A server address is a host and a port. It goes into a shell command, so its characters are limited.
fn valid_server(server: &str) -> bool {
    let Some((host, port)) = server.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && server.len() <= 255
        && port.parse::<u16>().is_ok_and(|port| port != 0)
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-:[]".contains(&byte))
}

fn command(settings: &ProxySettings, id: &str) -> Result<String, &'static str> {
    let app = settings
        .apps
        .iter()
        .find(|app| app.id == id)
        .ok_or("proxyLaunchFailed")?;
    if !valid_server(&settings.server) {
        return Err("proxyServerInvalid");
    }
    let template = app
        .command
        .clone()
        .or_else(|| built_in(id, app.variant))
        .ok_or("proxyLaunchFailed")?;
    Ok(template.replace("{server}", &settings.server))
}

pub fn launch(settings: &ProxySettings, id: &str) -> Result<(), &'static str> {
    platform::launch(id, &command(settings, id)?)
}

pub fn check_server(address: &str) -> bool {
    valid_server(address)
        && address.to_socket_addrs().is_ok_and(|mut addresses| {
            addresses.any(|address| TcpStream::connect_timeout(&address, CHECK_TIMEOUT).is_ok())
        })
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Installs {
    pub claude_code: bool,
    pub claude_app: bool,
    pub codex_code: bool,
    pub codex_app: bool,
}

pub fn detect() -> Installs {
    platform::detect()
}

#[derive(Serialize)]
pub struct Network {
    pub hostname: Option<String>,
    pub addresses: Vec<String>,
}

pub fn network() -> Network {
    // Connecting a UDP socket sends nothing. It only selects the outgoing interface.
    let address = UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("192.0.2.1:9")?;
            socket.local_addr()
        })
        .ok()
        .map(|address| address.ip())
        .filter(|address| !address.is_loopback() && !address.is_unspecified());
    Network {
        hostname: platform::hostname(),
        addresses: address.iter().map(ToString::to_string).collect(),
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Installs;
    use std::{
        env, fs,
        io::Read,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process::{Command, Stdio},
        sync::OnceLock,
        thread,
        time::{Duration, Instant},
    };

    const PATH_MARKER: &str = "__LAZY_WORKER_PATH__";
    const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

    fn home() -> Option<PathBuf> {
        env::var_os("HOME").map(PathBuf::from)
    }

    /// A shell that accepts `NAME=value command`. Other shells fall back to zsh.
    fn shell() -> String {
        env::var("SHELL")
            .ok()
            .filter(|shell| shell.ends_with("/zsh") || shell.ends_with("/bash"))
            .unwrap_or_else(|| "/bin/zsh".into())
    }

    pub fn hostname() -> Option<String> {
        let output = Command::new("/usr/sbin/scutil")
            .args(["--get", "LocalHostName"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let name = String::from_utf8(output.stdout).ok()?;
        let name = name.trim();
        (output.status.success() && !name.is_empty()).then(|| format!("{name}.local"))
    }

    /// An app started from Finder has a short PATH. The login shell has the one the user sees in a terminal.
    fn shell_path() -> Vec<PathBuf> {
        let Ok(mut child) = Command::new(shell())
            .args(["-ilc", &format!("printf '\\n{PATH_MARKER}%s\\n' \"$PATH\"")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return Vec::new();
        };
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let reader = thread::spawn(move || {
            let mut output = String::new();
            let _ = stdout.read_to_string(&mut output);
            output
        });
        let deadline = Instant::now() + SHELL_TIMEOUT;
        while matches!(child.try_wait(), Ok(None)) {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            thread::sleep(Duration::from_millis(40));
        }
        let output = reader.join().unwrap_or_default();
        output
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix(PATH_MARKER))
            .map(|path| env::split_paths(path).collect())
            .unwrap_or_default()
    }

    fn search_path() -> &'static [PathBuf] {
        static PATH: OnceLock<Vec<PathBuf>> = OnceLock::new();
        PATH.get_or_init(|| {
            let mut directories = shell_path();
            if let Some(home) = home() {
                directories.extend(
                    [".local/bin", ".claude/local", ".npm-global/bin", ".bun/bin", ".volta/bin"]
                        .map(|directory| home.join(directory)),
                );
            }
            directories.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
            directories
        })
    }

    fn has_command(name: &str) -> bool {
        search_path()
            .iter()
            .any(|directory| directory.join(name).is_file())
    }

    /// Finds an app by its bundle identifier, because its file name can differ between releases.
    fn has_app(names: &[&str], identifier: &str) -> bool {
        let roots = [Some(PathBuf::from("/")), home()];
        roots.iter().flatten().any(|root| {
            names.iter().any(|name| {
                let info = root.join("Applications").join(name).join("Contents/Info.plist");
                plist::Value::from_file(info).is_ok_and(|value| {
                    value
                        .as_dictionary()
                        .and_then(|info| info.get("CFBundleIdentifier"))
                        .and_then(plist::Value::as_string)
                        == Some(identifier)
                })
            })
        })
    }

    pub fn detect() -> Installs {
        Installs {
            claude_code: has_command("claude"),
            claude_app: has_app(&["Claude.app"], "com.anthropic.claudefordesktop"),
            codex_code: has_command("codex"),
            codex_app: has_app(&["ChatGPT.app", "Codex.app"], "com.openai.codex"),
        }
    }

    pub fn launch(id: &str, command: &str) -> Result<(), &'static str> {
        // `open` starts an app by itself. Any other command needs a terminal to show its interface.
        if command.starts_with("open ") {
            let mut child = Command::new("/bin/sh")
                .args(["-c", command])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| "proxyLaunchFailed")?;
            return match child.wait() {
                Ok(status) if status.success() => Ok(()),
                _ => Err("proxyLaunchFailed"),
            };
        }

        let directory = env::temp_dir().join("lazy-worker");
        fs::create_dir_all(&directory).map_err(|_| "proxyLaunchFailed")?;
        let script = directory.join(format!("{id}.command"));
        // The interactive login shell gives the command the PATH the user has in a terminal.
        // Clearing the screen first leaves only the tool's own interface in the window.
        fs::write(
            &script,
            format!("#!{} -il\nprintf '\\033[3J\\033[H\\033[2J'\n{command}\n", shell()),
        )
            .and_then(|()| fs::set_permissions(&script, fs::Permissions::from_mode(0o700)))
            .map_err(|_| "proxyLaunchFailed")?;
        let opened = Command::new("/usr/bin/open")
            .args(["-a", "Terminal"])
            .arg(&script)
            .stdin(Stdio::null())
            .status();
        match opened {
            Ok(status) if status.success() => Ok(()),
            _ => Err("proxyLaunchFailed"),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Installs;

    pub fn hostname() -> Option<String> {
        None
    }

    pub fn detect() -> Installs {
        Installs::default()
    }

    pub fn launch(_id: &str, _command: &str) -> Result<(), &'static str> {
        Err("proxyLaunchUnsupported")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_address_needs_a_host_and_a_port_and_no_shell_syntax() {
        assert!(valid_server("studio.local:8899"));
        assert!(valid_server("192.168.1.20:8899"));
        assert!(valid_server("[fe80::1]:8899"));
        assert!(!valid_server("studio.local"));
        assert!(!valid_server(":8899"));
        assert!(!valid_server("studio.local:0"));
        assert!(!valid_server("studio.local:8899; rm -rf ~"));
        assert!(!valid_server("$(whoami):8899"));
    }

    #[test]
    fn commands_come_from_the_saved_app_and_carry_the_server_address() {
        let mut settings = ProxySettings {
            server: "studio.local:8899".into(),
            ..ProxySettings::default()
        };
        assert_eq!(
            command(&settings, "claude").unwrap(),
            r#"open -a Claude --args --proxy-server="http://studio.local:8899" --proxy-bypass-list="localhost;127.0.0.1;::1""#
        );
        settings.apps[0].variant = Variant::Code;
        assert_eq!(
            command(&settings, "claude").unwrap(),
            "HTTPS_PROXY=http://studio.local:8899 HTTP_PROXY=http://studio.local:8899 NO_PROXY=localhost,127.0.0.1,::1 claude"
        );
        assert_eq!(
            command(&settings, "codex").unwrap(),
            r#"open -b com.openai.codex --env HTTPS_PROXY=http://studio.local:8899 --env HTTP_PROXY=http://studio.local:8899 --env NO_PROXY=localhost,127.0.0.1,::1 --args --proxy-server="http://studio.local:8899" --proxy-bypass-list="localhost;127.0.0.1;::1""#
        );
        settings.apps[1].command = Some("ALL_PROXY=http://{server} codex --fast".into());
        assert_eq!(
            command(&settings, "codex").unwrap(),
            "ALL_PROXY=http://studio.local:8899 codex --fast"
        );
        assert_eq!(command(&settings, "missing"), Err("proxyLaunchFailed"));
        settings.server = "studio.local".into();
        assert_eq!(command(&settings, "claude"), Err("proxyServerInvalid"));
    }
}
