#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activity;
mod caffeinate;
mod preferences;

#[cfg(target_os = "macos")]
mod panel;
#[cfg(target_os = "macos")]
mod presentation;
#[cfg(target_os = "macos")]
mod single_instance;
#[cfg(target_os = "macos")]
mod tray;
#[cfg(target_os = "macos")]
mod updates;

use activity::Activity;
use caffeinate::{Caffeinate, CaffeinateSettings, RunningProcess};
use preferences::Preferences;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tauri::{Manager, RunEvent, State};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum ToolId {
    Activity,
    Caffeinate,
}

#[derive(Serialize)]
struct Statuses {
    activity: activity::Status,
    caffeinate: caffeinate::Status,
}

struct Tools {
    activity: Activity,
    caffeinate: Caffeinate,
}

impl Tools {
    fn statuses(&mut self) -> Statuses {
        Statuses {
            activity: self.activity.status(),
            caffeinate: self.caffeinate.status(),
        }
    }

    fn set_active(&mut self, tool: ToolId, active: bool) {
        match tool {
            ToolId::Activity => {
                self.activity.set_active(active);
            }
            ToolId::Caffeinate => {
                self.caffeinate
                    .set_active(active, &self.activity.preferences().caffeinate);
            }
        }
    }

    fn save_caffeinate_settings(
        &mut self,
        settings: CaffeinateSettings,
    ) -> Result<Statuses, &'static str> {
        settings.validate()?;
        let mut preferences = self.activity.preferences();
        preferences.caffeinate = settings.clone();
        self.activity.save_preferences(preferences)?;
        self.caffeinate.apply_settings(settings);
        Ok(self.statuses())
    }

    fn start_selected(&mut self) -> Statuses {
        let preferences = self.activity.preferences();
        let current = self.statuses();
        if preferences.quick_start.activity
            && current.activity.supported
            && current.activity.mode == "off"
        {
            self.activity.set_active(true);
        }
        if preferences.quick_start.caffeinate
            && current.caffeinate.supported
            && current.caffeinate.mode == "off"
        {
            self.caffeinate.set_active(true, &preferences.caffeinate);
        }
        self.statuses()
    }
}

type Controller = Arc<Mutex<Tools>>;

#[tauri::command]
fn get_preferences(controller: State<'_, Controller>) -> Result<Preferences, &'static str> {
    Ok(controller
        .lock()
        .map_err(|_| "workerUnavailable")?
        .activity
        .preferences())
}

#[tauri::command]
fn get_system_language() -> Option<preferences::Language> {
    #[cfg(target_os = "macos")]
    return Some(preferences::system_language());

    #[cfg(not(target_os = "macos"))]
    None
}

#[tauri::command]
fn save_preferences(
    app: tauri::AppHandle,
    controller: State<'_, Controller>,
    mut preferences: Preferences,
) -> Result<(), &'static str> {
    let mut tools = controller.lock().map_err(|_| "saveFailed")?;
    preferences.caffeinate = tools.activity.preferences().caffeinate;
    #[cfg(target_os = "macos")]
    let presentation_changed =
        preferences.hide_while_sharing != tools.activity.preferences().hide_while_sharing;
    tools.activity.save_preferences(preferences)?;
    drop(tools);
    #[cfg(target_os = "macos")]
    if presentation_changed {
        presentation::refresh(&app);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    Ok(())
}

#[tauri::command]
fn save_caffeinate_settings(
    controller: State<'_, Controller>,
    settings: CaffeinateSettings,
) -> Result<Statuses, &'static str> {
    controller
        .lock()
        .map_err(|_| "saveFailed")?
        .save_caffeinate_settings(settings)
}

#[tauri::command]
async fn list_caffeinate_processes() -> Result<Vec<RunningProcess>, &'static str> {
    tauri::async_runtime::spawn_blocking(caffeinate::list_processes)
        .await
        .map_err(|_| "caffeinateProcessListFailed")?
}

#[tauri::command]
fn get_statuses(controller: State<'_, Controller>) -> Result<Statuses, &'static str> {
    Ok(controller
        .lock()
        .map_err(|_| "workerUnavailable")?
        .statuses())
}

#[tauri::command]
async fn get_activity_accessibility() -> &'static str {
    tauri::async_runtime::spawn_blocking(activity::accessibility_status)
        .await
        .unwrap_or("unavailable")
}

#[tauri::command]
async fn request_activity_accessibility() -> Result<(), &'static str> {
    tauri::async_runtime::spawn_blocking(activity::request_accessibility)
        .await
        .map_err(|_| "accessibilityRequestFailed")?
}

#[tauri::command]
fn set_tool_active(
    controller: State<'_, Controller>,
    tool: ToolId,
    active: bool,
) -> Result<Statuses, &'static str> {
    let mut tools = controller.lock().map_err(|_| "workerUnavailable")?;
    tools.set_active(tool, active);
    Ok(tools.statuses())
}

fn main() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder
        .plugin(single_instance::plugin())
        .plugin(tauri_plugin_updater::Builder::new().build());
    let app = builder
        .setup(|app| {
            let directory = app.path().app_config_dir()?;
            fs::create_dir_all(&directory)?;
            let path = directory.join("preferences.json");
            let preferences = preferences::load(&path)?;
            let controller = Arc::new(Mutex::new(Tools {
                activity: Activity::new(preferences, path),
                caffeinate: Caffeinate::new(),
            }));
            let clock = Arc::downgrade(&controller);
            app.manage(controller);
            #[cfg(target_os = "macos")]
            let tray_app = {
                panel::install(app)?;
                tray::install(app)?;
                app.manage(Mutex::new(presentation::Presentation::default()));
                presentation::refresh(app.handle());
                updates::start(app.handle());
                app.handle().clone()
            };
            thread::spawn(move || loop {
                thread::sleep(Duration::from_secs(1));
                let Some(controller) = clock.upgrade() else {
                    break;
                };
                let Ok(mut tools) = controller.lock() else {
                    break;
                };
                tools.activity.tick();
                tools.caffeinate.status();
                drop(tools);
                #[cfg(target_os = "macos")]
                {
                    tray::refresh(&tray_app);
                    presentation::refresh(&tray_app);
                    single_instance::reopen_if_requested(&tray_app);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_preferences,
            get_system_language,
            save_preferences,
            save_caffeinate_settings,
            list_caffeinate_processes,
            get_statuses,
            get_activity_accessibility,
            request_activity_accessibility,
            set_tool_active,
            #[cfg(target_os = "macos")]
            updates::get_available_update,
            #[cfg(target_os = "macos")]
            updates::restart_to_update
        ])
        .build(tauri::generate_context!())
        .expect("Could not build Lazy Worker");
    #[cfg(target_os = "macos")]
    let app = {
        let mut app = app;
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        app
    };
    app.run(|app, event| match event {
        RunEvent::Exit => {
            #[cfg(target_os = "macos")]
            presentation::stop(app);
            let controller = app.state::<Controller>();
            if let Ok(mut tools) = controller.lock() {
                tools.set_active(ToolId::Activity, false);
                tools.set_active(ToolId::Caffeinate, false);
            };
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => {
            let _ = panel::show(app);
        }
        _ => {}
    });
}
