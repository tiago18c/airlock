//! The two dialogs of a wallet that is not a hot key.
//!
//! Settings holds what can be changed about it (who pays its fees; an
//! offline key's programmatic signer and durable nonce; a
//! multisig's acting member and who approves together with it) and writes
//! nothing until Save. What starts a transaction or adds a wallet (Set up,
//! Create nonce account, a Vector wallet's steps) acts at once.
//!
//! Details shows what a smart contract wallet is: its addresses, its
//! members and threshold, and its settings as they are stored. Nothing in
//! it can be changed.

use super::*;
use crate::actions::wallets::{together_candidates, together_choice, TogetherNote};
use crate::ui::{Glyph, Icon};
use wallet_core::vector::SchemeId;
use wallet_core::AddressKind;

/// A Vector wallet's scheme, its key as shown, where the key that signs for
/// it is, and whether that is with the wallet on this device.
pub(in crate::ui) fn vector_facts(
    wallet: &WalletSummary,
    wallets: &[WalletSummary],
) -> Option<(SchemeId, String, String, bool)> {
    let settings = wallet.contract.as_ref()?.vector_settings()?;
    let held_by = match settings.authority {
        Authority::Wallet { wallet } => wallets
            .iter()
            .find(|other| other.id == *wallet)
            .map(|other| other.label.clone())
            .unwrap_or_else(|| "A wallet no longer on this device".into()),
        Authority::Held { .. } => "Kept with this wallet, on this device".into(),
        Authority::Outside => "On another device".into(),
    };
    Some((
        settings.scheme,
        settings.scheme.show_key(settings.key),
        held_by,
        matches!(settings.authority, Authority::Held { .. }),
    ))
}

/// The label of the wallet that ends up paying when a wallet leaves the
/// choice to `next`, the next along its chain.
fn pays_through(
    session: Session,
    wallets: &[WalletSummary],
    next: Option<WalletId>,
) -> Option<String> {
    let (payer, _) = session.route_of(next?).ok()?.fee_payer?;
    wallets
        .iter()
        .find(|other| other.id == payer)
        .map(|other| other.label.clone())
}

