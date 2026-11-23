//! The window that controls one device. It is the same interface as the panel, and every
//! request it sends goes to that device. Its label says which device, and Rust decides by the
//! label alone.

use tauri::{AppHandle, Manager};

/// The label of the window of a device.
pub fn label(id: &str) -> String {
    format!("remote-{id}")
}

/// The name as one query value: every byte that is not a letter or a digit becomes `%XX`.
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Opens the window of a device that this one controls, or brings the open one forward.
#[cfg(target_os = "macos")]
pub fn open(app: &AppHandle, id: &str) -> Result<(), &'static str> {
    use super::{identity::DeviceId, service::Service};
    use tauri::{LogicalPosition, LogicalSize, TitleBarStyle, WebviewUrl, WebviewWindowBuilder};

    DeviceId::parse(id)?;
    let device = app
        .state::<Service>()
        .device(id)
        .ok_or("notPermitted")?;
    let label = label(id);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.show();
        return window.set_focus().map_err(|_| "remoteWindowFailed");
    }
    // The size and the title bar are those of `main` in `tauri.conf.json`.
    let size = LogicalSize::new(1040.0, 640.0);
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::App(format!("index.html?remote={id}&name={}", percent_encode(&device.name)).into()),
    )
    .title(format!("{} — Lazy Worker", device.name))
    .inner_size(size.width, size.height)
    .min_inner_size(size.width, size.height)
    .max_inner_size(size.width, size.height)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .title_bar_style(TitleBarStyle::Overlay)
    .hidden_title(true)
    .traffic_light_position(LogicalPosition::new(14.0, 26.0))
    .center()
    .build()
    .map_err(|_| "remoteWindowFailed")?;
    window.set_focus().map_err(|_| "remoteWindowFailed")
}

#[cfg(not(target_os = "macos"))]
pub fn open(_: &AppHandle, _: &str) -> Result<(), &'static str> {
    Err("unsupported")
}

/// Closes the window of a device, when there is one.
pub fn close(app: &AppHandle, id: &str) {
    if let Some(window) = app.get_webview_window(&label(id)) {
        let _ = window.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_one_safe_query_value() {
        assert_eq!(percent_encode("Mac mini"), "Mac%20mini");
        assert_eq!(percent_encode("a&b=c#d"), "a%26b%3Dc%23d");
        assert_eq!(percent_encode("é"), "%C3%A9");
    }

    #[test]
    fn the_label_is_the_prefix_and_the_id() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(label(id), format!("remote-{id}"));
    }
}
