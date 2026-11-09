//! One programmatic signer or multisig on the Wallets pane, with its settings.

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
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut confirm_remove = use_signal(|| false);
    let squad = wallet.contract.as_ref().and_then(WalletKind::squad);
    let proposals_offered = crate::state::use_network().is_some();
    let proposals_label = wallet.label.clone();

    let fee_payer = wallet.contract.as_ref().and_then(WalletKind::fee_payer);
    let (member_wallet, members, nonce, authority) = match &wallet.contract {
        Some(WalletKind::Squads {
            member_wallet,
            members,
            ..
        }) => (*member_wallet, members.clone(), None, None),
        Some(kind @ WalletKind::Programmatic { .. }) => (
            None,
            Vec::new(),
            kind.executor_nonce(),
            kind.signer_authority(),
        ),
        _ => (None, Vec::new(), None, None),
    };
    let is_squads = matches!(wallet.contract, Some(WalletKind::Squads { .. }));
    let is_programmatic = authority.is_some();
    // A Vector wallet's key, and where the key that signs for it is.
    let vector = wallet
        .contract
        .as_ref()
        .and_then(WalletKind::vector_settings)
        .map(|settings| {
            let held_by = match settings.authority {
                Authority::Wallet { wallet } => wallets
                    .iter()
                    .find(|other| other.id == *wallet)
                    .map(|other| other.label.clone())
                    .unwrap_or_else(|| "A wallet no longer on this device".into()),
                Authority::Held { .. } => "Kept with this wallet, on this device".into(),
                Authority::Outside => "On another device".into(),
            };
            (
                settings.scheme,
                settings.scheme.show_key(settings.key),
                held_by,
                matches!(settings.authority, Authority::Held { .. }),
            )
        });
    let create_nonce = move |_| {
        error.set(None);
        session.when_unlocked(CHANGE_WALLET, move || {
            busy.set(true);
            spawn(async move {
                if let Err(err) = crate::actions::wallets::create_nonce(session, id).await {
                    error.set(Some(err));
                }
                busy.set(false);
            });
        });
    };
    rsx! {
        li { class: "wallet-row",
            div { class: "wallet-head",
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
                div { class: "btn-row wallet-side",
                    if !current {
                        button { class: "sm",
                            onclick: move |_| { let _ = session.select_wallet(id); },
                            "Use"
                        }
                    }
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
                    ExportKeyButton { id }
                    button { class: "danger sm",
                        onclick: move |_| {
                            session.when_unlocked("Unlock the vault to remove a wallet from it.", move || confirm_remove.set(true));
                        },
                        "Remove"
                    }
                }
            }
            div { class: "wallet-settings",
                if is_squads {
                    WalletPicker {
                        label: "Acts as member",
                        options: member_candidates(&wallets),
                        selected: member_wallet,
                        none_label: "Choose a wallet",
                        on_change: move |choice| {
                            session.when_unlocked(CHANGE_WALLET, move || {
                                let _ = session.update_wallet(id, |kind| kind.set_member_wallet(choice));
                            });
                        },
                    }
                }
                WalletPicker {
                    label: "Pays fees",
                    options: payers(&wallets, offline_app),
                    selected: fee_payer,
                    none_label: if is_squads { "The member's own setting" } else { "The authority key" },
                    on_change: move |choice| {
                        session.when_unlocked(CHANGE_WALLET, move || {
                            let _ = session.update_wallet(id, |kind| kind.set_fee_payer(choice));
                        });
                    },
                }
            }
            match &route {
                Ok(route) if route.fee_payer.is_none() => rsx! {
                    p { class: "warn",
                        if offline_app {
                            "Nothing pays fees for this wallet yet. Pick a hot or offline wallet above."
                        } else {
                            "Nothing on this device can pay fees for this wallet. Pick a hot wallet above."
                        }
                    }
                },
                Ok(_) => rsx! {},
                Err(err) => rsx! { p { class: "warn", "{err}" } },
            }
            if is_squads && members.is_empty() {
                p { class: "note", "Members were not read from the chain yet, so the member choice is not checked." }
            }
            if is_squads {
                ApprovesTogether { wallet: wallet.clone() }
            }
            if let Some(authority) = authority {
                p { class: "wallet-fact",
                    span { class: "muted", "Authority key" }
                    Address { pubkey: authority }
                }
            }
            if let (Some((scheme, key, held_by, held_here)), Some(address)) = (vector, wallet.address) {
                VectorFacts { id, address, scheme, shown_key: key, held_by, held_here }
            }
            if is_programmatic {
                match nonce {
                    Some(account) => rsx! {
                        p { class: "wallet-fact",
                            span { class: "muted", "Nonce account" }
                            Address { pubkey: account }
                        }
                    },
                    None => rsx! {
                        p { class: "note", "This signer has no nonce account yet. It needs one before it can act: the nonce is what stops a signed message from running twice." }
                        div { class: "btn-row",
                            button { disabled: busy(), onclick: create_nonce,
                                if busy() { "Preparing…" } else { "Create nonce account" }
                            }
                        }
                    },
                }
            }
            if let Some(err) = error() {
                p { class: "error", "{err}" }
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
fn kind_chip(kind: &WalletKind) -> Option<String> {
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
