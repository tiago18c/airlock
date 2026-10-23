//! System tray: Open, Lock now, Quit. Left click opens (Dioxus default).

use app_core::Core;
use dioxus::desktop::trayicon::menu::{Menu, MenuItem};
use dioxus::desktop::trayicon::{init_tray_icon, MouseButton, TrayIconEvent};
use dioxus::desktop::WindowCloseBehaviour;
use dioxus::prelude::*;

const ID_OPEN: &str = "airlock-open";
const ID_LOCK: &str = "airlock-lock";
const ID_QUIT: &str = "airlock-quit";

/// Install the tray icon, `icon` a PNG, once. Call from a desktop shell's
/// `App` hook.
pub fn install(icon: &'static [u8]) {
    use_hook(|| {
        let open = MenuItem::with_id(ID_OPEN, "Open", true, None);
        let lock = MenuItem::with_id(ID_LOCK, "Lock now", true, None);
        let quit = MenuItem::with_id(ID_QUIT, "Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &lock, &quit])
            .expect("tray menu");
        init_tray_icon(menu, dioxus::desktop::icon_from_memory(icon).ok());
    });
}

/// Wire tray menu and left-click to the core and window.
pub fn use_actions(core: CopyValue<Core>) {
    let window = dioxus::desktop::use_window();

    // Not `use_tray_menu_event_handler`: tray-icon's menu is muda's, whose
    // event handler is a OnceCell. Dioxus sets it for the window menubar
    // first, so tray menu clicks arrive as muda events and the tray-menu
    // hook never fires.
    dioxus::desktop::use_muda_event_handler({
        let window = window.clone();
        move |event| match event.id().0.as_str() {
            ID_OPEN => {
                tracing::debug!(target: "airlock::app", "tray open");
                crate::window::reveal(&window, &core.peek());
            }
            ID_LOCK => {
                core.peek().lock();
            }
            ID_QUIT => {
                tracing::debug!(target: "airlock::app", "tray quit");
                core.peek().flush();
                window.set_close_behavior(WindowCloseBehaviour::WindowCloses);
                window.close();
            }
            _ => {}
        }
    });

    dioxus::desktop::use_tray_icon_event_handler(move |event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            ..
        } = event
        {
            // Dioxus shows and focuses the window; keep LockPolicy in sync.
            core.peek().set_ui_visible(true);
        }
    });
}
