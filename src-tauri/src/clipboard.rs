//! Copies text to the system pasteboard.
//!
//! The webview cannot do it after an awaited call: WKWebView drops the user gesture, so
//! `navigator.clipboard.writeText` fails there. The app writes the text itself.

/// The most text one copy holds, in bytes.
pub const MAX_TEXT: usize = 256 * 1024;

/// Puts `text` on the pasteboard, replacing what was there.
pub fn write(text: &str) -> Result<(), &'static str> {
    if text.len() > MAX_TEXT {
        return Err("badRequest");
    }
    platform::write(text)
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;

    /// `NSPasteboard` takes no main thread marker, so this runs on the caller's thread.
    pub fn write(text: &str) -> Result<(), &'static str> {
        unsafe {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            if pasteboard.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) {
                Ok(())
            } else {
                Err("copyFailed")
            }
        }
    }

    #[cfg(test)]
    pub fn read() -> Option<String> {
        unsafe {
            NSPasteboard::generalPasteboard()
                .stringForType(NSPasteboardTypeString)
                .map(|text| text.to_string())
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn write(_text: &str) -> Result<(), &'static str> {
        Err("unsupported")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_over_the_cap_is_refused_before_the_system_sees_it() {
        assert_eq!(write(&"a".repeat(MAX_TEXT + 1)), Err("badRequest"));
    }

    /// Writes to the real pasteboard of the user, so it runs only on request:
    /// `cargo test clipboard -- --ignored`. The previous text comes back afterwards.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_pasteboard_keeps_what_was_written() {
        let before = platform::read();
        let probe = "lazy-worker pasteboard check \u{e9}\u{4e16}";
        let written = write(probe);
        let read = platform::read();
        if let Some(text) = before {
            write(&text).unwrap();
        }
        assert_eq!(written, Ok(()));
        assert_eq!(read.as_deref(), Some(probe));
    }
}
