//! The native pickers of the person at this device: files to send, a folder, and where to save a file.
//!
//! They run in the app and not in the webview, so the webview needs no permission of the dialog
//! plugin. A picker waits for the person, so call it off the main thread.

use tauri::AppHandle;

#[cfg(target_os = "macos")]
mod native {
    use super::AppHandle;
    use tauri::{Manager, WebviewWindow};
    use tauri_plugin_dialog::{DialogExt, FilePath};

    fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
        app.get_webview_window("main")
    }

    fn path_text(path: FilePath) -> Option<String> {
        path.into_path().ok()?.to_str().map(str::to_owned)
    }

    pub fn files(app: &AppHandle) -> Vec<String> {
        let mut dialog = app.dialog().file();
        if let Some(window) = main_window(app) {
            dialog = dialog.set_parent(&window);
        }
        dialog
            .blocking_pick_files()
            .unwrap_or_default()
            .into_iter()
            .filter_map(path_text)
            .collect()
    }

    pub fn folder(app: &AppHandle) -> Option<String> {
        let mut dialog = app.dialog().file().set_can_create_directories(true);
        if let Some(window) = main_window(app) {
            dialog = dialog.set_parent(&window);
        }
        dialog.blocking_pick_folder().and_then(path_text)
    }

    pub fn save(app: &AppHandle, name: &str) -> Option<String> {
        let mut dialog = app.dialog().file().set_file_name(name);
        if let Ok(downloads) = app.path().download_dir() {
            dialog = dialog.set_directory(downloads);
        }
        if let Some(window) = main_window(app) {
            dialog = dialog.set_parent(&window);
        }
        dialog.blocking_save_file().and_then(path_text)
    }
}

/// The files that the person chose, or none when the person closed the picker.
pub fn files(app: &AppHandle) -> Result<Vec<String>, &'static str> {
    #[cfg(target_os = "macos")]
    return Ok(native::files(app));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("unsupported")
    }
}

pub fn folder(app: &AppHandle) -> Result<Option<String>, &'static str> {
    #[cfg(target_os = "macos")]
    return Ok(native::folder(app));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("unsupported")
    }
}

/// Where the person wants to save a file, starting in Downloads.
pub fn save(app: &AppHandle, name: &str) -> Result<Option<String>, &'static str> {
    #[cfg(target_os = "macos")]
    return Ok(native::save(app, name));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, name);
        Err("unsupported")
    }
}
