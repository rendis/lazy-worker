#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(unix)]
mod actions;
mod activity;
mod apps;
mod caffeinate;
mod clipboard;
mod devices;
mod dispatch;
mod inbox;
mod lid;
mod preferences;
mod proxy;
#[cfg(unix)]
mod remote;
mod resume;

#[cfg(target_os = "macos")]
mod login_launch;
#[cfg(target_os = "macos")]
mod notifications;
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
use caffeinate::Caffeinate;
use proxy::Proxy;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tauri::{Manager, RunEvent};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum ToolId {
    Activity,
    Caffeinate,
    Proxy,
}

#[derive(Serialize)]
struct Statuses {
    activity: activity::Status,
    caffeinate: caffeinate::Status,
    proxy: proxy::Status,
}

struct Tools {
    activity: Activity,
    caffeinate: Caffeinate,
    proxy: Proxy,
    lid: lid::Lid,
}

impl Tools {
    fn statuses(&mut self) -> Statuses {
        Statuses {
            activity: self.activity.status(),
            caffeinate: self.caffeinate.status(),
            proxy: self.proxy.status(),
        }
    }

    /// The tools an update must start again after its restart.
    /// A scheduled Activity session is left out, because its schedule starts it.
    fn running(&mut self) -> Vec<ToolId> {
        let current = self.statuses();
        [
            (ToolId::Activity, current.activity.mode == "manual"),
            (ToolId::Caffeinate, current.caffeinate.mode != "off"),
            (ToolId::Proxy, current.proxy.mode != "off"),
        ]
        .into_iter()
        .filter_map(|(tool, on)| on.then_some(tool))
        .collect()
    }

    /// Applies the closed-lid choice to the system for the current Caffeinate state.
    fn reconcile_lid(&mut self) -> bool {
        let mode = self.activity.preferences().lid_awake;
        let active = self.caffeinate.status().mode != "off";
        self.lid.reconcile(mode, active)
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
            ToolId::Proxy => {
                self.proxy
                    .set_active(active, &self.activity.preferences().proxy);
            }
        }
    }

    fn save_caffeinate_settings(
        &mut self,
        settings: caffeinate::CaffeinateSettings,
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
        if preferences.quick_start.proxy && current.proxy.mode == "off" {
            self.proxy.set_active(true, &preferences.proxy);
        }
        self.statuses()
    }
}

type Controller = Arc<Mutex<Tools>>;

/// True when this launch found no saved preferences, so it is a new installation.
struct FirstRun(bool);

/// Records the running tools, so the launch after an update starts them again.
#[cfg(target_os = "macos")]
fn remember_running(app: &tauri::AppHandle) {
    let Ok(directory) = app.path().app_config_dir() else {
        return;
    };
    if let Ok(mut tools) = app.state::<Controller>().lock() {
        resume::remember(&directory, &tools.running());
    };
}

fn main() {
    // Started by sshd as `remote --peer <id>` or `remote --pair`: relay lines and exit, before any window code.
    #[cfg(unix)]
    if let Some(code) = remote::helper::run_if_requested() {
        std::process::exit(code);
    }
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder
        .plugin(single_instance::plugin())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init());
    let app = builder
        .setup(|app| {
            let directory = app.path().app_config_dir()?;
            fs::create_dir_all(&directory)?;
            let path = directory.join("preferences.json");
            app.manage(FirstRun(!path.exists()));
            let preferences = preferences::load(&path)?;
            let start_tools_on_launch = preferences.start_tools_on_launch;
            let mut tools = Tools {
                activity: Activity::new(preferences, path),
                caffeinate: Caffeinate::new(),
                proxy: Proxy::new(),
                lid: lid::Lid::new(directory.clone()),
            };
            // After an update, the tools that were on return. Their clients are still open.
            if let Some(resumed) = resume::take(&directory) {
                for tool in resumed {
                    tools.set_active(tool, true);
                }
            } else if start_tools_on_launch {
                tools.start_selected();
                // Opening an app can wait for a shell, so it stays off the startup path.
                let settings = tools.activity.preferences().proxy;
                thread::spawn(move || {
                    for app in settings.apps.iter().filter(|app| app.quick_start) {
                        let _ = proxy::launch(&settings, &app.id);
                    }
                });
            }
            let controller = Arc::new(Mutex::new(tools));
            let clock = Arc::downgrade(&controller);
            app.manage(controller);
            app.manage(inbox::Inbox::open(directory.join("notifications.json")));
            #[cfg(unix)]
            {
                app.manage(remote::service::Service::start(app.handle(), &directory));
                app.manage(remote::transfers::Transfers::new(remote::transfers::AppLink(app.handle().clone())));
                // The actions come after the service, because their runs are written in its log.
                let handle = app.handle().clone();
                let sink: actions::Sink = std::sync::Arc::new(move |entry| {
                    if let Some(service) = handle.try_state::<remote::service::Service>() {
                        service.record(entry);
                    }
                });
                // A file that cannot be read leaves the actions off, and the panel says so. It is never replaced.
                let allowed: actions::PeerCheck = {
                    let handle = app.handle().clone();
                    std::sync::Arc::new(move |id| {
                        handle
                            .try_state::<remote::service::Service>()
                            .is_some_and(|service| service.peer_may_run_actions(id))
                    })
                };
                match actions::Actions::open(directory.join("actions.json"), sink, allowed) {
                    Ok(actions) => {
                        actions.start_scheduler();
                        app.manage(actions);
                    }
                    Err(code) => eprintln!("Lazy Worker: the actions are off ({code})"),
                }
            }
            #[cfg(target_os = "macos")]
            let tray_app = {
                panel::install(app)?;
                tray::install(app)?;
                app.manage(Mutex::new(presentation::Presentation::default()));
                presentation::refresh(app.handle());
                updates::start(app.handle());
                notifications::install(app.handle());
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
                tools.proxy.status();
                tools.reconcile_lid();
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
        .invoke_handler(tauri::generate_handler![dispatch::dispatch])
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
            #[cfg(unix)]
            if let Some(actions) = app.try_state::<actions::Actions>() {
                actions.stop();
            }
            #[cfg(unix)]
            if let Some(service) = app.try_state::<remote::service::Service>() {
                service.stop();
            }
            let controller = app.state::<Controller>();
            if let Ok(mut tools) = controller.lock() {
                tools.set_active(ToolId::Activity, false);
                tools.set_active(ToolId::Caffeinate, false);
                tools.set_active(ToolId::Proxy, false);
                tools.reconcile_lid();
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
