//! macOS notifications through `UNUserNotificationCenter`, and the notification sound.
//!
//! Tauri's notification plugin does not tell a desktop app which notification was clicked, so the
//! app owns the delegate. Each banner carries the id of its entry in the bell and a route. A click
//! removes the entry from the bell, shows the panel, and sends the route to the main window.

use crate::{
    inbox::{self, Entry, Inbox},
    panel,
    preferences::{speaks_spanish, NotificationMode, SoundName},
    Controller,
};
use block2::RcBlock;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::ProtocolObject, AllocAnyThread, DefinedClass,
    MainThreadMarker,
};
use objc2_app_kit::NSSound;
use objc2_foundation::{NSDictionary, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSettings, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::{process::Command, ptr::NonNull, sync::mpsc, time::Duration};
use tauri::{AppHandle, Emitter, Manager};

/// The event that the main window receives with the route to open.
pub const OPEN_EVENT: &str = "notification-open";
/// The event that tells the main window to read the macOS permission again.
pub const PERMISSION_EVENT: &str = "notification-permission-changed";
const ROUTE_KEY: &str = "route";

pub struct Ivars {
    app: AppHandle,
}

define_class!(
    // SAFETY: NSObject has no subclassing rules, and the delegate keeps only an AppHandle.
    #[unsafe(super(NSObject))]
    #[name = "LazyWorkerNotificationDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        // A banner that arrives while the app is in front still shows. The app plays its own sound.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &block2::DynBlock<dyn Fn()>,
        ) {
            let request = response.notification().request();
            let id = request.identifier().to_string();
            let route = request
                .content()
                .userInfo()
                .objectForKey(&NSString::from_str(ROUTE_KEY))
                .and_then(|value| value.downcast::<NSString>().ok())
                .map(|value| value.to_string());
            open(&self.ivars().app, Some(&id), route.as_deref().unwrap_or("bell"));
            completion.call(());
        }
    }
);

/// Shows the panel at a route. A click on a banner names its entry, which then leaves the bell.
pub fn open(app: &AppHandle, id: Option<&str>, route: &str) {
    if let (Some(id), Some(inbox)) = (id, app.try_state::<Inbox>()) {
        if inbox.dismiss(&[id.to_owned()]).unwrap_or(false) {
            inbox::changed(app);
        }
    }
    let _ = panel::show(app);
    let _ = app.emit_to("main", OPEN_EVENT, route);
}

/// Sets the delegate and asks for permission. macOS asks the person only once.
pub fn install(app: &AppHandle) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let delegate: Retained<Delegate> = {
        let this = Delegate::alloc().set_ivars(Ivars { app: app.clone() });
        // SAFETY: NSObject's init takes no arguments.
        unsafe { msg_send![super(this), init] }
    };
    let center = UNUserNotificationCenter::currentNotificationCenter();
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // The center keeps a weak reference, so the delegate lives as long as the app.
    std::mem::forget(delegate);
    // A refusal only leaves the banners off. The bell and the sound still work.
    let answer = RcBlock::new(|_granted: objc2::runtime::Bool, _error: *mut objc2_foundation::NSError| {});
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &answer,
    );
}

/// Whether macOS lets this app show banners. It waits at most two seconds for the answer.
pub fn permission() -> &'static str {
    let (sender, receiver) = mpsc::channel();
    let answer = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: The center passes valid settings for the length of the call.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = sender.send(match status {
            UNAuthorizationStatus::NotDetermined => "notDetermined",
            UNAuthorizationStatus::Denied => "denied",
            _ => "authorized",
        });
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .getNotificationSettingsWithCompletionHandler(&answer);
    receiver.recv_timeout(Duration::from_secs(2)).unwrap_or("unknown")
}