/// What can be changed about a wallet that is not a hot key. The choices
/// are kept here until Save writes them, all at once.
#[component]
pub(in crate::ui) fn WalletSettingsDialog(
    wallet: WalletSummary,
    on_close: EventHandler<()>,
) -> Element {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    let id = wallet.id;
    let pubkey = wallet.address;
    let offline_app = crate::state::use_network().is_none();
    let kind = wallet.contract.clone();
    let is_contract = kind.is_some();
    let is_squads = matches!(kind, Some(WalletKind::Squads { .. }));
    let vector = vector_facts(&wallet, &wallets);
    let is_vector = vector.is_some();
    let authority = kind.as_ref().and_then(WalletKind::signer_authority);
    let executor_nonce = kind.as_ref().and_then(WalletKind::executor_nonce);
    let is_signer = authority.is_some();
    // A durable nonce belongs to the network it was made on. One from
    // before networks were recorded is checked on the chain, online.
    let network = session.cluster().network_key();
    let made_on = wallet.nonce_network.clone();
    let durable_nonce = wallet
        .durable_nonce
        .filter(|_| made_on.as_ref().is_none_or(|made_on| *made_on == network));
    let nonce_elsewhere = wallet
        .durable_nonce
        .and(made_on.clone())
        .filter(|made_on| *made_on != network);
    let mut unplaced_here = use_signal(|| None::<bool>);
    let unplaced = wallet.durable_nonce.filter(|_| made_on.is_none());
    let watched_network = network.clone();
    use_effect(use_reactive!(|(unplaced, pubkey, watched_network)| {
        let _ = &watched_network;
        unplaced_here.set(None);
        let (Some(account), Some(key), false) = (unplaced, pubkey, offline_app) else {
            return;
        };
        spawn(async move {
            unplaced_here.set(crate::actions::wallets::nonce_is_here(session, account, key).await);
        });
    }));
    // Not on this network: as good as none, with Create offered.
    let nonce_missing = unplaced_here() == Some(false);
    let nonce_usable = durable_nonce.is_some() && !nonce_missing;
    let network_words = wallet_core::Cluster::network_words(&network);
    let mut note = use_signal(|| None::<String>);
    let device = wallet.on_device.zip(wallet.derivation_path.clone());

    // What is stored now.
    let stored_payer = match &kind {
        Some(kind) => kind.fee_payer(),
        None => wallet.fee_payer,
    };
    let (stored_member, members, threshold, stored_with, stored_nonce) = match &kind {
        Some(WalletKind::Squads {
            member_wallet,
            members,
            threshold,
            approves_with,
            approval_nonce,
            ..
        }) => (
            *member_wallet,
            members.clone(),
            *threshold,
            approves_with.clone(),
            *approval_nonce,
        ),
        _ => (None, Vec::new(), 0, Vec::new(), None),
    };

    // What Save would write.
    let mut payer = use_signal(|| stored_payer);
    let mut member = use_signal(|| stored_member);
    let mut with = use_signal(|| stored_with.clone());
    let mut approval_nonce = use_signal(|| stored_nonce);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let dirty = payer() != stored_payer
        || member() != stored_member
        || with() != stored_with
        || approval_nonce() != stored_nonce;

    let save = move |_| {
        error.set(None);
        let (payer, member, with, nonce) = (payer(), member(), with(), approval_nonce());
        session.when_unlocked(CHANGE_WALLET, move || {
            let saved = session.update_wallet(id, |kind| {
                kind.set_fee_payer(payer);
                kind.set_member_wallet(member);
                kind.set_approves_with(with);
                kind.set_approval_nonce(nonce);
            });
            match saved {
                Ok(()) => on_close.call(()),
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };

    // An offline key's programmatic signer: the one this key is the authority of.
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
    let signer_label = format!("{} signer", wallet.label);
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
                    crate::actions::network::start_refresh(session);
                    on_close.call(());
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
                let made = if is_contract {
                    crate::actions::wallets::create_nonce(session, id)
                        .await
                        .map(|()| None)
                } else {
                    crate::actions::wallets::create_durable_nonce(session, id).await
                };
                busy.set(false);
                match made {
                    Ok(None) => on_close.call(()),
                    Ok(Some(said)) => note.set(Some(said)),
                    Err(err) => error.set(Some(err)),
                }
            });
        });
    };

    // Who pays when the choice is left to the next wallet along the chain.
    let next_in_chain = match &kind {
        Some(WalletKind::Squads { .. }) => member(),
        Some(WalletKind::Programmatic {
            authority: Authority::Wallet { wallet },
            ..
        }) => Some(*wallet),
        _ => None,
    };
    let none_payer = if is_squads {
        "The member's own setting"
    } else if is_contract {
        "The authority key"
    } else if wallet.on_device.is_some() {
        "The hardware wallet itself"
    } else {
        "The offline wallet itself"
    };
    let none_payer = match pays_through(session, &wallets, next_in_chain) {
        Some(payer) => format!("{none_payer} · {payer}"),
        None => none_payer.to_string(),
    };
    let payer_options: Vec<WalletSummary> = payers(&wallets, offline_app)
        .into_iter()
        .filter(|other| other.id != id)
        .collect();

    // A multisig's approvers, from the choices as they stand.
    let together_options = together_candidates(&wallets, member(), &members);
    let together = together_choice(&wallets, member(), &with(), approval_nonce());
    let nonce_accounts: Vec<_> = (session.book)()
        .iter()
        .filter(|entry| entry.kind == AddressKind::NonceAccount)
        .cloned()
        .collect();
    let mut nonce_options: Vec<Pubkey> = nonce_accounts.iter().map(|entry| entry.pubkey).collect();
    if let Some(kept) = approval_nonce().filter(|kept| !nonce_options.contains(kept)) {
        nonce_options.push(kept);
    }

    rsx! {
        // A click outside closes it, unless that would drop a choice.
        div { class: "modal-backdrop", onclick: move |_| if !dirty { on_close.call(()) },
            div {
                class: "modal wide wallet-dialog-modal",
                role: "dialog",
                aria_modal: "true",
                aria_label: "Settings of {wallet.label}",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack wallet-dialog",
                    h3 { class: "wallet-details-title",
                        "{wallet.label}"
                        span { class: "muted", "Settings" }
                    }
                    div { class: "settings-grid",
                        WalletPicker {
                            label: "Pays fees",
                            options: payer_options,
                            selected: payer(),
                            none_label: none_payer,
                            on_change: move |choice| payer.set(choice),
                        }
                        if is_squads {
                            WalletPicker {
                                label: "Proposes and votes as",
                                options: member_candidates(&wallets),
                                selected: member(),
                                none_label: "Choose a wallet",
                                on_change: move |choice: Option<WalletId>| {
                                    member.set(choice);
                                    with.with_mut(|with| with.retain(|other| Some(*other) != choice));
                                },
                            }
                        }
                        if let Some((vendor, path)) = device {
                            div { class: "setting-row",
                                span { class: "setting-label", "{vendor.label()}" }
                                div { class: "setting-value",
                                    code { class: "mono", "{path}" }
                                }
                            }
                        }
                        if !is_contract {
                            div { class: "setting-row",
                                span { class: "setting-label", "Programmatic signer" }
                                div { class: "setting-value",
                                    match signer.clone() {
                                        Some(signer) => rsx! {
                                            if let Some(address) = signer.address {
                                                Address { pubkey: address }
                                            } else {
                                                span { "{signer.label}" }
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
                            }
                            div { class: "setting-row",
                                span { class: "setting-label", "Durable nonce" }
                                div { class: "setting-value",
                                    match durable_nonce.filter(|_| !nonce_missing) {
                                        Some(account) => rsx! {
                                            Address { pubkey: account }
                                        },
                                        None => rsx! {
                                            button { class: "sm",
                                                title: "Create a nonce account on {network_words}, controlled by this key and paid by the fee payer saved for it. What this key signs for itself there then lives on it.",
                                                disabled: busy(),
                                                onclick: create_nonce,
                                                if busy() { "Preparing…" } else { "Create nonce account" }
                                            }
                                        },
                                    }
                                }
                            }
                        }
                        if is_signer && !is_vector && executor_nonce.is_none() {
                            div { class: "setting-row",
                                span { class: "setting-label", "Nonce account" }
                                div { class: "setting-value",
                                    button { class: "sm",
                                        title: "Create the nonce account this signer needs before it can act",
                                        disabled: busy(),
                                        onclick: create_nonce,
                                        if busy() { "Preparing…" } else { "Create nonce account" }
                                    }
                                }
                            }
                        }
                    }
                    if !is_contract {
                        if let Some(made_on) = nonce_elsewhere.clone() {
                            p { class: "warn",
                                "Its durable nonce is on {wallet_core::Cluster::network_words(&made_on)}, not on {network_words}, so what this key signs here lives on a recent blockhash. Create nonce account makes one for {network_words}."
                            }
                        } else if nonce_missing {
                            p { class: "warn",
                                "Its durable nonce account is not on {network_words}: it was made on another network. Create nonce account makes one here."
                            }
                        }
                    }
                    if let Some(said) = note() {
                        p { class: "note", role: "status", "{said}" }
                    }
                    if !is_contract && nonce_usable {
                        p { class: "hint",
                            "A transaction this key signs for itself lives on the durable nonce, so it stays valid however long the trip to the other device, or the confirmation on it, takes. One transaction at a time: the first to land voids any other made on the same nonce."
                        }
                    }
                    if is_squads && members.is_empty() {
                        p { class: "note", "Members were not read from the chain yet, so the member choice is not checked." }
                    }
                    if is_squads && member().is_some() && !together_options.is_empty() {
                        div { class: "stack approves-together",
                            WalletChecklist {
                                label: "Approves together with",
                                hint: "A ticked wallet votes with the acting member, so a proposal they can finish together is approved and executed in one transaction.",
                                options: together_options,
                                selected: with(),
                                on_change: move |chosen: Vec<WalletId>| with.set(chosen),
                            }
                            if together.members > 1 {
                                p { class: "hint",
                                    "{together.members} members approve together."
                                    if threshold > 0 {
                                        " This multisig needs {threshold}."
                                    }
                                    " A proposal that still needs more than one of their votes can be approved by all of them and executed in one transaction."
                                }
                            }
                            if together.wants_nonce {
                                AddressSelect {
                                    label: "Durable nonce for approving together".to_string(),
                                    options: nonce_options,
                                    selected: approval_nonce(),
                                    none_label: "No durable nonce".to_string(),
                                    on_change: move |picked| approval_nonce.set(picked),
                                }
                                p { class: "hint",
                                    "An offline wallet signs the transaction itself, so a combined approval it votes in uses a durable nonce. Its authority has to be the fee payer or one of the wallets that sign."
                                    if nonce_accounts.is_empty() {
                                        " There is no nonce account in the address book. Create one in Build first."
                                    }
                                }
                            }
                            for note in together.notes.iter().copied() {
                                p { class: "note", key: "{note:?}",
                                    match note {
                                        TogetherNote::NoExecutorNonce => "None of the programmatic signers chosen has a nonce account. One of them needs one before they can approve together.",
                                        TogetherNote::OfflineWithoutNonce => "No durable nonce is picked, so the offline wallets are left out of combined approvals until one is.",
                                        TogetherNote::OfflineBesideProgrammatic => "An offline wallet and a programmatic signer cannot vote in the same transaction, so a proposal that needs both gets no combined approval.",
                                    }
                                }
                            }
                        }
                    }
                    if let (Some((scheme, key, held_by, held_here)), Some(address)) = (vector, pubkey) {
                        h4 { class: "eyebrow", "Account on chain" }
                        VectorFacts { id, address, scheme, shown_key: key, held_by, held_here, state_only: true }
                    }
                    if let Some(err) = error() {
                        p { class: "error", "{err}" }
                    }
                    div { class: "btn-row",
                        if dirty {
                            span { class: "muted wallet-dialog-unsaved", "Not saved yet" }
                        }
                        button { class: "ghost",
                            r#type: "button",
                            title: if dirty { "Close without saving these choices" } else { "" },
                            onclick: move |_| on_close.call(()),
                            "Cancel"
                        }
                        button { class: "primary",
                            r#type: "button",
                            disabled: !dirty,
                            title: if dirty { "Write these choices to the vault" } else { "Nothing has been changed" },
                            onclick: save,
                            "Save"
                        }
                    }
                }
            }
        }
    }
}

/// One fact of the Details dialog: its name and what it is.
#[component]
fn Fact(name: &'static str, children: Element) -> Element {
    rsx! {
        div { class: "setting-row",
            span { class: "setting-label", "{name}" }
            div { class: "setting-value", {children} }
        }
    }
}

/// The heading of a group of facts in the Details dialog, on a phone, where
/// the facts are one long column: what the wallet is and where it lives,
/// who is in it, and how this device uses it. A wider screen has room to
/// show them all at a glance, and no headings.
#[component]
fn FactGroup(name: &'static str) -> Element {
    let phone = app_ui_kit::ui::use_phone_layout();
    rsx! {
        if phone {
            h4 { class: "eyebrow wallet-facts-group", "{name}" }
        }
    }
}

/// A wallet of this device named in the Details dialog. On a phone it is
/// drawn as a wallet row, icon, name, short address and copy, as the
/// members are, so a wallet looks the same wherever the dialog names it.
/// A wider screen names it.
#[component]
fn FactWallet(id: WalletId) -> Element {
    let session = use_context::<Session>();
    let phone = app_ui_kit::ui::use_phone_layout();
    let found = (session.wallets)()
        .summaries
        .iter()
        .find(|other| other.id == id)
        .map(|other| (other.label.clone(), other.address));
    match found {
        Some((_, Some(pubkey))) if phone => rsx! { AddressRow { pubkey } },
        Some((label, _)) => rsx! { span { "{label}" } },
        None => rsx! { span { "A wallet no longer on this device" } },
    }
}

/// What a smart contract wallet is and how it is set up, to read. Nothing
/// here can be changed: Settings does that.
#[component]
pub(in crate::ui) fn WalletDetailsDialog(
    wallet: WalletSummary,
    on_close: EventHandler<()>,
    /// Close this and open the wallet's Settings.
    on_settings: EventHandler<()>,
) -> Element {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    let label_of = |id: WalletId| {
        wallets
            .iter()
            .find(|other| other.id == id)
            .map(|other| other.label.clone())
            .unwrap_or_else(|| "A wallet no longer on this device".to_string())
    };
    let pays = session
        .route_of(wallet.id)
        .ok()
        .and_then(|route| route.fee_payer)
        .map(|(payer, _)| payer);
    let kind = wallet.contract.clone();
    let vector = vector_facts(&wallet, &wallets);
    // The wallet that signs for a Vector account, where it is one of this
    // device's wallets.
    let vector_signer = kind
        .as_ref()
        .and_then(WalletKind::vector_settings)
        .and_then(|settings| match settings.authority {
            Authority::Wallet { wallet } => Some(*wallet),
            Authority::Held { .. } | Authority::Outside => None,
        });
    let authority = kind.as_ref().and_then(WalletKind::signer_authority);
    let executor_nonce = kind.as_ref().and_then(WalletKind::executor_nonce);
    let kind_name = kind.as_ref().and_then(kind_chip);
    // A phone has no room for a name and a whole address on one line: it
    // shortens the address, as the Members rows do, and copy still copies
    // all of it.
    let phone = app_ui_kit::ui::use_phone_layout();

    rsx! {
        div { class: "modal-backdrop", onclick: move |_| on_close.call(()),
            div {
                class: "modal wide wallet-dialog-modal wallet-facts-modal",
                role: "dialog",
                aria_modal: "true",
                aria_label: "Details of {wallet.label}",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack wallet-dialog",
                    h3 { class: "wallet-details-title",
                        "{wallet.label}"
                        span { class: "muted", "Details" }
                    }
                    div { class: "settings-grid wallet-facts",
                        FactGroup { name: "What it is" }
                        if let Some(kind_name) = kind_name {
                            Fact { name: "Kind", span { "{kind_name}" } }
                        }
                        match kind.clone() {
                            Some(WalletKind::Squads {
                                program,
                                multisig,
                                vault,
                                members,
                                threshold,
                                member_wallet,
                                approves_with,
                                approval_nonce,
                                ..
                            }) => rsx! {
                                Fact { name: "Vault", AddressRow { pubkey: vault, full: !phone } }
                                Fact { name: "Multisig account", AddressRow { pubkey: multisig, full: !phone } }
                                Fact { name: "Program", AddressRow { pubkey: program.program_id(), full: !phone } }
                                FactGroup { name: "Who is in it" }
                                Fact { name: "Threshold",
                                    if members.is_empty() {
                                        span { class: "muted", "Not read from the chain yet" }
                                    } else {
                                        span { "{threshold} of {members.len()} members" }
                                    }
                                }
                                if !members.is_empty() {
                                    Fact { name: "Members",
                                        ul { class: "wallet-fact-list",
                                            for member in members {
                                                li { key: "{member}", AddressRow { pubkey: member } }
                                            }
                                        }
                                    }
                                }
                                FactGroup { name: "On this device" }
                                Fact { name: "Proposes and votes as",
                                    match member_wallet {
                                        Some(member) => rsx! { FactWallet { id: member } },
                                        None => rsx! { span { class: "muted", "No wallet chosen" } },
                                    }
                                }
                                Fact { name: "Approves together with",
                                    if approves_with.is_empty() {
                                        span { class: "muted", "No one" }
                                    } else if phone {
                                        ul { class: "wallet-fact-list",
                                            for other in approves_with {
                                                li { key: "{other}", FactWallet { id: other } }
                                            }
                                        }
                                    } else {
                                        span {
                                            {approves_with.iter().map(|other| label_of(*other)).collect::<Vec<_>>().join(", ")}
                                        }
                                    }
                                }
                                if let Some(nonce) = approval_nonce {
                                    Fact { name: "Nonce for approving together", AddressRow { pubkey: nonce } }
                                }
                            },
                            _ => rsx! {
                                if let Some(address) = wallet.address {
                                    Fact { name: "Address", AddressRow { pubkey: address, full: !phone } }
                                }
                                if let Some(authority) = authority {
                                    Fact { name: "Authority key", AddressRow { pubkey: authority, full: !phone } }
                                    Fact { name: "Nonce account",
                                        match executor_nonce {
                                            Some(nonce) => rsx! { AddressRow { pubkey: nonce, full: !phone } },
                                            None => rsx! { span { class: "muted", "None yet" } },
                                        }
                                    }
                                }
                                if let Some((scheme, key, held_by, _)) = vector.clone() {
                                    Fact { name: "Scheme", span { "{scheme.label()}" } }
                                    Fact { name: "Key", code { class: "mono", "{key}" } }
                                    FactGroup { name: "On this device" }
                                    Fact { name: "Signs with",
                                        match vector_signer {
                                            Some(signer) => rsx! { FactWallet { id: signer } },
                                            None => rsx! { span { "{held_by}" } },
                                        }
                                    }
                                } else {
                                    FactGroup { name: "On this device" }
                                }
                            },
                        }
                        Fact { name: "Pays fees",
                            match pays {
                                Some(payer) => rsx! { FactWallet { id: payer } },
                                None => rsx! { span { class: "muted", "Nothing on this device" } },
                            }
                        }
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            r#type: "button",
                            onclick: move |_| on_close.call(()),
                            "Close"
                        }
                        button {
                            r#type: "button",
                            title: "Change who pays its fees and how it signs",
                            onclick: move |_| on_settings.call(()),
                            Icon { glyph: Glyph::Gear }
                            "Change in Settings"
                        }
                    }
                }
            }
        }
    }
}

