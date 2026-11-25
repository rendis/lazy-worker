use crate::{panel, preferences::speaks_spanish, Controller, ToolId};
use objc2::{define_class, msg_send, rc::Retained, MainThreadMarker};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSView,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize};
use std::{
    io::{self, Cursor},
    sync::Mutex,
};
use tauri::{
    async_runtime,
    image::Image,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    App, AppHandle, Manager,
};

const TRAY_ID: &str = "lazy-worker";
const CONTROLLED: &str = "tray-controlled";
const NOTIFICATIONS: &str = "tray-notifications";
const OPEN: &str = "tray-open";
const START_SELECTED: &str = "tray-start-selected";
const ACTIVITY: &str = "tray-activity";
const CAFFEINATE: &str = "tray-caffeinate";
const PROXY: &str = "tray-proxy";
const QUIT: &str = "tray-quit";

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct ToolState {
    supported: bool,
    active: bool,
    failed: bool,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct MenuState {
    spanish: bool,
    /// The names of the devices that have an open connection to control this one.
    controlled_by: Vec<String>,
    /// The notifications that the bell shows. Each one is still new.
    unread: usize,
    can_start_selected: bool,
    update_available: bool,
    activity: ToolState,
    caffeinate: ToolState,
    proxy: ToolState,
}

impl MenuState {
    fn read(app: &AppHandle) -> Option<Self> {
        let controller = app.state::<Controller>();
        let mut tools = controller.try_lock().ok()?;
        let preferences = tools.activity.preferences();
        let statuses = tools.statuses();
        let activity = ToolState {
            supported: statuses.activity.supported,
            active: statuses.activity.mode != "off",
            failed: statuses.activity.error.is_some(),
        };
        let caffeinate = ToolState {
            supported: statuses.caffeinate.supported,
            active: statuses.caffeinate.mode != "off",
            failed: statuses.caffeinate.error.is_some(),
        };
        let proxy = ToolState {
            supported: statuses.proxy.supported,
            active: statuses.proxy.mode != "off",
            failed: statuses.proxy.error.is_some(),
        };
        Some(Self {
            spanish: speaks_spanish(preferences.language),
            unread: app
                .try_state::<crate::inbox::Inbox>()
                .map_or(0, |inbox| inbox.count()),
            can_start_selected: (preferences.quick_start.activity
                && activity.supported
                && !activity.active)
                || (preferences.quick_start.caffeinate
                    && caffeinate.supported
                    && !caffeinate.active)
                || (preferences.quick_start.proxy && proxy.supported && !proxy.active),
            update_available: crate::updates::has_update(app),
            controlled_by: controlled_by(app),
            activity,
            caffeinate,
            proxy,
        })
    }

    fn has_active_tool(&self) -> bool {
        self.activity.active || self.caffeinate.active || self.proxy.active
    }

    fn labels(&self) -> Labels {
        let (
            open,
            start_selected,
            quit,
            start_activity,
            stop_activity,
            start_caffeinate,
            stop_caffeinate,
            start_proxy,
            stop_proxy,
        ) = if self.spanish {
            (
                "Abrir panel",
                "Iniciar selección",
                "Salir de Lazy Worker",
                "Iniciar Actividad",
                "Detener Actividad",
                "Iniciar Caffeinate",
                "Detener Caffeinate",
                "Iniciar servidor proxy",
                "Detener servidor proxy",
            )
        } else {
            (
                "Open Panel",
                "Start Selected",
                "Quit Lazy Worker",
                "Start Activity",
                "Stop Activity",
                "Start Caffeinate",
                "Stop Caffeinate",
                "Start Proxy Server",
                "Stop Proxy Server",
            )
        };
        let tool_label = |tool: ToolState, start: &str, stop: &str| {
            let action = if tool.active { stop } else { start };
            let suffix = if tool.failed { " (error)" } else { "" };
            format!("{action}{suffix}")
        };
        let controlled = match (self.controlled_by.as_slice(), self.spanish) {
            ([], _) => String::new(),
            ([name], true) => format!("Controlado por {name}"),
            ([name], false) => format!("Controlled by {name}"),
            (all, true) => format!("Controlado por {} dispositivos", all.len()),
            (all, false) => format!("Controlled by {} devices", all.len()),
        };
        let notifications = match (self.unread, self.spanish) {
            (0, _) => String::new(),
            (1, true) => "1 notificación nueva".into(),
            (1, false) => "1 new notification".into(),
            (count, true) => format!("{count} notificaciones nuevas"),
            (count, false) => format!("{count} new notifications"),
        };
        Labels {
            controlled,
            notifications,
            open: match (self.spanish, self.update_available) {
                (true, true) => "Abrir panel (actualización disponible)",
                (false, true) => "Open Panel (Update Available)",
                _ => open,
            },
            start_selected,
            quit,
            activity: tool_label(self.activity, start_activity, stop_activity),
            caffeinate: tool_label(self.caffeinate, start_caffeinate, stop_caffeinate),
            proxy: tool_label(self.proxy, start_proxy, stop_proxy),
        }
    }
}

struct Labels {
    controlled: String,
    notifications: String,
    open: &'static str,
    start_selected: &'static str,
    quit: &'static str,
    activity: String,
    caffeinate: String,
    proxy: String,
}

struct Controls {
    menu: Menu<tauri::Wry>,
    /// A disabled line and its separator, first in the menu while a device controls this one.
    controlled: MenuItem<tauri::Wry>,
    controlled_separator: PredefinedMenuItem<tauri::Wry>,
    /// A line that opens the bell, before "Open Panel" while some notifications are not read.
    notifications: MenuItem<tauri::Wry>,
    open: MenuItem<tauri::Wry>,
    start_selected: MenuItem<tauri::Wry>,
    activity: MenuItem<tauri::Wry>,
    caffeinate: MenuItem<tauri::Wry>,
    proxy: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
    rendered: Mutex<MenuState>,
}

impl Controls {
    fn update(&self, app: &AppHandle, state: MenuState) -> tauri::Result<()> {
        let rendered = self.rendered.lock().unwrap().clone();
        if rendered == state {
            return Ok(());
        }
        let labels = state.labels();
        if rendered.controlled_by != state.controlled_by {
            let (was, is) = (!rendered.controlled_by.is_empty(), !state.controlled_by.is_empty());
            if is {
                self.controlled.set_text(&labels.controlled)?;
            }
            if is && !was {
                self.menu.insert(&self.controlled_separator, 0)?;
                self.menu.insert(&self.controlled, 0)?;
            } else if was && !is {
                self.menu.remove(&self.controlled)?;
                self.menu.remove(&self.controlled_separator)?;
            }
        }
        if rendered.unread != state.unread || rendered.spanish != state.spanish {
            let (was, is) = (rendered.unread > 0, state.unread > 0);
            if is {
                self.notifications.set_text(&labels.notifications)?;
            }
            // The controlled lines, when shown, stay first.
            let position = if state.controlled_by.is_empty() { 0 } else { 2 };
            if is && !was {
                self.menu.insert(&self.notifications, position)?;
            } else if was && !is {
                self.menu.remove(&self.notifications)?;
            }
        }
        if (rendered.unread > 0) != (state.unread > 0) {
            let tray = app
                .tray_by_id(TRAY_ID)
                .ok_or_else(|| io::Error::other("Missing status item"))?;
            set_unread_dot(&tray, state.unread > 0)?;
        }
        if rendered.spanish != state.spanish || rendered.update_available != state.update_available {
            self.open.set_text(labels.open)?;
        }
        if rendered.spanish != state.spanish {
            self.start_selected.set_text(labels.start_selected)?;
            self.quit.set_text(labels.quit)?;
        }
        self.start_selected.set_enabled(state.can_start_selected)?;
        self.activity.set_text(labels.activity)?;
        self.activity.set_enabled(state.activity.supported)?;
        self.caffeinate.set_text(labels.caffeinate)?;
        self.caffeinate.set_enabled(state.caffeinate.supported)?;
        self.proxy.set_text(labels.proxy)?;
        self.proxy.set_enabled(state.proxy.supported)?;
        if rendered.has_active_tool() != state.has_active_tool() {
            let tray = app
                .tray_by_id(TRAY_ID)
                .ok_or_else(|| io::Error::other("Missing status item"))?;
            set_activity_dot(&tray, state.has_active_tool())?;
        }
        *self.rendered.lock().unwrap() = state;
        Ok(())
    }
}

pub fn install(app: &App) -> tauri::Result<()> {
    let state = MenuState::read(app.handle()).unwrap_or_default();
    let labels = state.labels();
    let item =
        |id: &str, label: &str, enabled| MenuItem::with_id(app, id, label, enabled, None::<&str>);
    let controls = Controls {
        menu: Menu::new(app)?,
        controlled: item(CONTROLLED, &labels.controlled, false)?,
        controlled_separator: PredefinedMenuItem::separator(app)?,
        notifications: item(NOTIFICATIONS, &labels.notifications, true)?,
        open: item(OPEN, labels.open, true)?,
        start_selected: item(
            START_SELECTED,
            labels.start_selected,
            state.can_start_selected,
        )?,
        activity: item(ACTIVITY, &labels.activity, state.activity.supported)?,
        caffeinate: item(CAFFEINATE, &labels.caffeinate, state.caffeinate.supported)?,
        proxy: item(PROXY, &labels.proxy, state.proxy.supported)?,
        quit: item(QUIT, labels.quit, true)?,
        rendered: Mutex::new(state.clone()),
    };
    let menu = &controls.menu;
    if !state.controlled_by.is_empty() {
        menu.append(&controls.controlled)?;
        menu.append(&controls.controlled_separator)?;
    }
    if state.unread > 0 {
        menu.append(&controls.notifications)?;
    }
    menu.append_items(&[
        &controls.open,
        &PredefinedMenuItem::separator(app)?,
        &controls.start_selected,
        &controls.activity,
        &controls.caffeinate,
        &controls.proxy,
        &PredefinedMenuItem::separator(app)?,
        &controls.quit,
    ])?;
    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(windup_icon()?)
        .icon_as_template(true)
        .tooltip("Lazy Worker")
        .menu(menu)
        .on_menu_event(handle_menu_event)
        .build(app)?;
    set_activity_dot(&tray, state.has_active_tool())?;
    set_unread_dot(&tray, state.unread > 0)?;
    app.manage(controls);
    Ok(())
}

/// The names of the devices that have an open connection to this one, each once and in order.
fn controlled_by(app: &AppHandle) -> Vec<String> {
    let Some(service) = app.try_state::<crate::remote::service::Service>() else {
        return Vec::new();
    };
    let mut names: Vec<String> = service
        .controllers()
        .into_iter()
        .filter(|controller| controller.connected)
        .map(|controller| controller.name)
        .collect();
    names.sort();
    names
}

pub fn refresh(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(controls) = handle.try_state::<Controls>() else {
            return;
        };
        if let Some(state) = MenuState::read(&handle) {
            if let Err(error) = controls.update(&handle, state) {
                eprintln!("Could not update the status menu: {error}");
            }
        }
    });
}

