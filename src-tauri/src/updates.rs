use std::{
    sync::Mutex,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use tauri::{async_runtime, AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
// A failed check, such as one made while offline, is repeated sooner.
const RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
struct Updates {
    available: Option<Update>,
    installing: bool,
    /// Seconds since the Unix epoch of the last check that got an answer.
    checked: Option<u64>,
}

#[derive(Serialize)]
pub struct UpdateStatus {
    version: Option<String>,
    checked: Option<u64>,
}

pub fn start(app: &AppHandle) {
    app.manage(Mutex::new(Updates::default()));
    let configured = app.config().plugins.0.get("updater").is_some_and(|config| {
        config["endpoints"]
            .as_array()
            .is_some_and(|urls| !urls.is_empty())
            && config["pubkey"]
                .as_str()
                .is_some_and(|key| !key.trim().is_empty())
    });
    if !configured {
        return;
    }

    let app = app.clone();
    thread::spawn(move || loop {
        let checked = async_runtime::block_on(check(&app)).is_ok();
        if !checked {
            eprintln!("Could not check for an update");
        }
        thread::sleep(if checked { CHECK_INTERVAL } else { RETRY_INTERVAL });
    });
}

async fn check(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    let available = app
        .updater_builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .check()
        .await?;

    let state = app.state::<Mutex<Updates>>();
    let changed = if let Ok(mut state) = state.lock() {
        if state.installing {
            return Ok(());
        }
        state.checked = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|time| time.as_secs());
        let changed = state.available.as_ref().map(|update| &update.version)
            != available.as_ref().map(|update| &update.version);
        state.available = available;
        changed
    } else {
        false
    };
    if changed {
        let _ = app.emit_to("main", "available-update-changed", ());
    }
    Ok(())
}

pub fn has_update(app: &AppHandle) -> bool {
    app.try_state::<Mutex<Updates>>()
        .is_some_and(|state| state.lock().is_ok_and(|state| state.available.is_some()))
}

#[tauri::command]
pub fn get_update_status(app: AppHandle) -> UpdateStatus {
    let state = app.try_state::<Mutex<Updates>>();
    let state = state.as_ref().and_then(|state| state.lock().ok());
    UpdateStatus {
        version: state
            .as_ref()
            .and_then(|state| state.available.as_ref())
            .map(|update| update.version.clone()),
        checked: state.as_ref().and_then(|state| state.checked),
    }
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<UpdateStatus, &'static str> {
    check(&app).await.map_err(|_| "updateCheckFailed")?;
    Ok(get_update_status(app))
}

#[tauri::command]
pub async fn restart_to_update(app: AppHandle, version: String) -> Result<(), &'static str> {
    let state = app.state::<Mutex<Updates>>();
    let mut update = {
        let mut state = state.lock().map_err(|_| "updateFailed")?;
        if state.installing {
            return Err("updateFailed");
        }
        let update = state
            .available
            .as_ref()
            .filter(|update| update.version == version)
            .ok_or("updateChanged")?
            .clone();
        state.installing = true;
        update
    };

    update.timeout = Some(Duration::from_secs(5 * 60));
    if update.download_and_install(|_, _| {}, || {}).await.is_err() {
        state.lock().map_err(|_| "updateFailed")?.installing = false;
        return Err("updateFailed");
    }

    crate::remember_running(&app);
    app.request_restart();
    Ok(())
}
