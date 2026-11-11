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
/// wallet shows its kind and address and copies it. The current wallet's
/// address is the one copied most, so the button's end copies it without
/// opening the sheet: one bordered control in two parts. It shows the name
/// and the short address whether the vault is locked or not. Locked, the
/// Locked button beside it takes more of the bar, and the short address is
/// what gives way (the stylesheet cuts it first), never the name or the
/// button's word.
#[component]
fn PhoneWalletSwitcher(wallets: Vec<WalletSummary>, selected: Option<WalletId>) -> Element {
    let session = use_context::<Session>();
    let mut open = use_signal(|| false);
    let current = wallets
        .iter()
        .find(|wallet| Some(wallet.id) == selected)
        .cloned();
    let current_address = current.as_ref().and_then(|wallet| wallet.address);
    rsx! {
        div { class: "wallet-switch-group",
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
            if let Some(address) = current_address {
                CopyButton {
                    text: address.to_string(),
                    label: "Copy wallet address",
                    class: "sm wallet-switch-copy",
                }
            }
        }
        if open() {
            PhoneSheet {
                label: "Wallets",
                title: "Act as",
                class: "wallet-sheet",
                on_close: move |_| open.set(false),
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
                            // inside a button is not a button to a screen
                            // reader. A target of its own, apart from the
                            // row, so a near miss does not switch wallet;
                            // a plain icon, quieter than the row it serves.
                            if let Some(address) = wallet.address {
                                CopyButton {
                                    text: address.to_string(),
                                    label: "Copy wallet address",
                                    class: "ghost wallet-sheet-copy",
                                }
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
        _ => rsx! {
            span { class: "addr-row",
                span { class: "addr-row-main",
                    CategoryIcon { category: Category::Wallet(wallet.class.into()) }
                    span { class: "addr-label", "{wallet.label}" }
                }
            }
        },
    }
}
