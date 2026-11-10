//! Top bar: the wallet the app is acting as.

use super::*;
use crate::ui::{use_phone_layout, CategoryIcon, PhoneSheet};

/// Top bar: the wallet the app is acting as.
#[component]
pub fn WalletSwitcher() -> Element {
    let session = use_context::<Session>();
    let phone = use_phone_layout();
    let wallets = (session.wallets)().summaries.clone();
    if wallets.is_empty() {
        return rsx! {};
    }
    let selected = session.selected_wallet();
    if phone {
        return rsx! { PhoneWalletSwitcher { wallets, selected } };
    }
    let address = wallets
        .iter()
        .find(|wallet| Some(wallet.id) == selected)
        .and_then(|wallet| wallet.address);
    let choices = wallets.clone();
    rsx! {
        label { class: "wallet-switch", title: "The wallet the app is acting as",
            span { class: "sr-only", "Current wallet" }
            select {
                onchange: move |evt| {
                    if let Some(id) = find_id(&choices, &evt.value()) {
                        let _ = session.select_wallet(id);
                    }
                },
                for wallet in wallets {
                    option {
                        key: "{wallet.id}",
                        value: "{wallet.id}",
                        selected: selected == Some(wallet.id),
                        "{wallet.label}"
                    }
                }
            }
        }
        if let Some(address) = address {
            CopyButton { text: address.to_string(), label: "Copy wallet address", class: "sm" }
        }
    }
}

/// On a phone, a `<select>` opens the system's own list, white in a dark app
/// and with names alone. A button opens the app's sheet instead, where each
/// wallet shows its kind and address and copies it, so the top bar needs no
/// copy button of its own.
#[component]
fn PhoneWalletSwitcher(wallets: Vec<WalletSummary>, selected: Option<WalletId>) -> Element {
    let session = use_context::<Session>();
    let mut open = use_signal(|| false);
    let current = wallets
        .iter()
        .find(|wallet| Some(wallet.id) == selected)
        .cloned();
    rsx! {
        button {
            class: "picker-button sm wallet-switch wallet-switch-btn",
            r#type: "button",
            title: "The wallet the app is acting as",
            "aria-haspopup": "dialog",
            "aria-expanded": "{open()}",
            onclick: move |_| open.toggle(),
            span { class: "sr-only", "Current wallet: " }
            match current {
                Some(wallet) => rsx! { WalletLine { wallet } },
                None => rsx! { span { class: "picker-none", "Choose a wallet" } },
            }
        }
        if open() {
            PhoneSheet {
                label: "Wallets",
                class: "wallet-sheet",
                on_close: move |_| open.set(false),
                h2 { class: "phone-sheet-title", "Act as" }
                div { class: "wallet-sheet-list",
                    for wallet in wallets {
                        div {
                            key: "{wallet.id}",
                            class: if selected == Some(wallet.id) { "wallet-sheet-row on" } else { "wallet-sheet-row" },
                            button {
                                class: "wallet-sheet-pick",
                                r#type: "button",
                                "aria-current": if selected == Some(wallet.id) { "true" } else { "false" },
                                onclick: {
                                    let id = wallet.id;
                                    move |_| {
                                        let _ = session.select_wallet(id);
                                        open.set(false);
                                    }
                                },
                                WalletLine { wallet: wallet.clone() }
                                if selected == Some(wallet.id) {
                                    CurrentChip {}
                                }
                            }
                            // Beside the row's button, not in it: a button
                            // inside a button is not a button to a screen reader.
                            if let Some(address) = wallet.address {
                                CopyButton { text: address.to_string(), label: "Copy wallet address" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A wallet as one line: the icon its card on the Wallets pane has, its name,
/// and its address shortened in the middle. A wallet with no address yet,
/// such as a hardware wallet, shows its icon and name.
#[component]
fn WalletLine(wallet: WalletSummary) -> Element {
    match wallet.address {
        Some(pubkey) => rsx! { AddressRow { pubkey, no_copy: true, no_card: true } },
        None => rsx! {
            span { class: "addr-row",
                span { class: "addr-row-main",
                    CategoryIcon { category: Category::Wallet(wallet.class.into()) }
                    span { class: "addr-label", "{wallet.label}" }
                }
            }
        },
    }
}
