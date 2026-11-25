use serde::Serialize;
use std::{
    fs,
    io::{self, ErrorKind},
    path::PathBuf,
};
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct LaunchAgent<'a> {
    label: &'a str,
    associated_bundle_identifiers: [&'a str; 1],
    program_arguments: [&'a str; 3],
    run_at_load: bool,
}

impl<'a> LaunchAgent<'a> {
    fn new(label: &'a str, bundle_id: &'a str, bundle_path: &'a str) -> Self {
        Self {
            label,
            associated_bundle_identifiers: [bundle_id],
            program_arguments: ["/usr/bin/open", "-a", bundle_path],
            run_at_load: true,
        }
    }
}

fn agent_path(app: &AppHandle) -> Result<PathBuf, &'static str> {
    let home = app
        .path()
        .home_dir()
        .map_err(|_| "loginLaunchCheckFailed")?;
    Ok(home
        .join("Library/LaunchAgents")
        .join(format!("{}.login.plist", app.config().identifier)))
}

pub fn get_login_launch(app: AppHandle) -> Result<bool, &'static str> {
    agent_path(&app)?
        .try_exists()
        .map_err(|_| "loginLaunchCheckFailed")
}

pub fn set_login_launch(app: AppHandle, enabled: bool) -> Result<(), &'static str> {
    let path = agent_path(&app).map_err(|_| "loginLaunchSaveFailed")?;
    if !enabled {
        return match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(_) => Err("loginLaunchSaveFailed"),
        };
    }

    let executable = std::env::current_exe().map_err(|_| "loginLaunchSaveFailed")?;
    let bundle = executable
        .ancestors()
        .nth(3)
        .filter(|bundle| {
            bundle
                .extension()
                .is_some_and(|extension| extension == "app")
                && executable.starts_with(bundle.join("Contents/MacOS"))
        })
        .and_then(|bundle| bundle.to_str())
        .ok_or("loginLaunchRequiresBundle")?;
    let bundle_id = &app.config().identifier;
    let label = format!("{bundle_id}.login");
    let agent = LaunchAgent::new(&label, bundle_id, bundle);
    write_agent(&path, &agent).map_err(|_| "loginLaunchSaveFailed")
}

fn write_agent(path: &std::path::Path, agent: &LaunchAgent<'_>) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let mut data = Vec::new();
    plist::to_writer_xml(&mut data, agent).map_err(io::Error::other)?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, data)?;
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn launch_agent_opens_the_bundle_once_and_escapes_its_path() {
        let bundle = "/Applications/Tools & Utilities/Lazy Worker.app";
        let agent = LaunchAgent::new("test.login", "test", bundle);
        let mut data = Vec::new();
        plist::to_writer_xml(&mut data, &agent).unwrap();
        let decoded: serde_json::Value = plist::from_bytes(&data).unwrap();
        assert_eq!(
            decoded,
            json!({
                "Label": "test.login",
                "AssociatedBundleIdentifiers": ["test"],
                "ProgramArguments": ["/usr/bin/open", "-a", bundle],
                "RunAtLoad": true
            })
        );
    }
}
