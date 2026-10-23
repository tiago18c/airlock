//! One hot or offline wallet on the Wallets pane.

use super::*;

/// One hot or offline wallet on the Wallets pane.
#[component]
pub(in crate::ui) fn WalletRow(wallet: WalletSummary, current: bool) -> Element {
    let session = use_context::<Session>();
    let id = wallet.id;
    let can_sign = wallet.class == WalletClass::Hot;
    let pubkey = wallet.address;
    let tags = wallet.tags;
    let label = wallet.label;
    let fee_payer = wallet.fee_payer;
    let show_balances = crate::state::use_network().is_some();
    let lamports = show_balances
        .then(|| pubkey.and_then(|pk| (session.book)().get(&pk).and_then(|e| e.lamports)))
        .flatten();
    let durable_nonce = wallet.durable_nonce;
    let default_path = wallet.default_path;
    let mut confirm_open = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);

    let wallets = (session.wallets)().summaries.clone();
    // The programmatic signer this key is the authority of.
    let signer = wallets
        .iter()
        .find(|other| {
            let authority = other
                .contract
                .as_ref()
                .and_then(WalletKind::signer_authority);
            authority.is_some() && authority == pubkey
        })
        .cloned();
    let signer_label = format!("{label} signer");
    let add_signer = move |_| {
        error.set(None);
        let Some(authority) = pubkey else {
            return;
        };
        let signer = WalletRecord::programmatic(signer_label.clone(), authority, Some(id));
        session.when_unlocked(
            "Unlock the vault to add the programmatic signer.",
            move || match session.add_contract_wallet(signer) {
                Ok(()) => {
                    let _ = session.update_wallet(id, |kind| {
                        kind.default_path_unless_chosen(OfflinePath::Programmatic)
                    });
                    crate::actions::network::start_refresh(session);
                }
                Err(err) => error.set(Some(err.to_string())),
            },
        );
    };
    let create_nonce = move |_| {
        error.set(None);
        session.when_unlocked(CHANGE_WALLET, move || {
            busy.set(true);
            spawn(async move {
                if let Err(err) = crate::actions::wallets::create_durable_nonce(session, id).await {
                    error.set(Some(err));
                }
                busy.set(false);
            });
        });
    };
    let make_default = move |path: OfflinePath| {
        session.when_unlocked(CHANGE_WALLET, move || {
            let _ = session.update_wallet(id, |kind| kind.set_default_path(Some(path)));
        });
    };
    // A default whose target is gone is not taken.
    let lost_default = match default_path {
        Some(OfflinePath::Programmatic) => signer.is_none(),
        Some(OfflinePath::DurableNonce) => durable_nonce.is_none(),
        None => false,
    };
    // Use follows the default path: the signer is a wallet of its own.
    let acting = session.acting_wallet(id);
    let use_title = if acting == id {
        "Act as this key"
    } else {
        "Act as this key's programmatic signer, its default path. The signer is a wallet of its own, at another address."
    };

    rsx! {
        li { class: "wallet-row",
            div { class: "wallet-head",
                AddressHeader {
                    pubkey,
                    title: label.clone(),
                    category: Category::Wallet(wallet.class.into()),
                    subtitle: String::new(),
                    title_after: rsx! {
                        if current {
                            CurrentChip {}
                        }
                    },
                    if show_balances && pubkey.is_some() {
                        WalletBalance {
                            lamports,
                            send_title: "Send SOL or a token from this wallet",
                            on_send: move |_| session.open_send(SendOpen {
                                from: Some(id),
                                ..SendOpen::default()
                            }),
                        }
                    }
                }
                div { class: "btn-row wallet-side",
                    if !current {
                        button { class: "sm",
                            title: use_title,
                            onclick: move |_| { let _ = session.select_wallet(acting); },
                            "Use"
                        }
                    }
                    ExportKeyButton { id }
                    button { class: "danger sm",
                        onclick: move |_| {
                            session.when_unlocked("Unlock the vault to remove a wallet from it.", move || confirm_open.set(true));
                        },
                        "Remove"
                    }
                }
            }
            if !can_sign {
                div { class: "wallet-settings",
                    WalletPicker {
                        label: "Pays fees",
                        options: payers(&wallets, !show_balances)
                            .into_iter()
                            .filter(|other| other.id != id)
                            .collect::<Vec<_>>(),
                        selected: fee_payer,
                        none_label: "The offline wallet itself",
                        on_change: move |choice| {
                            session.when_unlocked(CHANGE_WALLET, move || {
                                let _ = session.update_wallet(id, |kind| kind.set_fee_payer(choice));
                            });
                        },
                    }
                }
                p { class: "wallet-fact",
                    span { class: "muted", "Programmatic signer" }
                    match signer {
                        Some(signer) => rsx! {
                            if let Some(address) = signer.address {
                                Address { pubkey: address }
                            } else {
                                span { "{signer.label}" }
                            }
                            PathDefault {
                                path: OfflinePath::Programmatic,
                                chosen: default_path,
                                on_choose: make_default,
                            }
                        },
                        None => rsx! {
                            button { class: "sm",
                                title: "Add the programmatic signer this key is the authority of. It is listed under Smart contract wallets.",
                                onclick: add_signer,
                                "Set up"
                            }
                        },
                    }
                }
                p { class: "wallet-fact",
                    span { class: "muted", "Durable nonce" }
                    match durable_nonce {
                        Some(account) => rsx! {
                            Address { pubkey: account }
                            PathDefault {
                                path: OfflinePath::DurableNonce,
                                chosen: default_path,
                                on_choose: make_default,
                            }
                        },
                        None => rsx! {
                            button { class: "sm",
                                title: "Create a nonce account controlled by this key, paid by the fee payer above. What this key signs for itself then lives on it.",
                                disabled: busy(),
                                onclick: create_nonce,
                                if busy() { "Preparing…" } else { "Create nonce account" }
                            }
                        },
                    }
                }
                if durable_nonce.is_some() {
                    p { class: "hint",
                        "A transaction this key signs for itself lives on the durable nonce, so it stays valid however long the trip to the offline device takes. One transaction at a time: the first to land voids any other made on the same nonce."
                    }
                }
                if lost_default {
                    p { class: "warn",
                        "The default path is no longer set up, so this key acts as itself, on a fresh blockhash."
                    }
                }
                if let Some(err) = error() {
                    p { class: "error", "{err}" }
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

/// Marks the path an offline wallet takes by default, or offers to make it so.
#[component]
fn PathDefault(
    path: OfflinePath,
    chosen: Option<OfflinePath>,
    on_choose: EventHandler<OfflinePath>,
) -> Element {
    if chosen == Some(path) {
        return rsx! {
            span { class: "chip ok", title: "What this key acts through when it is chosen", "Default" }
        };
    }
    rsx! {
        button { class: "sm ghost",
            title: "Act through this path when this key is chosen",
            onclick: move |_| on_choose.call(path),
            "Make default"
        }
    }
}
