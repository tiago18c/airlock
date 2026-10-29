#![cfg_attr(feature = "bundle", windows_subsystem = "windows")]

//! The online app: the shared shell, with the network, the Stake pane and the
//! network chip, and on desktop the browser bridge with its chip, the tray and
//! OS toasts.

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
use app_desktop::bridge::{Bootstrap, Bridge};
use app_online::{NetworkChip, PortfolioPanel, ProposalsHost, SendHost, StakePanel};
use app_ui::{AppBrand, AppShell, Network, NoticeBell, Tab, ToastHost};
use dioxus::prelude::*;

const BRAND: AppBrand = AppBrand {
    name: "Airlock",
    sub: "Solana super wallet",
    mark: asset!("/icons/128x128.png"),
};

/// The window and tray icon.
#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
const ICON: &[u8] = include_bytes!("../icons/128x128.png");

/// The online build's navigation.
const TABS: &[Tab] = &[
    Tab::Portfolio,
    Tab::Wallets,
    Tab::Stake,
    Tab::AddressBook,
    Tab::Programs,
    Tab::Build,
    Tab::Sign,
    Tab::Settings,
];

fn main() {
    app_core::diagnostics::init(app_core::diagnostics::LogConfig {
        flavour: if cfg!(target_arch = "wasm32") {
            "web"
        } else {
            "online"
        },
        version: env!("CARGO_PKG_VERSION"),
    });

    // The bridge socket is claimed before any window opens, so a second
    // launch only asks the running one to show.
    #[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
    {
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
            .launch(App);
    }

    #[cfg(not(all(feature = "desktop", not(target_arch = "wasm32"))))]
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    // dx 0.7.3 hot-reload can append a second App into #main instead of replacing it.
    #[cfg(target_arch = "wasm32")]
    use_effect(keep_single_app_root);

    #[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
    let (core, toast_clicks) = app_desktop::use_online_core();
    #[cfg(not(all(feature = "desktop", not(target_arch = "wasm32"))))]
    let core = use_hook(app_ui::open_core);
    let core = use_hook(move || CopyValue::new(core));

    let (online, submitter) = app_online::use_online(core);
    let session = app_ui::use_session(core, online, submitter);
    use_context_provider(|| Some(Network::new(session)));
    use_hook(|| session.start_detect());

    #[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
    {
        app_desktop::use_desktop_window(core);
        app_desktop::use_tray(core, session.settings, ICON);
        app_desktop::use_online_window(core, session, toast_clicks);
    }

    rsx! {
        AppShell {
            brand: BRAND,
            tabs: TABS,
            stake: rsx! { StakePanel {} },
            portfolio: rsx! { PortfolioPanel {} },
            top_bar_extra: rsx! { NoticeBell {} },
            sidebar_status: rsx! {
                NetworkChip {}
                {extension_chip()}
            },
            above_panes: bridge_requests(),
            window_controls: window_controls(),
            // Outside the pane switch so toasts survive tab changes. Hosts
            // the Proposals and Send modals opened from other panes.
            overlays: rsx! {
                ToastHost { pinned: request_toast() }
                ProposalsHost {}
                SendHost {}
            },
        }
    }
}

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
fn window_controls() -> Option<Element> {
    app_desktop::window_controls()
}

#[cfg(not(all(feature = "desktop", not(target_arch = "wasm32"))))]
fn window_controls() -> Option<Element> {
    None
}

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
fn request_toast() -> Option<Element> {
    Some(rsx! { app_desktop::RequestToast {} })
}

#[cfg(not(all(feature = "desktop", not(target_arch = "wasm32"))))]
fn request_toast() -> Option<Element> {
    None
}

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
fn extension_chip() -> Element {
    rsx! { app_desktop::ExtensionChip {} }
}

#[cfg(not(all(feature = "desktop", not(target_arch = "wasm32"))))]
fn extension_chip() -> Element {
    rsx! {}
}

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
fn bridge_requests() -> Element {
    rsx! { app_desktop::BridgeRequests {} }
}

#[cfg(not(all(feature = "desktop", not(target_arch = "wasm32"))))]
fn bridge_requests() -> Element {
    rsx! {}
}

/// dx 0.7.3 can leave a previous App tree in `#main` after a full rebuild.
#[cfg(target_arch = "wasm32")]
fn keep_single_app_root() {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(main) = document.get_element_by_id("main") else {
        return;
    };
    let Ok(apps) = main.query_selector_all(".app") else {
        return;
    };
    let n = apps.length();
    if n <= 1 {
        return;
    }
    for i in 0..n - 1 {
        if let Some(node) = apps.item(i) {
            let _ = main.remove_child(&node);
        }
    }
}
