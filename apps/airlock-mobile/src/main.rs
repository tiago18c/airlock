//! Airlock on Android: the online app's shell and panes, laid out for a
//! phone, with a bottom tab bar and the rest of the sections under More.
//!
//! The app runs online or offline, as chosen in Settings. Each mode keeps
//! its own vault in its own folder; offline, no node is asked anything and
//! the online panes are not listed. Switching restarts the app in the other
//! mode.

use app_mobile::Mode;
use app_online::{HistoryPanel, NetworkChip, PortfolioPanel, ProposalsHost, SendHost, StakePanel};
use app_ui::{AppBrand, AppShell, Network, NoticeBell, OnlineHooks, SubmitHandle, Tab, ToastHost};
use dioxus::prelude::*;

const BRAND: AppBrand = AppBrand {
    name: "Airlock",
    sub: "Solana super wallet",
    mark: asset!("/icons/128x128.png"),
};

/// Online: the bottom bar, then what More lists.
const ONLINE_TABS: &[Tab] = &[Tab::Portfolio, Tab::Wallets, Tab::Sign, Tab::History];
const ONLINE_MORE: &[Tab] = &[
    Tab::Stake,
    Tab::Build,
    Tab::AddressBook,
    Tab::Programs,
    Tab::Settings,
];

/// Offline: everything that works without a node.
const OFFLINE_TABS: &[Tab] = &[Tab::Wallets, Tab::Sign, Tab::Build];
const OFFLINE_MORE: &[Tab] = &[Tab::AddressBook, Tab::Programs, Tab::Settings];

fn main() {
    let mode = app_mobile::prepare();
    app_core::diagnostics::init(app_core::diagnostics::LogConfig {
        flavour: match mode {
            Mode::Online => "android",
            Mode::Offline => "android-offline",
        },
        version: env!("CARGO_PKG_VERSION"),
    });
    tracing::info!(target: "airlock::app", ?mode, "starting");

    dioxus::LaunchBuilder::mobile()
        .with_cfg(app_mobile::mobile_config())
        .with_context(mode)
        .launch(App);
}

#[component]
fn App() -> Element {
    let mode = use_context::<Mode>();
    let core = use_hook(|| {
        app_ui::open_core_on(
            std::sync::Arc::new(app_core::NoNotifier),
            Some(app_mobile::links()),
            app_mobile::seed_vault::hub(),
        )
    });
    let core = use_hook(move || CopyValue::new(core));
    app_mobile::use_lifecycle(core);

    match mode {
        Mode::Online => rsx! { Online { core } },
        Mode::Offline => rsx! { Offline { core } },
    }
}

#[component]
fn Online(core: CopyValue<app_core::Core>) -> Element {
    let (online, submitter) = app_online::use_online(core);
    let session = app_ui::use_session(core, online, submitter);
    use_context_provider(|| Some(Network::new(session)));
    use_hook(|| session.start_detect());

    rsx! {
        AppShell {
            brand: BRAND,
            tabs: ONLINE_TABS,
            more: ONLINE_MORE,
            stake: rsx! { StakePanel {} },
            portfolio: rsx! { PortfolioPanel {} },
            history: rsx! { HistoryPanel {} },
            top_bar_extra: rsx! { NoticeBell {} },
            sidebar_status: rsx! { NetworkChip {} },
            settings_extra: rsx! { app_mobile::ModeCard { mode: Mode::Online } },
            overlays: rsx! {
                ToastHost {}
                ProposalsHost {}
                SendHost {}
            },
        }
    }
}

#[component]
fn Offline(core: CopyValue<app_core::Core>) -> Element {
    // Provides the session context AppShell reads; the value is unused here.
    let _session = app_ui::use_session(core, OnlineHooks::none(), SubmitHandle::none());
    // No Network: every pane shows its offline form, and nothing probes a node.
    use_context_provider(|| None::<Network>);

    rsx! {
        AppShell {
            brand: BRAND,
            tabs: OFFLINE_TABS,
            more: OFFLINE_MORE,
            settings_extra: rsx! { app_mobile::ModeCard { mode: Mode::Offline } },
        }
    }
}
