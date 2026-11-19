use tauri::{App, AppHandle, Error, Manager, WindowEvent};

const PANEL_LABEL: &str = "main";

pub fn install(app: &mut App) -> tauri::Result<()> {
    let panel = app
        .get_webview_window(PANEL_LABEL)
        .ok_or(Error::WindowNotFound)?;
    // A minimized window would have its own tile in the Dock.
    panel.set_minimizable(false)?;

    let panel_to_hide = panel.clone();
    panel.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Err(error) = panel_to_hide.hide() {
                eprintln!("Could not hide the Lazy Worker panel: {error}");
            }
        }
    });

    Ok(())
}

pub fn show(app: &AppHandle) -> tauri::Result<()> {
    let panel = app
        .get_webview_window(PANEL_LABEL)
        .ok_or(Error::WindowNotFound)?;
    panel.show()?;
    panel.set_focus()
}