enum Action {
    StartSelected,
    SetActive(ToolId, bool),
}

fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    let action = match event.id.as_ref() {
        OPEN => {
            if let Err(error) = panel::show(app) {
                eprintln!("Could not open the panel: {error}");
            }
            return;
        }
        QUIT => {
            app.exit(0);
            return;
        }
        NOTIFICATIONS => {
            crate::notifications::open(app, None, "bell");
            return;
        }
        START_SELECTED => Action::StartSelected,
        ACTIVITY | CAFFEINATE | PROXY => {
            let controls = app.state::<Controls>();
            let rendered = controls.rendered.lock().unwrap().clone();
            match event.id.as_ref() {
                ACTIVITY => Action::SetActive(ToolId::Activity, !rendered.activity.active),
                CAFFEINATE => Action::SetActive(ToolId::Caffeinate, !rendered.caffeinate.active),
                _ => Action::SetActive(ToolId::Proxy, !rendered.proxy.active),
            }
        }
        _ => return,
    };

    let app = app.clone();
    async_runtime::spawn_blocking(move || {
        if let Ok(mut tools) = app.state::<Controller>().lock() {
            match action {
                Action::StartSelected => {
                    tools.start_selected();
                }
                Action::SetActive(tool, active) => {
                    let current = tools.statuses();
                    let (supported, is_active) = match tool {
                        ToolId::Activity => {
                            (current.activity.supported, current.activity.mode != "off")
                        }
                        ToolId::Caffeinate => (
                            current.caffeinate.supported,
                            current.caffeinate.mode != "off",
                        ),
                        ToolId::Proxy => (current.proxy.supported, current.proxy.mode != "off"),
                    };
                    // The tool can finish while AppKit is tracking the menu selection.
                    if supported && is_active != active {
                        tools.set_active(tool, active);
                    }
                }
            }
        }
        refresh(&app);
    });
}

