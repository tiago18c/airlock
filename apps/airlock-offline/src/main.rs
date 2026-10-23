#![cfg_attr(feature = "bundle", windows_subsystem = "windows")]

//! The airgap app: the shared shell with no network. It links no network
//! code, never shows Stake, sync chips, the browser bridge, notifications
//! or a system tray, and closing the window quits.

use app_ui::{AppBrand, AppShell, Network, OnlineHooks, SubmitHandle, Tab};
use dioxus::prelude::*;

/// The airgap build's navigation: everything that works without a node.
const TABS: &[Tab] = &[
    Tab::Wallets,
    Tab::AddressBook,
    Tab::Programs,
    Tab::Build,
    Tab::Sign,
    Tab::Settings,
];

const BRAND: AppBrand = AppBrand {
    name: "Airlock Offline",
    sub: "Offline signing",
    mark: asset!("/icons/128x128.png"),
};

/// The window icon.
#[cfg(feature = "desktop")]
const ICON: &[u8] = include_bytes!("../icons/128x128.png");

fn main() {
    // Its own data folder and variables, so it never opens the online app's
    // vault, and both can run on one machine.
    app_core::storage::set_app_dirs(app_core::storage::AppDirs::OFFLINE);
    app_core::diagnostics::init(app_core::diagnostics::LogConfig {
        flavour: "airgap",
        version: env!("CARGO_PKG_VERSION"),
    });

    #[cfg(feature = "desktop")]
    dioxus::LaunchBuilder::desktop()
        .with_cfg(app_desktop::desktop_config(BRAND.name, ICON))
        .launch(App);

    #[cfg(not(feature = "desktop"))]
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let core = use_hook(app_ui::open_core);
    let core = use_hook(move || CopyValue::new(core));
    // Provides the session context AppShell reads; the value is unused here.
    let _session = app_ui::use_session(core, OnlineHooks::none(), SubmitHandle::none());
    // No Network: every pane shows its offline form, and nothing probes a node.
    use_context_provider(|| None::<Network>);

    #[cfg(feature = "desktop")]
    app_desktop::use_desktop_window(core);

    rsx! {
        AppShell {
            brand: BRAND,
            tabs: TABS,
            window_controls: window_controls(),
        }
    }
}

#[cfg(feature = "desktop")]
fn window_controls() -> Option<Element> {
    app_desktop::window_controls()
}

#[cfg(not(feature = "desktop"))]
fn window_controls() -> Option<Element> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_airgap_build_lists_no_online_pane() {
        assert!(!TABS.contains(&Tab::Stake));
        assert!(!TABS.contains(&Tab::Portfolio));
    }
}
