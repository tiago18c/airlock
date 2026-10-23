//! The desktop window: WebView settings, the frameless window and its title
//! bar on Windows, camera permission, and `ui_visible` reporting for
//! LockPolicy. Both desktop builds share that.
//! Tray and close-to-tray are online-only ([`use_tray`]).

use app_core::Core;
use dioxus::desktop::tao::dpi::LogicalSize;
use dioxus::desktop::tao::event::{Event, WindowEvent};
use dioxus::desktop::WindowCloseBehaviour;
use dioxus::prelude::*;

#[cfg(feature = "bridge")]
use std::sync::Arc;

#[cfg(feature = "bridge")]
use app_core::Settings;
#[cfg(feature = "bridge")]
use dioxus::desktop::DesktopContext;

/// The smallest the window can be made, in logical pixels. Wide enough for
/// the sidebar rail rather than the phone layout's bottom tab bar (below
/// 640 px), with the top bar's tools and the window buttons on one line.
const MIN_WINDOW: LogicalSize<f64> = LogicalSize::new(720.0, 540.0);

/// The window and WebView settings both desktop builds launch with. `icon`
/// is the build's PNG app icon, for the window and the taskbar. On Windows
/// the window has no native frame: the shell draws its own title bar from
/// [`window_controls`].
pub fn desktop_config(title: &str, icon: &[u8]) -> dioxus::desktop::Config {
    // Replaces the whole builder Config::new made, so the title is set again.
    let window = dioxus::desktop::WindowBuilder::new()
        .with_title(title)
        .with_window_icon(dioxus::desktop::icon_from_memory(icon).ok())
        .with_min_inner_size(MIN_WINDOW);
    // Dioxus drops its default menu bar along with the decorations.
    #[cfg(windows)]
    let window = window.with_decorations(false);
    let cfg = dioxus::desktop::Config::new().with_window(window);
    // Dioxus desktop maps dioxus:// to http://dioxus.index.html, which is not a
    // secure context, so navigator.mediaDevices is undefined unless we force it.
    // Setting any arguments replaces the WebView's defaults, so the first
    // line repeats them. The camera prompt is answered by
    // `camera::allow_windows_camera`, not by a blanket media flag.
    #[cfg(windows)]
    let cfg = cfg.with_windows_browser_args(concat!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection ",
        "--unsafely-treat-insecure-origin-as-secure=",
        "http://dioxus.index.html,http://dioxus.localhost ",
        "--autoplay-policy=no-user-gesture-required"
    ));
    cfg
}

/// The title bar buttons for `AppShell { window_controls }`: Windows only,
/// where [`desktop_config`] removes the native frame.
pub fn window_controls() -> Option<Element> {
    #[cfg(windows)]
    return Some(rsx! { crate::titlebar::WindowControls {} });
    #[cfg(not(windows))]
    return None;
}

/// Camera permission and `ui_visible` for LockPolicy. Closing the window
/// quits. Call once from every desktop shell; the online shell also calls
/// [`use_tray`].
pub fn use_desktop_window(core: CopyValue<Core>) {
    let window = dioxus::desktop::use_window();
    use_hook({
        let window = window.clone();
        move || window.set_close_behavior(WindowCloseBehaviour::WindowCloses)
    });
    report_to_core(core);
    #[cfg(windows)]
    {
        use_effect(move || crate::camera::allow_windows_camera(&window));
    }
}

/// System tray and Settings-driven close-to-tray. Online desktop only.
/// `icon` is the PNG the tray shows.
#[cfg(feature = "bridge")]
pub fn use_tray(core: CopyValue<Core>, settings: ReadSignal<Arc<Settings>>, icon: &'static [u8]) {
    crate::tray::install(icon);
    sync_close_behaviour(settings);
    crate::tray::use_actions(core);
}

/// Apply Settings `close_to_tray` to the window's close behaviour, again each
/// time it changes.
#[cfg(feature = "bridge")]
fn sync_close_behaviour(settings: ReadSignal<Arc<Settings>>) {
    let window = dioxus::desktop::use_window();
    use_effect(move || {
        let behaviour = if settings.read().close_to_tray {
            WindowCloseBehaviour::WindowHides
        } else {
            WindowCloseBehaviour::WindowCloses
        };
        window.set_close_behavior(behaviour);
    });
}

/// Bring the window forward on the screen it was on and mark the UI visible.
#[cfg(feature = "bridge")]
pub fn reveal(window: &DesktopContext, core: &Core) {
    window.set_visible(true);
    window.set_focus();
    core.set_ui_visible(true);
}

/// Report visibility / focus / minimize / close-to-tray hide into the core.
fn report_to_core(core: CopyValue<Core>) {
    let window = dioxus::desktop::use_window();

    use_hook({
        let window = window.clone();
        move || {
            let visible = window.is_visible() && !window.is_minimized();
            core.peek().set_ui_visible(visible);
        }
    });

    dioxus::desktop::use_wry_event_handler(move |event, _| {
        let Event::WindowEvent { event, .. } = event else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                let hide = core.peek().slices().settings.get().close_to_tray;
                if hide {
                    tracing::info!(target: "airlock::app", "window hidden");
                    core.peek().set_ui_visible(false);
                }
            }
            WindowEvent::Resized(_) | WindowEvent::Focused(_) | WindowEvent::Moved(_) => {
                let visible = window.is_visible() && !window.is_minimized();
                core.peek().set_ui_visible(visible);
            }
            _ => {}
        }
    });
}
