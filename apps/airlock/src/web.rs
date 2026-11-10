//! What the online app is without the desktop: in a browser, with no
//! bridge, tray, window buttons or OS toasts, so each of those slots is empty.

use app_core::Core;
use app_ui::Session;
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::BRAND;

pub fn launch(app: fn() -> Element) {
    crate::init_logging(if cfg!(target_arch = "wasm32") {
        "web"
    } else {
        "online"
    });

    // In a browser the tab takes the app's data before any store opens,
    // and `WebGate` opens the app once it has.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        dioxus::launch(WebGate);
    }
    #[cfg(not(target_arch = "wasm32"))]
    dioxus::launch(app);
}

/// The web build's counterpart of claiming the data folder: the app opens
/// once this tab holds the app's data, and says why when another tab does.
#[cfg(target_arch = "wasm32")]
#[component]
fn WebGate() -> Element {
    let claim = use_resource(|| async {
        let claim = app_core::storage::claim_web_data().await;
        if let Err(err) = &claim {
            tracing::error!(target: "airlock::app", "{err}");
        }
        claim
    });
    let refused = match &*claim.read() {
        None => return rsx! {},
        Some(Ok(())) => return rsx! { crate::App {} },
        Some(Err(err)) => err.to_string(),
    };
    rsx! {
        document::Stylesheet { href: app_ui::TOKENS_CSS }
        document::Stylesheet { href: app_ui::MAIN_CSS }
        div { class: "content",
            div { class: "empty",
                h2 { "{BRAND.name} cannot open here" }
                p { "{refused}" }
            }
        }
    }
}

/// Open the core, and make the session over it with `session_of`.
pub fn use_app(session_of: impl FnOnce(CopyValue<Core>) -> Session) {
    // dx 0.7.3 hot-reload can append a second App into #main instead of replacing it.
    #[cfg(target_arch = "wasm32")]
    use_effect(keep_single_app_root);

    let core = use_hook(|| app_ui::open_core(hw_transport::hub()));
    let core = use_hook(move || CopyValue::new(core));
    session_of(core);
}

pub fn window_controls() -> Option<Element> {
    None
}

pub fn request_toast() -> Option<Element> {
    None
}

pub fn extension_chip() -> Element {
    rsx! {}
}

pub fn connections_card() -> Element {
    rsx! {}
}

pub fn bridge_requests() -> Element {
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
