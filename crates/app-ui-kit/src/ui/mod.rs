//! Widgets every screen can use, and the app's other crates too: icons,
//! addresses, pickers, the transaction view, the lock prompt, the clock and
//! the theme toggle. Nothing here reaches into a screen.

pub mod address;
mod address_picker;
pub mod clock;
mod icons;
pub mod local_time;
pub mod lock;
mod theme;
pub mod transfer;
pub mod tx_view;
mod wallet_picker;

pub use address::{
    category_class, category_glyph, Address, AddressCard, AddressHeader, AddressRow, AddressSelect,
    BalanceDeltas, CategoryIcon, PhraseView,
};
pub use address_picker::AddressPicker;
pub use icons::{Glyph, Icon};
pub use lock::{LockScreen, UnlockPrompt, VaultUnlock};
pub use theme::ThemeToggle;
pub use wallet_picker::WalletPicker;

use dioxus::prelude::*;
pub use wallet_core::{format_sol, format_token_amount};

#[component]
pub fn Accordion(title: String, mut open: Signal<bool>, children: Element) -> Element {
    rsx! {
        details {
            class: "accordion",
            open: open(),
            summary {
                class: "accordion-header",
                onclick: move |e| {
                    e.prevent_default();
                    open.set(!open());
                },
                span { class: "accordion-title", "{title}" }
                span { class: "accordion-chevron", aria_hidden: "true" }
            }
            if open() {
                div { class: "accordion-body", {children} }
            }
        }
    }
}

/// A question mark that explains the heading it sits beside, on hover, focus
/// or tap.
#[component]
pub fn HelpTip(text: String) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "help tip",
            "aria-label": "{text}",
            "data-tip": "{text}",
            "?"
        }
    }
}

/// Copies `text` to the clipboard and says so for a moment. An icon; `label`
/// is its tooltip and its accessible name.
#[component]
pub fn CopyButton(
    text: String,
    #[props(default = "Copy to clipboard")] label: &'static str,
    #[props(default = "ghost sm")] class: &'static str,
) -> Element {
    let mut copied = use_signal(|| false);
    let tip = if copied() { "Copied" } else { label };
    rsx! {
        button { class: "{class} icon-btn tip tip-end copy-btn",
            r#type: "button",
            "data-tip": "{tip}",
            aria_label: "{tip}",
            onclick: move |e| {
                // A copy button inside a row must not also pick the row.
                e.stop_propagation();
                let text = text.clone();
                copied.set(true);
                spawn(async move {
                    let _ = crate::platform::copy_text(&text).await;
                    crate::rt::sleep(std::time::Duration::from_millis(1500)).await;
                    copied.set(false);
                });
            },
            Icon { glyph: if copied() { Glyph::Check } else { Glyph::Copy } }
        }
    }
}

/// The first four bytes of a checksum, as shown beside a program.
pub fn short_checksum(sha256: &[u8; 32]) -> String {
    hex::encode(&sha256[..4])
}
