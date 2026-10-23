//! A dropdown over wallets that shows each one's label and address.

use super::AddressRow;
use crate::state::session::WalletSummary;
use dioxus::prelude::*;
use wallet_core::WalletId;

/// A dropdown over wallets with a first entry for "none". Each wallet shows
/// its label and address, the way the address picker does.
#[component]
pub fn WalletPicker(
    label: String,
    options: Vec<WalletSummary>,
    selected: Option<WalletId>,
    none_label: String,
    on_change: EventHandler<Option<WalletId>>,
) -> Element {
    let mut open = use_signal(|| false);
    // Cards of the entries open beside the list, not over it.
    let beside = super::address::use_card_beside();
    let chosen = options
        .iter()
        .find(|wallet| selected == Some(wallet.id))
        .cloned();
    let item_class = |on: bool| {
        if on {
            "picker-item active"
        } else {
            "picker-item"
        }
    };
    rsx! {
        div { class: if open() { "picker open" } else { "picker" },
            if open() {
                div { class: "picker-backdrop", onclick: move |_| open.set(false) }
            }
            label { "{label}"
                button {
                    r#type: "button",
                    class: "picker-button",
                    "aria-haspopup": "listbox",
                    "aria-expanded": "{open()}",
                    onclick: move |_| open.set(!open()),
                    onkeydown: move |evt| {
                        if evt.key().to_string() == "Escape" {
                            open.set(false);
                        }
                    },
                    match chosen {
                        Some(wallet) => match wallet.address {
                            Some(pubkey) => rsx! { AddressRow { pubkey, no_copy: true } },
                            None => rsx! {
                                span { class: "picker-name",
                                    span { class: "picker-display-main", "{wallet.label}" }
                                }
                            },
                        },
                        None => rsx! {
                            span { class: "picker-none", "{none_label}" }
                        },
                    }
                }
            }
            if open() {
                div { class: "picker-menu", role: "listbox",
                    onmounted: move |e| beside.mounted(e),
                    button {
                        r#type: "button",
                        class: item_class(selected.is_none()),
                        onclick: move |_| {
                            open.set(false);
                            on_change.call(None);
                        },
                        span { class: "picker-label", "{none_label}" }
                    }
                    for wallet in options {
                        button {
                            r#type: "button",
                            key: "{wallet.id}",
                            class: item_class(selected == Some(wallet.id)),
                            onclick: move |_| {
                                open.set(false);
                                on_change.call(Some(wallet.id));
                            },
                            match wallet.address {
                                Some(pubkey) => rsx! { AddressRow { pubkey, no_copy: true } },
                                None => rsx! { span { class: "picker-label", "{wallet.label}" } },
                            }
                        }
                    }
                }
            }
        }
    }
}
