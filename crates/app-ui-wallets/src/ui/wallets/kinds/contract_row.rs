//! One programmatic signer or multisig on the Wallets pane.
//!
//! The card says what the wallet is and what needs attention. What can be
//! changed about it opens from its wheel, and what it is made of from its
//! magnifier: see [`WalletSettingsDialog`] and [`WalletDetailsDialog`].

use super::*;
/// One programmatic signer or multisig on the Wallets pane, with its settings.
#[component]
pub fn ContractWalletRow(wallet: WalletSummary, current: bool) -> Element {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    let id = wallet.id;
    let route = session.route_of(id);
    let show_balances = crate::state::use_network().is_some();
    let offline_app = !show_balances;
    let lamports = show_balances
        .then(|| {
            wallet.address.and_then(|address| {
                (session.book)()
                    .get(&address)
                    .and_then(|entry| entry.lamports)
            })
        })
        .flatten();
    let kind = wallet.contract.as_ref().and_then(kind_chip);
    let mut confirm_remove = use_signal(|| false);
    let squad = wallet.contract.as_ref().and_then(WalletKind::squad);
    let proposals_offered = crate::state::use_network().is_some();
    let proposals_label = wallet.label.clone();

    let is_vector = wallet
        .contract
        .as_ref()
        .and_then(WalletKind::vector_settings)
        .is_some();
    // An Ed25519 signer acts through a nonce account of its own.
    let wants_nonce = !is_vector
        && wallet.contract.as_ref().is_some_and(|kind| {
            kind.signer_authority().is_some() && kind.executor_nonce().is_none()
        });
    let vector = vector_facts(&wallet, &wallets);
    let dialogs_of = wallet.clone();
    rsx! {
        li { class: "wallet-row",
            div { class: "wallet-head wallet-card-head",
                AddressHeader {
                    pubkey: wallet.address,
                    title: wallet.label.clone(),
                    category: Category::Wallet(wallet.class.into()),
                    subtitle: String::new(),
                    title_after: rsx! {
                        if let Some(kind) = kind {
                            span { class: "chip kind", "{kind}" }
                        }
                        if current {
                            CurrentChip {}
                        }
                    },
                    if show_balances && wallet.address.is_some() {
                        crate::ui::wallets::WalletBalance {
                            lamports,
                            send_title: "Send SOL or a token from this wallet. Sign rebuilds the transfer around it.",
                            on_send: move |_| session.open_send(SendOpen {
                                from: Some(id),
                                ..SendOpen::default()
                            }),
                        }
                    }
                }
                div { class: "wallet-end",
                    div { class: "btn-row wallet-side",
                        if proposals_offered {
                            if let Some(squad) = squad {
                                button { class: "sm",
                                    title: "Browse this multisig's proposals, and approve or execute them",
                                    onclick: move |_| {
                                        session.open_proposals(id, proposals_label.clone(), squad);
                                    },
                                    "Proposals"
                                }
                            }
                        }
                        WalletDialogButtons { wallet: dialogs_of }
                        WalletRowMenu { id,
                            on_remove: move |_| {
                                session.when_unlocked("Unlock the vault to remove a wallet from it.", move || confirm_remove.set(true));
                            },
                        }
                    }
                }
            }
            if wants_nonce {
                p { class: "note", "This signer has no nonce account yet. It needs one before it can act: the nonce is what stops a signed message from running twice. Create it in this wallet's settings." }
            }
            match &route {
                Ok(route) if route.fee_payer.is_none() => rsx! {
                    p { class: "warn",
                        if offline_app {
                            "Nothing pays fees for this wallet yet. Pick a hot or offline wallet in its settings."
                        } else {
                            "Nothing on this device can pay fees for this wallet. Pick a hot wallet in its settings."
                        }
                    }
                },
                Ok(_) => rsx! {},
                Err(err) => rsx! { p { class: "warn", "{err}" } },
            }
            // A Vector wallet's account is watched from the card, where only
            // what needs attention shows. The rest is in its settings.
            if let (Some((scheme, key, held_by, held_here)), Some(address)) = (vector, wallet.address) {
                div { class: "wallet-status",
                    VectorFacts { id, address, scheme, shown_key: key, held_by, held_here }
                }
            }
        }
        crate::ui::wallets::ConfirmRemoveModal {
            open: confirm_remove,
            label: wallet.label.clone(),
            require_label: false,
            lede: "This removes the wallet from this device. Nothing on chain changes, and no key is deleted. You can add it again by its address.".to_string(),
            on_confirm: move |_| session.remove_wallet(id).is_ok(),
        }
    }
}

/// What a smart contract wallet is, beside its name: the protocol, and a
/// multisig's threshold. A Vector wallet's scheme is listed with its key.
pub(super) fn kind_chip(kind: &WalletKind) -> Option<String> {
    match kind {
        WalletKind::Squads {
            program,
            members,
            threshold,
            ..
        } => {
            let name = match program {
                SquadsProgram::V4 => "Squads v4",
                SquadsProgram::SmartAccount => "Squads smart account",
            };
            Some(if members.is_empty() {
                name.into()
            } else {
                format!("{name} · {threshold} of {}", members.len())
            })
        }
        WalletKind::Programmatic { .. } if kind.vector_settings().is_some() => {
            Some("Vector".into())
        }
        WalletKind::Programmatic { .. } => Some("Programmatic signer".into()),
        _ => None,
    }
}
