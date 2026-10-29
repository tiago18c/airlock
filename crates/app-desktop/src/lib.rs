//! Native desktop services: the window chrome both desktop builds share
//! ([`use_desktop_window`]), and for the online build the system tray
//! ([`use_tray`]) plus (with the `bridge` feature) OS toasts, the browser
//! bridge socket and the request modal ([`use_online_core`],
//! [`use_online_window`], [`BridgeRequests`]).
//!
//! The airgap shell links this crate with `desktop` only — no `bridge` — so
//! it gets camera permission and quit-on-close, without tray, toasts or the
//! socket.

#[allow(unsafe_code)] // WebView2's COM interfaces.
mod camera;

#[cfg(all(feature = "bridge", feature = "desktop", not(target_arch = "wasm32")))]
mod tray;

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
mod window;

#[cfg(all(feature = "desktop", windows))]
#[allow(unsafe_code)] // Win32 calls on the window's own HWND.
mod titlebar;

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
#[allow(unsafe_code)] // MessageBoxW, before any window exists.
mod refuse;

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
pub use refuse::refuse_to_start;

// Toasts and the hidden-window notify rule are online-only: they ride with
// `bridge`, same as [`online`], so the airgap build does not compile them.
#[cfg(all(feature = "bridge", feature = "desktop", not(target_arch = "wasm32")))]
mod notifier;

#[cfg(all(feature = "bridge", feature = "desktop", not(target_arch = "wasm32")))]
mod shell;

#[cfg(all(feature = "bridge", not(target_arch = "wasm32")))]
pub mod bridge;

#[cfg(all(feature = "bridge", feature = "desktop", not(target_arch = "wasm32")))]
mod online;

#[cfg(all(feature = "bridge", feature = "desktop", not(target_arch = "wasm32")))]
pub use online::{use_online_core, use_online_window};

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
pub use window::{desktop_config, use_desktop_window, window_controls};

#[cfg(all(feature = "bridge", feature = "desktop", not(target_arch = "wasm32")))]
pub use window::use_tray;

#[cfg(all(feature = "bridge", not(target_arch = "wasm32")))]
mod ui;

#[cfg(all(feature = "bridge", not(target_arch = "wasm32")))]
pub use ui::{use_request_modal, BridgeRequests, ExtensionChip, RequestModal, RequestToast};