/// Asks macOS for permission again, and tells the main window when the person answers.
pub fn request_permission(app: &AppHandle) {
    let app = app.clone();
    let answer = RcBlock::new(move |_granted: objc2::runtime::Bool, _error: *mut objc2_foundation::NSError| {
        let _ = app.emit_to("main", PERMISSION_EVENT, ());
    });
    UNUserNotificationCenter::currentNotificationCenter().requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &answer,
    );
}

/// The address of the Notifications pane at one app. It holds the bundle identifier and nothing else.
fn settings_url(identifier: &str) -> Option<String> {
    let valid = !identifier.is_empty()
        && identifier.len() <= 255
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'));
    valid.then(|| format!("x-apple.systempreferences:com.apple.preference.notifications?id={identifier}"))
}

/// Opens the Notifications pane of System Settings at this app.
pub fn open_settings(identifier: &str) -> Result<(), &'static str> {
    let url = settings_url(identifier).ok_or("notificationSettingsFailed")?;
    let opened = Command::new("/usr/bin/open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if opened {
        Ok(())
    } else {
        Err("notificationSettingsFailed")
    }
}

/// Shows a banner. A click opens `route` in the panel and removes the entry `id` from the bell.
fn post(id: &str, title: &str, body: &str, route: &str) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let key = NSString::from_str(ROUTE_KEY);
    let value = NSString::from_str(route);
    let info: Retained<NSDictionary<NSString, NSString>> =
        NSDictionary::from_slices(&[&*key], &[&*value]);
    // SAFETY: The dictionary holds only strings, which a notification can store, and erasing its
    // key and value types keeps the same object.
    unsafe { content.setUserInfo(&Retained::cast_unchecked::<NSDictionary>(info)) };
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(id),
        &content,
        None,
    );
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, None);
}

/// Plays a system sound on the main thread.
pub fn play(app: &AppHandle, sound: SoundName) {
    let _ = app.run_on_main_thread(move || {
        if let Some(sound) = NSSound::soundNamed(&NSString::from_str(sound.system_name())) {
            sound.play();
        }
    });
}

/// The banner of an entry, in the language of the app. An entry with no banner text gets none.
fn banner(entry: &Entry, spanish: bool) -> Option<(String, String)> {
    let param = |name: &str| entry.params.get(name).map(String::as_str).unwrap_or_default();
    match entry.key.as_str() {
        "notifyUpdate" => Some(if spanish {
            (format!("La versión {} está lista", param("version")), "Reinicia para actualizar".into())
        } else {
            (format!("Version {} is ready", param("version")), "Restart to update".into())
        }),
        // The device name and the text came from another device. A banner shows them as plain text.
        "mailboxText" => Some((param("device").to_owned(), param("item").to_owned())),
        "mailboxFile" => Some(if spanish {
            (format!("{} te envió un archivo", param("device")), param("item").to_owned())
        } else {
            (format!("{} sent you a file", param("device")), param("item").to_owned())
        }),
        _ => None,
    }
}

/// Sounds and shows a banner for a new entry, as the preferences allow.
pub fn alert(app: &AppHandle, entry: &Entry) {
    let Some(preferences) = app
        .try_state::<Controller>()
        .and_then(|controller| controller.lock().ok().map(|tools| tools.activity.preferences()))
    else {
        return;
    };
    let settings = preferences.notifications;
    if settings.mode == NotificationMode::Off {
        return;
    }
    if settings.sound {
        play(app, settings.sound_name);
    }
    if settings.mode == NotificationMode::System {
        if let Some((title, body)) = banner(entry, speaks_spanish(preferences.language)) {
            post(&entry.id, &title, &body, entry.route.as_deref().unwrap_or("bell"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_address_holds_only_a_bundle_identifier() {
        assert_eq!(
            settings_url("com.rendis.lazyworker").as_deref(),
            Some("x-apple.systempreferences:com.apple.preference.notifications?id=com.rendis.lazyworker")
        );
        assert!(settings_url("").is_none());
        assert!(settings_url("com.x&y=1").is_none());
        assert!(settings_url("com x").is_none());
    }
}
