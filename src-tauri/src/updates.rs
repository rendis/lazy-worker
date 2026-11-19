use std::{sync::Mutex, thread, time::Duration};

use tauri::{async_runtime, AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

#[derive(Default)]
struct Updates {
    available: Option<Update>,
    installing: bool,
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
        if async_runtime::block_on(check(&app)).is_err() {
            eprintln!("Could not check for an update");
        }
        thread::sleep(CHECK_INTERVAL);
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
pub fn get_available_update(app: AppHandle) -> Option<String> {
    let state = app.try_state::<Mutex<Updates>>()?;
    let state = state.lock().ok()?;
    Some(state.available.as_ref()?.version.clone())
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

    app.request_restart();
    Ok(())
}
