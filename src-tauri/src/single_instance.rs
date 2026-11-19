use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::ErrorKind,
    path::PathBuf,
};

use tauri::{plugin, AppHandle, Manager, Resource, Wry};

struct InstanceLock {
    _file: File,
}

impl Resource for InstanceLock {}

struct ReopenRequest(PathBuf);

pub fn plugin() -> plugin::TauriPlugin<Wry> {
    plugin::Builder::new("single-instance")
        .setup(|app, _| {
            let directory = app.path().app_config_dir()?;
            fs::create_dir_all(&directory)?;
            let request = directory.join("reopen-panel");
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(directory.join("instance.lock"))?;

            match file.try_lock() {
                Ok(()) => {
                    // Tauri drops resources after the exit handler and before relaunching.
                    app.resources_table().add(InstanceLock { _file: file });
                    app.manage(ReopenRequest(request));
                }
                Err(TryLockError::WouldBlock) => {
                    match File::create_new(request) {
                        Ok(_) => {}
                        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error.into()),
                    }
                    std::process::exit(0);
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
            Ok(())
        })
        .build()
}

pub fn reopen_if_requested(app: &AppHandle) {
    if fs::remove_file(&app.state::<ReopenRequest>().0).is_ok() {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(error) = crate::panel::show(&handle) {
                eprintln!("Could not reopen the Lazy Worker panel: {error}");
            }
        });
    }
}
