//! The browser extension's chip in the sidebar's status bar, beside the
//! network chip: whether an extension is connected to the bridge socket.

use dioxus::prelude::*;

use super::connections::ConnectionsModal;
use crate::bridge::Bridge;

/// "Extension": green while a browser's extension holds a connection to the
/// app, grey otherwise. Hovering or focusing it says how many browsers, or when one connects.
#[component]
pub fn ExtensionChip() -> Element {
    let bridge = use_context::<Bridge>();
    let manage = use_context::<ConnectionsModal>();
    let mut connected = use_signal(|| *bridge.subscribe_extension().borrow());
    use_future(move || {
        let mut extension = bridge.subscribe_extension();
        async move {
            while extension.changed().await.is_ok() {
                let n = *extension.borrow_and_update();
                connected.set(n);
            }
        }
    });

    let n = connected();
    let (class, status) = if n > 0 {
        ("status net-chip online", "Connected")
    } else {
        ("status net-chip idle", "Not connected")
    };
    rsx! {
        span {
            class: "{class}",
            tabindex: "0",
            aria_label: "Extension {status}",
            span { class: "status-dot" }
            span { "Extension" }
            // Not a place to move the window from.
            div { class: "net-card", "data-drag": "false",
                dl {
                    dt { "Status" }
                    dd { "{status}" }
                    if n > 0 {
                        dt { "Browsers" }
                        dd { "{n}" }
                    } else {
                        dt { "Opens" }
                        dd { "When a site or the extension's popup first asks for Airlock" }
                    }
                }
                button { class: "sm",
                    onclick: move |_| manage.show(),
                    "Manage connections"
                }
            }
        }
    }
}