fn windup_icon() -> tauri::Result<Image<'static>> {
    let bytes = include_bytes!("../icons/tray@2x.png");
    let mut reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(io::Error::other)?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| io::Error::other("Invalid status icon dimensions"))?;
    let mut rgba = vec![0; size];
    let frame = reader.next_frame(&mut rgba).map_err(io::Error::other)?;
    if frame.color_type != png::ColorType::Rgba || frame.bit_depth != png::BitDepth::Eight {
        return Err(io::Error::other("The status icon must be an 8-bit RGBA PNG").into());
    }
    Ok(Image::new_owned(rgba, frame.width, frame.height))
}

pub fn set_visible(app: &AppHandle, visible: bool) -> tauri::Result<()> {
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| io::Error::other("Missing status item"))?;
    tray.with_inner_tray_icon(move |tray| {
        let item = tray
            .ns_status_item()
            .ok_or_else(|| io::Error::other("Missing native status item"))?;
        // Use the existing status item so its activity badge survives hiding.
        item.setVisible(visible);
        Ok(())
    })?
}

fn set_activity_dot(tray: &TrayIcon, active: bool) -> tauri::Result<()> {
    tray.with_inner_tray_icon(move |tray| {
        let mtm = MainThreadMarker::new().expect("Status item update must run on the main thread");
        let button = tray
            .ns_status_item()
            .and_then(|item| item.button(mtm))
            .ok_or_else(|| io::Error::other("Missing status item button"))?;
        if let Some(dot) = button
            .subviews()
            .iter()
            .find_map(|view| view.downcast::<ActivityDot>().ok())
        {
            dot.setHidden(!active);
        } else {
            let dot = ActivityDot::new(mtm, button.bounds());
            dot.setHidden(!active);
            button.addSubview(&dot);
        }
        Ok(())
    })?
}

