//! One hot, offline or hardware wallet on the Wallets pane.

use super::*;

/// One hot, offline or hardware wallet on the Wallets pane.
#[component]
pub(in crate::ui) fn WalletRow(wallet: WalletSummary, current: bool) -> Element {
    let session = use_context::<Session>();
    let id = wallet.id;
    let can_sign = wallet.class == WalletClass::Hot;
    let pubkey = wallet.address;
    let tags = wallet.tags.clone();
    let label = wallet.label.clone();
    let show_balances = crate::state::use_network().is_some();
    let lamports = show_balances
        .then(|| pubkey.and_then(|pk| (session.book)().get(&pk).and_then(|e| e.lamports)))
        .flatten();
    let on_device = wallet.on_device;
    let dialogs_of = wallet.clone();
    let mut confirm_open = use_signal(|| false);
    // A phone shortens the address to one line, and puts Send with the
    // card's other buttons.
    let phone = app_ui_kit::ui::use_phone_layout();
    let can_send = show_balances && pubkey.is_some();
    let send_title = "Send SOL or a token from this wallet";
    let send = move |_| {
        session.open_send(SendOpen {
            from: Some(id),
            ..SendOpen::default()
        })
    };

    rsx! {
        li { class: "wallet-row",
            div { class: "wallet-head wallet-card-head",
                AddressHeader {
                    pubkey,
                    title: label.clone(),
                    category: Category::Wallet(wallet.class.into()),
                    subtitle: String::new(),
                    short: phone,
                    title_after: rsx! {
                        if current {
                            CurrentChip {}
                        }
                    },
                    if can_send {
                        WalletBalance { lamports, send_title, on_send: send }
                    }
                }
                div { class: "wallet-end",
                    div { class: "btn-row wallet-side",
                        if phone && can_send {
                            WalletSend { send_title, on_send: send }
                        }
                        if on_device.is_some() {
                            if let Some(address) = pubkey {
                                button { class: "sm",
                                    title: "Show this address on the device, to check it against the one on this screen",
                                    onclick: move |_| crate::actions::device::show_on_device(session, address),
                                    "Show on device"
                                }
                            }
                        }
                        if !can_sign {
                            WalletDialogButtons { wallet: dialogs_of }
                        }
                        WalletRowMenu { id,
                            on_remove: move |_| {
                                session.when_unlocked("Unlock the vault to remove a wallet from it.", move || confirm_open.set(true));
                            },
                        }
                    }
                }
            }
            if !tags.is_empty() {
                div { class: "chip-row",
                    for tag in tags {
                        span { class: "chip", key: "{tag}", "{tag}" }
                    }
                }
            }
        }
        ConfirmRemoveModal {
            open: confirm_open,
            label: label.clone(),
            require_label: can_sign,
            lede: if can_sign {
                "This deletes the signer key from this device. The app does not keep a copy, so the key cannot be recovered after removal. Any programmatic signer or multisig that used it loses that setting.".to_string()
            } else {
                "This removes the wallet from this device. No key is stored here, so none is lost. Its address stays in the address book.".to_string()
            },
            on_confirm: move |_| session.remove_wallet(id).is_ok(),
        }
    }
}
