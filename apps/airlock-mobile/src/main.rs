//! Airlock on Android: the online app's shell and panes, laid out for a
//! phone, with a bottom tab bar and the rest of the sections under More.
//!
//! The app runs online or offline, as chosen on the first launch and in
//! Settings. Each mode keeps its own vault in its own folder; offline, no
//! node is asked anything and the online panes are not listed. Switching
//! restarts the app in the other mode.
//!
//! Online, other apps on the phone and sites in Chrome can ask Airlock to
//! connect and sign through Mobile Wallet Adapter, answered in the browser
//! request modal. Offline, Airlock is not listed as a wallet at all.

use app_mobile::Mode;
use app_online::{
    CleanupHost, HistoryPanel, MigrateHost, NetworkAlert, PortfolioPanel, ProposalsHost, SendHost,
    StakePanel,
};
use app_ui::{AppBrand, AppShell, Network, NoticeBell, OnlineHooks, SubmitHandle, Tab, ToastHost};
use dioxus::prelude::*;

const BRAND: AppBrand = AppBrand {
    name: "Airlock",
    sub: "Solana super wallet",
    mark: asset!("/icons/128x128.png"),
};

/// Online: the bottom bar, then what More lists.
const ONLINE_TABS: &[Tab] = &[Tab::Portfolio, Tab::Wallets, Tab::Sign, Tab::History];
/// In the sidebar's order, as the desktop lists them.
const ONLINE_MORE: &[Tab] = &[
    Tab::Stake,
    Tab::AddressBook,
    Tab::Programs,
    Tab::Build,
    Tab::Settings,
];

/// Offline: everything that works without a node, in the sidebar's order:
/// Build before Review and Sign.
const OFFLINE_TABS: &[Tab] = &[Tab::Wallets, Tab::Build, Tab::Sign];
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
    // Fingerprint unlock, where the phone has a strong biometric.
    let guard = use_hook(app_mobile::key_guard::key_guard);
    if let Some(guard) = guard {
        use_context_provider(move || app_ui::state::key_guard::KeyGuardHandle(guard));
    }
    // Only an online Airlock whose user chose so is a wallet other apps
    // find. A first launch is listed once its setup says so.
    use_hook(move || {
        use app_mobile::wallet_adapter;
        let setting_up = first_launch(core);
        wallet_adapter::set_listed(
            mode == Mode::Online && !setting_up && wallet_adapter::allowed(),
        );
    });

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

    // Requests from other apps: the browser bridge's queue and modal, with
    // no socket.
    let bridge = use_context_provider(app_desktop::bridge::Bridge::for_apps);
    use_hook(move || {
        bridge.attach(core.peek().public());
        app_mobile::wallet_adapter::serve(bridge.clone());
    });
    app_desktop::use_request_modal();

    // The first launch asks how the app is to be used before its vault is
    // created.
    let mut setting_up = use_signal(move || first_launch(core));

    rsx! {
        AppShell {
            brand: BRAND,
            tabs: ONLINE_TABS,
            more: ONLINE_MORE,
            stake: rsx! { StakePanel {} },
            portfolio: rsx! { PortfolioPanel {} },
            history: rsx! { HistoryPanel {} },
            top_bar_extra: rsx! { NoticeBell {} },
            // Only while the node does not answer or a sync failed: a
            // phone has no row to spare for "Online".
            sidebar_status: rsx! { NetworkAlert {} },
            above_panes: rsx! {
                app_desktop::BridgeRequests {}
                app_desktop::ConnectionsHost {}
            },
            settings_extra: rsx! {
                app_mobile::ModeCard { mode: Mode::Online }
                app_mobile::AppsCard {}
            },
            overlays: rsx! {
                ToastHost { pinned: rsx! { app_desktop::RequestToast {} } }
                ProposalsHost {}
                SendHost {}
                CleanupHost {}
                MigrateHost {}
            },
            setup: setting_up().then(|| rsx! {
                app_mobile::Setup { on_done: move |_| setting_up.set(false) }
            }),
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

/// The first launch: no mode was ever chosen and there is no vault yet. A
/// vault made before the first launch asked anything counts as chosen.
fn first_launch(core: CopyValue<app_core::Core>) -> bool {
    !app_mobile::mode::chosen()
        && *core.peek().slices().vault.get() == app_core::VaultState::Uninitialized
}
