#![cfg_attr(feature = "bundle", windows_subsystem = "windows")]

//! The online app: the shared shell, with the network, the Stake pane and the
//! network chip, and on desktop the browser bridge with its chip, the tray and
//! OS toasts.
//!
//! What only one build has is in a module of its own, `desktop` or `web`, and
//! both are `shell` here: the same functions, so nothing below asks which
//! build this is.

use app_core::Core;
use app_online::{
    CleanupHost, HistoryPanel, MigrateHost, NetworkChip, PortfolioPanel, ProposalsHost, SendHost,
    StakePanel,
};
use app_ui::{AppBrand, AppShell, Network, NoticeBell, Session, Tab, ToastHost};
use dioxus::prelude::*;

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
#[path = "desktop.rs"]
mod shell;
#[cfg(not(all(feature = "desktop", not(target_arch = "wasm32"))))]
#[path = "web.rs"]
mod shell;

const BRAND: AppBrand = AppBrand {
    name: "Airlock",
    sub: "Solana super wallet",
    mark: asset!("/icons/128x128.png"),
};

/// The online build's navigation.
const TABS: &[Tab] = &[
    Tab::Portfolio,
    Tab::Wallets,
    Tab::Stake,
    Tab::AddressBook,
    Tab::Programs,
    Tab::Build,
    Tab::Sign,
    Tab::History,
    Tab::Settings,
];

fn main() {
    shell::launch(App);
}

/// Install logging, as the first thing each shell does once the environment
/// is as it wants it.
fn init_logging(flavour: &'static str) {
    app_core::diagnostics::init(app_core::diagnostics::LogConfig {
        flavour,
        version: env!("CARGO_PKG_VERSION"),
    });
}

/// The session over `core`, with the network every pane reaches a node through.
fn use_online_session(core: CopyValue<Core>) -> Session {
    let (online, submitter) = app_online::use_online(core);
    let session = app_ui::use_session(core, online, submitter);
    use_context_provider(|| Some(Network::new(session)));
    use_hook(|| session.start_detect());
    session
}

#[component]
fn App() -> Element {
    shell::use_app(use_online_session);

    rsx! {
        AppShell {
            brand: BRAND,
            tabs: TABS,
            stake: rsx! { StakePanel {} },
            portfolio: rsx! { PortfolioPanel {} },
            history: rsx! { HistoryPanel {} },
            top_bar_extra: rsx! { NoticeBell {} },
            sidebar_status: rsx! {
                NetworkChip {}
                {shell::extension_chip()}
            },
            above_panes: shell::bridge_requests(),
            settings_extra: shell::connections_card(),
            window_controls: shell::window_controls(),
            // Outside the pane switch so toasts survive tab changes. Hosts
            // the Proposals, Send, Burn and close and Migrate wallet modals
            // opened from other panes.
            overlays: rsx! {
                ToastHost { pinned: shell::request_toast() }
                ProposalsHost {}
                SendHost {}
                CleanupHost {}
                MigrateHost {}
            },
        }
    }
}
