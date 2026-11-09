//! What the online app has only on the desktop: the browser bridge and its
//! chip and modals, the tray, the window's own buttons and OS toasts.

use app_core::Core;
use app_desktop::bridge::{Bootstrap, Bridge};
use app_ui::Session;
use dioxus::prelude::*;

use crate::BRAND;

/// The window and tray icon.
const ICON: &[u8] = include_bytes!("../icons/128x128.png");

pub fn launch(app: fn() -> Element) {
    // Before anything else, while this is the only thread.
    let dropped = app_desktop::drop_webview_overrides();
    crate::init_logging("online");
    app_desktop::log_dropped_webview_overrides(&dropped);

    // The bridge socket is claimed before any window opens, so a second
    // launch only asks the running one to show.
    let bridge = match Bridge::bootstrap() {
        Bootstrap::AlreadyRunning => {
            tracing::info!(target: "airlock::app", "already running, asked it to show");
            return;
        }
        Bootstrap::Primary(bridge) => bridge,
    };
    // The bridge socket says whether this app is running, but its data
    // folder is what has to be one process's: claim it before any store
    // opens.
    if let Err(err) = app_core::storage::claim_data_dir() {
        app_desktop::refuse_to_start(BRAND.name, &err.to_string());
        return;
    }
    dioxus::LaunchBuilder::desktop()
        .with_cfg(app_desktop::desktop_config(BRAND.name, ICON))
        .with_context(bridge)
        .launch(app);
}

/// Open the core with OS toasts and the bridge, make the session over it
/// with `session_of`, and hand both to the window, the tray and the toasts.
pub fn use_app(session_of: impl FnOnce(CopyValue<Core>) -> Session) {
    let (core, toast_clicks) = app_desktop::use_online_core(hw_transport::hub());
    let core = use_hook(move || CopyValue::new(core));
    let session = session_of(core);
    app_desktop::use_desktop_window(core);
    app_desktop::use_tray(core, session.settings, ICON);
    app_desktop::use_online_window(core, session, toast_clicks);
}

pub fn window_controls() -> Option<Element> {
    app_desktop::window_controls()
}

pub fn request_toast() -> Option<Element> {
    Some(rsx! { app_desktop::RequestToast {} })
}

pub fn extension_chip() -> Element {
    rsx! { app_desktop::ExtensionChip {} }
}

pub fn connections_card() -> Element {
    rsx! { app_desktop::ConnectionsCard {} }
}

pub fn bridge_requests() -> Element {
    rsx! {
        app_desktop::BridgeRequests {}
        app_desktop::ConnectionsHost {}
    }
}