define_class!(
    // SAFETY: The view uses AppKit's main-thread model and draws inside its bounds.
    #[unsafe(super(NSView))]
    #[name = "LazyWorkerActivityDot"]
    struct ActivityDot;

    impl ActivityDot {
        #[unsafe(method(drawRect:))]
        fn draw_dot(&self, _dirty_rect: NSRect) {
            set_badge_fill(self);
            let center = self.bounds().size;
            // The template reserves space right of the glyph, 12pt from its canvas center.
            let origin = NSPoint::new(center.width / 2.0 + 9.0, center.height / 2.0 - 3.0);
            NSBezierPath::bezierPathWithOvalInRect(NSRect::new(origin, NSSize::new(6.0, 6.0)))
                .fill();
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            // SAFETY: Forward AppKit's appearance callback to NSView first.
            let _: () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            // The tray's existing mouse target receives clicks across the full status item.
            std::ptr::null_mut()
        }
    }
);

/// Sets the fill of a badge in the menu bar to the terracotta of the current appearance.
fn set_badge_fill(view: &NSView) {
    // SAFETY: These are constant AppKit appearance names.
    let (light, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let appearances = NSArray::from_slice(&[light, dark]);
    let uses_dark_palette = view
        .effectiveAppearance()
        .bestMatchFromAppearancesWithNames(&appearances)
        .is_some_and(|name| name.isEqualToString(dark));
    let (red, green, blue) = if uses_dark_palette {
        (0xE4, 0x97, 0x84)
    } else {
        (0xA3, 0x4F, 0x44)
    };
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
        1.0,
    )
    .setFill();
}

