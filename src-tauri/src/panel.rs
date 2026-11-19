use objc2_app_kit::NSWindow;
use tauri::{App, AppHandle, Error, Manager, WebviewWindow, WindowEvent};

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
    if crate::presentation::is_hidden(app) {
        return Ok(());
    }
    let panel = window(app)?;
    panel.show()?;
    panel.set_focus()
}

pub fn hide_for_presentation(app: &AppHandle) -> tauri::Result<bool> {
    let panel = window(app)?;
    let visible = panel.is_visible()?;
    if visible {
        panel.hide()?;
    }
    Ok(visible)
}

pub fn restore_after_presentation(app: &AppHandle) -> tauri::Result<()> {
    let panel = window(app)?;
    // SAFETY: The window is kept alive by the WebviewWindow and this runs on the main thread.
    let native = unsafe { &*panel.ns_window()?.cast::<NSWindow>() };
    native.orderFront(None);
    Ok(())
}

fn window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    app.get_webview_window(PANEL_LABEL)
        .ok_or(Error::WindowNotFound)
}
