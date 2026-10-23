//! Our own title bar on Windows, where the window has no native frame:
//! minimize, maximize / restore and close, moving the window from the top
//! bar, and a resize grip along the top edge.
//!
//! tao keeps the frame's resize border outside the window on the left,
//! right and bottom, so those edges resize on their own. Along the top the
//! WebView covers the border, so a grip in the page starts the resize.
//!
//! The page tells us what the mouse did through one listener (`LISTEN`):
//! a mouse down on a `data-drag` area moves the window, a double click
//! toggles maximize, a right click opens the system menu, and a resize of
//! the page re-reads the maximized state.

use dioxus::desktop::tao::platform::windows::WindowExtWindows;
use dioxus::desktop::tao::window::ResizeDirection;
use dioxus::desktop::DesktopContext;
use dioxus::prelude::*;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnableMenuItem, GetCursorPos, GetSystemMenu, PostMessageW, TrackPopupMenu, MF_BYCOMMAND,
    MF_ENABLED, MF_GRAYED, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE, SC_SIZE,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_CLOSE, WM_SYSCOMMAND,
};

/// Installed once per page. A remount only swaps the channel it sends to.
/// Controls inside a drag area (buttons, fields, links) keep their clicks,
/// and so does anything under `data-drag="false"`, such as a popover that
/// opens from the top bar.
const LISTEN: &str = r#"
window.__airlockChrome = (message) => dioxus.send(message);
if (!window.__airlockChromeInstalled) {
  window.__airlockChromeInstalled = true;
  const send = (message) => window.__airlockChrome(message);
  const keep = 'button, a, input, select, textarea, label, summary, [role="button"], [contenteditable]';
  const dragArea = (target) =>
    target instanceof Element &&
    target.closest('[data-drag]')?.dataset.drag === 'true' &&
    !target.closest(keep);
  document.addEventListener('mousedown', (e) => {
    if (e.button !== 0 || !(e.target instanceof Element)) return;
    const grip = e.target.closest('[data-resize]');
    if (grip) {
      e.preventDefault();
      send('resize:' + grip.dataset.resize);
    } else if (dragArea(e.target)) {
      e.preventDefault();
      send(e.detail === 2 ? 'toggle' : 'drag');
    }
  });
  document.addEventListener('contextmenu', (e) => {
    if (!dragArea(e.target)) return;
    e.preventDefault();
    send('menu');
  });
  window.addEventListener('resize', () => send('resized'));
  const root = document.documentElement;
  window.addEventListener('blur', () => root.classList.add('window-inactive'));
  window.addEventListener('focus', () => root.classList.remove('window-inactive'));
}
"#;

/// The title bar buttons and the top resize grip. Pass to
/// `AppShell { window_controls }` through [`window_controls`](crate::window_controls).
#[component]
pub fn WindowControls() -> Element {
    let window = dioxus::desktop::use_window();
    let mut maximized = use_signal({
        let window = window.clone();
        move || window.is_maximized()
    });

    use_hook({
        let window = window.clone();
        move || {
            spawn(async move {
                let mut page = document::eval(LISTEN);
                while let Ok(message) = page.recv::<String>().await {
                    match message.as_str() {
                        "drag" => window.drag(),
                        "toggle" => window.toggle_maximized(),
                        "menu" => system_menu(&window),
                        "resized" => {}
                        grip => {
                            if let Some(direction) = grip.strip_prefix("resize:").and_then(edge) {
                                _ = window.drag_resize_window(direction);
                            }
                        }
                    }
                    // Win+Up, a snap or a double click all end up here.
                    let now = window.is_maximized();
                    if *maximized.peek() != now {
                        maximized.set(now);
                    }
                }
            });
        }
    });

    let is_maximized = maximized();
    rsx! {
        if !is_maximized {
            div { class: "resize-grip n", "data-resize": "n", "aria-hidden": "true" }
            div { class: "resize-grip nw", "data-resize": "nw", "aria-hidden": "true" }
            div { class: "resize-grip ne", "data-resize": "ne", "aria-hidden": "true" }
        }
        div { class: "window-controls",
            button {
                r#type: "button",
                title: "Minimize",
                aria_label: "Minimize",
                onclick: {
                    let window = window.clone();
                    move |_| window.set_minimized(true)
                },
                svg { view_box: "0 0 10 10", "aria-hidden": "true",
                    path { d: "M0 5.5h10" }
                }
            }
            button {
                r#type: "button",
                title: if is_maximized { "Restore" } else { "Maximize" },
                aria_label: if is_maximized { "Restore" } else { "Maximize" },
                onclick: {
                    let window = window.clone();
                    move |_| {
                        window.toggle_maximized();
                        maximized.set(window.is_maximized());
                    }
                },
                svg { view_box: "0 0 10 10", "aria-hidden": "true",
                    if is_maximized {
                        path { d: "M.5 2.5h7v7h-7z" }
                        path { d: "M2.5 2.5v-2h7v7h-2" }
                    } else {
                        path { d: "M.5 .5h9v9h-9z" }
                    }
                }
            }
            button {
                class: "close",
                r#type: "button",
                title: "Close",
                aria_label: "Close",
                onclick: move |_| request_close(&window),
                svg { view_box: "0 0 10 10", "aria-hidden": "true",
                    path { d: "M0 0l10 10M10 0 0 10" }
                }
            }
        }
    }
}

fn edge(name: &str) -> Option<ResizeDirection> {
    match name {
        "n" => Some(ResizeDirection::North),
        "nw" => Some(ResizeDirection::NorthWest),
        "ne" => Some(ResizeDirection::NorthEast),
        _ => None,
    }
}

fn hwnd(window: &DesktopContext) -> HWND {
    window.window.hwnd() as HWND
}

/// Close as the native X does, so close-to-tray and quitting behave the same.
fn request_close(window: &DesktopContext) {
    // SAFETY: the HWND belongs to this live window; posting does not block.
    unsafe {
        PostMessageW(hwnd(window), WM_CLOSE, 0, 0);
    }
}

/// The window's system menu at the cursor: Restore, Move, Size, Minimize,
/// Maximize, Close, with the ones that do not apply greyed out.
fn system_menu(window: &DesktopContext) {
    let hwnd = hwnd(window);
    let maximized = window.is_maximized();
    let state = |enabled: bool| MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED };
    // SAFETY: the HWND belongs to this live window, and the menu is the
    // system's own copy for it. TrackPopupMenu runs its loop on this, the
    // window's thread.
    unsafe {
        let menu = GetSystemMenu(hwnd, 0);
        if menu.is_null() {
            return;
        }
        EnableMenuItem(menu, SC_RESTORE, state(maximized));
        EnableMenuItem(menu, SC_MOVE, state(!maximized));
        EnableMenuItem(menu, SC_SIZE, state(!maximized));
        EnableMenuItem(menu, SC_MINIMIZE, state(true));
        EnableMenuItem(menu, SC_MAXIMIZE, state(!maximized));
        EnableMenuItem(menu, SC_CLOSE, state(true));
        let mut at = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut at) == 0 {
            return;
        }
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            at.x,
            at.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        if command != 0 {
            PostMessageW(hwnd, WM_SYSCOMMAND, command as usize, 0);
        }
    }
}