fn set_unread_dot(tray: &TrayIcon, unread: bool) -> tauri::Result<()> {
    tray.with_inner_tray_icon(move |tray| {
        let mtm = MainThreadMarker::new().expect("Status item update must run on the main thread");
        let button = tray
            .ns_status_item()
            .and_then(|item| item.button(mtm))
            .ok_or_else(|| io::Error::other("Missing status item button"))?;
        if let Some(dot) = button
            .subviews()
            .iter()
            .find_map(|view| view.downcast::<UnreadDot>().ok())
        {
            dot.setHidden(!unread);
        } else {
            let dot = UnreadDot::new(mtm, button.bounds());
            dot.setHidden(!unread);
            button.addSubview(&dot);
        }
        Ok(())
    })?
}

define_class!(
    // SAFETY: The view uses AppKit's main-thread model and draws inside its bounds.
    #[unsafe(super(NSView))]
    #[name = "LazyWorkerUnreadDot"]
    struct UnreadDot;

    impl UnreadDot {
        /// A smaller dot above the activity dot, in the same reserved space right of the glyph.
        #[unsafe(method(drawRect:))]
        fn draw_dot(&self, _dirty_rect: NSRect) {
            set_badge_fill(self);
            let center = self.bounds().size;
            let origin = NSPoint::new(center.width / 2.0 + 9.5, center.height / 2.0 + 4.0);
            NSBezierPath::bezierPathWithOvalInRect(NSRect::new(origin, NSSize::new(5.0, 5.0)))
                .fill();
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            // SAFETY: Forward AppKit's appearance callback to NSView first.
            let _: () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

impl UnreadDot {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        // SAFETY: Initialize the view on the main thread with the status button's bounds.
        let view: Retained<Self> =
            unsafe { msg_send![super(mtm.alloc().set_ivars(())), initWithFrame: frame] };
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view
    }
}

impl ActivityDot {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        // SAFETY: Initialize the view on the main thread with the status button's bounds.
        let view: Retained<Self> =
            unsafe { msg_send![super(mtm.alloc().set_ivars(())), initWithFrame: frame] };
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_says_who_controls_this_device_in_both_languages() {
        let mut state = MenuState::default();
        assert_eq!(state.labels().controlled, "");
        state.controlled_by = vec!["Studio".into()];
        assert_eq!(state.labels().controlled, "Controlled by Studio");
        state.controlled_by.push("Office".into());
        assert_eq!(state.labels().controlled, "Controlled by 2 devices");
        state.spanish = true;
        assert_eq!(state.labels().controlled, "Controlado por 2 dispositivos");
        state.controlled_by.pop();
        assert_eq!(state.labels().controlled, "Controlado por Studio");
    }

    #[test]
    fn the_menu_counts_unread_notifications_in_both_languages() {
        let mut state = MenuState::default();
        assert_eq!(state.labels().notifications, "");
        state.unread = 1;
        assert_eq!(state.labels().notifications, "1 new notification");
        state.unread = 3;
        assert_eq!(state.labels().notifications, "3 new notifications");
        state.spanish = true;
        assert_eq!(state.labels().notifications, "3 notificaciones nuevas");
        state.unread = 1;
        assert_eq!(state.labels().notifications, "1 notificación nueva");
    }

    #[test]
    fn activity_dot_stays_visible_until_both_tools_stop() {
        let mut state = MenuState::default();
        state.activity.failed = true;
        state.can_start_selected = true;
        assert!(!state.has_active_tool());

        state.activity.active = true;
        assert!(state.has_active_tool());

        state.caffeinate.active = true;
        state.activity.active = false;
        assert!(state.has_active_tool());

        state.caffeinate.active = false;
        assert!(!state.has_active_tool());
    }
}