/// The two icon buttons that open a wallet's dialogs: an "i" in a circle for
/// Details, on a smart contract wallet, and a wheel for Settings. A phone
/// lists both in the card's sheet of actions instead: see [`WalletRowMenu`].
#[component]
pub(in crate::ui) fn WalletDialogButtons(wallet: WalletSummary) -> Element {
    let mut settings_open = use_signal(|| false);
    let mut details_open = use_signal(|| false);
    let has_details = wallet.contract.is_some();
    rsx! {
        if has_details {
            button { class: "icon-btn sm",
                title: "Details: its addresses, members and how it is set up",
                aria_label: "Details of {wallet.label}",
                aria_haspopup: "dialog",
                onclick: move |_| details_open.set(true),
                Icon { glyph: Glyph::Info }
            }
        }
        button { class: "icon-btn sm",
            title: "Settings: who pays its fees, and how it signs",
            aria_label: "Settings of {wallet.label}",
            aria_haspopup: "dialog",
            onclick: move |_| settings_open.set(true),
            Icon { glyph: Glyph::Gear }
        }
        WalletDialogs { wallet: wallet.clone(), settings_open, details_open }
    }
}

/// A wallet's Details and Settings dialogs, each shown while its signal is
/// true, for whatever opens them: [`WalletDialogButtons`], or a card's sheet
/// of actions on a phone. Details hands over to Settings.
#[component]
pub(in crate::ui) fn WalletDialogs(
    wallet: WalletSummary,
    settings_open: Signal<bool>,
    details_open: Signal<bool>,
) -> Element {
    let mut settings_open = settings_open;
    let mut details_open = details_open;
    rsx! {
        if details_open() {
            WalletDetailsDialog {
                wallet: wallet.clone(),
                on_close: move |_| details_open.set(false),
                on_settings: move |_| {
                    details_open.set(false);
                    settings_open.set(true);
                },
            }
        }
        if settings_open() {
            WalletSettingsDialog { wallet: wallet.clone(), on_close: move |_| settings_open.set(false) }
        }
    }
}
