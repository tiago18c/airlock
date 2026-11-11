//! Widgets every screen can use, and the app's other crates too: icons,
//! addresses, pickers, the transaction view, the lock prompt, the clock and
//! the theme toggle. Nothing here reaches into a screen.

pub mod address;
mod address_picker;
pub mod clock;
pub mod device;
mod icons;
pub mod inspect;
pub mod local_time;
pub mod lock;
mod pane_menu;
pub mod phone;
mod theme;
pub mod transfer;
pub mod tx_view;
mod wallet_picker;

pub use address::{
    category_class, category_glyph, Address, AddressCard, AddressHeader, AddressRow, AddressSelect,
    BalanceDeltas, CategoryIcon, CopyAddress, PhraseView,
};
pub use address_picker::AddressPicker;
pub use device::DeviceDialog;
pub use icons::{Glyph, Icon};
pub use inspect::AccountInspector;
pub use lock::{FingerprintOffer, LockScreen, UnlockPrompt, VaultUnlock};
pub use pane_menu::PaneMenu;
pub use phone::{use_phone_layout, ActionSheet, PhoneLayout, PhoneSheet};
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

/// Where an online app shows its network status: the word Offline, in an
/// app that has no network at all. A hollow mark in the text colour, not a
/// status colour, since nothing is wrong. `short` leaves out "never
/// connects", for a phone's top bar.
#[component]
pub fn OfflineMarker(#[props(default)] short: bool) -> Element {
    rsx! {
        span {
            class: "status offline-mark",
            role: "note",
            title: "Offline: this app never connects.",
            aria_label: "Offline: this app never connects.",
            span { class: "status-dot", aria_hidden: "true" }
            span { aria_hidden: "true",
                "Offline"
                if !short {
                    span { class: "net-cluster", " · never connects" }
                }
            }
        }
    }
}

/// Marks the active one of several: the wallet the app acts as, the RPC in
/// use. A check before the word and an outline in the brand's colour, so it
/// is told from a kind chip, which is filled, by more than colour.
#[component]
pub fn SelectedChip(text: &'static str, #[props(default)] title: Option<&'static str>) -> Element {
    rsx! {
        span { class: "chip selected", title,
            Icon { glyph: Glyph::Check }
            "{text}"
        }
    }
}

/// What a list shows while it has nothing: an icon, one sentence saying what
/// will appear there, and an optional action as its children. A dashed card
/// with the three centred; `compact`, a row as tall as one of the list's
/// entries, for a group among others such as the Wallets pane's. `status`
/// announces the sentence, for one that says something is being read.
#[component]
pub fn Empty(
    glyph: Glyph,
    text: String,
    #[props(default)] compact: bool,
    #[props(default)] status: bool,
    children: Element,
) -> Element {
    rsx! {
        div { class: if compact { "empty compact" } else { "empty" },
            Icon { glyph }
            p { role: if status { "status" }, "{text}" }
            {children}
        }
    }
}

/// Every Refresh in the apps: the refresh glyph, then the word on a desktop
/// ("Refreshing…", or `busy_text`, while it runs). On a phone the glyph
/// alone, so the filter and the primary beside it keep the row's room, named
/// "Refresh" for assistive tech and in its tooltip, and the pane says so in a
/// line under the head while it runs. The glyph turns while it runs on both.
/// `labelled` keeps the word on a phone too, for a dialog's row of buttons.
/// `class` adds the button's look (`ghost sm` in a dialog's head).
#[component]
pub fn RefreshButton(
    busy: bool,
    onclick: EventHandler<MouseEvent>,
    #[props(default)] title: Option<&'static str>,
    #[props(default = "Refreshing…")] busy_text: &'static str,
    #[props(default)] labelled: bool,
    #[props(default)] class: &'static str,
) -> Element {
    let state = if busy { " busy" } else { "" };
    if phone::use_phone_layout() && !labelled {
        return rsx! {
            button {
                class: "{class} icon-btn refresh-btn{state}",
                r#type: "button",
                title: title.unwrap_or("Refresh"),
                aria_label: "Refresh",
                aria_busy: "{busy}",
                disabled: busy,
                onclick: move |e| onclick.call(e),
                Icon { glyph: Glyph::Refresh }
            }
        };
    }
    rsx! {
        button {
            class: "{class} refresh-btn{state}",
            r#type: "button",
            title,
            aria_busy: "{busy}",
            disabled: busy,
            onclick: move |e| onclick.call(e),
            Icon { glyph: Glyph::Refresh }
            if busy { "{busy_text}" } else { "Refresh" }
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
