//! The Sign screen: one transaction, from wherever it came, taken from
//! reading to signing to submitting.
//!
//! Build, Wallets and a browser request hand a transaction in through
//! `Session::open_in_sign`. A QR code or pasted text does the same from here.
//! The screen does not edit what it is given, with one exception: a
//! transaction that names a programmatic signer or a Squads vault as an
//! ordinary signer cannot be signed by anyone, so it is rebuilt around
//! that signer's authorization or as a proposal of that multisig
//! (`wallet_core::adapt`). One another screen of this app made is rebuilt
//! as soon as it lands. One from outside (a browser request, or pasted or
//! scanned text) waits for the user to press Rebuild, so a change to it is
//! never a surprise.

use crate::actions::build::edit_on_build;
use crate::actions::idl_check::fetch_missing_idls;
use crate::actions::sentence;
use crate::actions::sign_screen::*;
use crate::state::forms::{ShareTab, SignView, SimState, TxSource};
use crate::state::session::Session;
use crate::state::use_network;
use crate::ui::clock::BlockhashCountdown;
use crate::ui::transfer::receive::QrScanner;
use crate::ui::transfer::send::{PayloadShare, QrCarousel};
use crate::ui::tx_view::{InstructionList, SimulatedDeltas, TxView};
use crate::ui::{Address, AddressRow, CopyButton};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::rc::Rc;
use wallet_core::adapt::{AdaptPlan, AskedWallet};
use wallet_core::protocol::{decode_payload, parse_payload_text, QrObject, Signed};
use wallet_core::relay::{
    authority_signature_objects, authority_slots_in, vector_signature_objects, AuthContext,
};
use wallet_core::simulate::Simulation;
use wallet_core::{
    context_note_for, explain_instructions, missing_catalog_programs, next_step, sha256_bytes,
    signature_objects, signing_status_in, tx_lifetime, DecodedTx, LocalKeys, NextSignature,
    SignerRole, SignerState, SigningStatus, TxFormat,
};
use wallet_flows::sign::wants_fresh_blockhash;

mod banners;
mod inbox;
mod share;
mod simulation;

pub(in crate::ui) use banners::*;
pub(in crate::ui) use inbox::*;
pub(in crate::ui) use share::*;
pub(in crate::ui) use simulation::*;

#[component]
pub fn SignPanel() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let mut view = form.view;
    let online = use_network().is_some();
    let decoded = use_memo(move || {
        let wire = form.wire.read().clone();
        let tables = form.tables.read().clone();
        wire.map(|wire| decode_on_sign(&wire, &tables).map(Rc::new))
    });

    // A simulation in flight when the screen was left never reported back.
    use_hook(|| {
        if *form.simulation.peek() == SimState::Running {
            let mut marked = form.simulated_for;
            marked.set(None);
        }
    });

    // IDLs this device lacks, for any program at any depth, when online.
    let mut fetched_for = use_signal(|| None::<[u8; 32]>);
    let mut fetching = use_signal(|| false);
    use_effect(move || {
        // One fetch at a time. When it stores an IDL the catalog changes and
        // this runs again for whatever is still missing.
        if !online || fetching() {
            return;
        }
        let Some(Ok(tx)) = decoded() else {
            return;
        };
        let digest = sha256_bytes(&tx.message);
        if *fetched_for.peek() == Some(digest) {
            return;
        }
        let missing = missing_catalog_programs(&*session.idls.read(), tx.as_ref());
        if missing.is_empty() {
            return;
        }
        fetched_for.set(Some(digest));
        fetching.set(true);
        let mut note = form.note;
        spawn(async move {
            let report = fetch_missing_idls(&session, &missing).await;
            fetching.set(false);
            let mut extra = Vec::new();
            if !report.imported.is_empty() {
                extra.push(format!("Fetched {}.", report.imported.join(", ")));
            }
            if report.missing > 0 {
                extra.push(format!(
                    "No on-chain IDL for {} program(s).",
                    report.missing
                ));
            }
            if let Some(err) = report.errors.first() {
                extra.push(err.clone());
            }
            if extra.is_empty() {
                return;
            }
            let line = extra.join(" ");
            // The screen can be left and opened again on the same
            // transaction, which asks again. Say it once.
            if note
                .peek()
                .as_ref()
                .is_some_and(|existing| existing.plain().contains(&line))
            {
                return;
            }
            let next = match note.peek().clone() {
                Some(existing) if !existing.0.is_empty() => existing.t(format!(" {line}")),
                _ => line.into(),
            };
            note.set(Some(next));
        });
    });

    // Simulate as soon as there is something to simulate and a node to ask.
    use_effect(move || {
        let mut state = form.simulation;
        let mut marked = form.simulated_for;
        let tx = match decoded() {
            Some(Ok(tx)) if online => Some(tx),
            _ => None,
        };
        let Some(tx) = tx else {
            if *state.peek() != SimState::Idle {
                state.set(SimState::Idle);
            }
            if marked.peek().is_some() {
                marked.set(None);
            }
            return;
        };
        if *marked.peek() == Some(sha256_bytes(&tx.message)) {
            return;
        }
        start_simulation(session, tx.as_ref().clone());
    });

    // A transaction another screen made that asks a keyless wallet to sign
    // is rebuilt for it right away, once per message. One from outside
    // keeps the banner, so the user sees the change before it is made.
    use_effect(move || {
        let Some(Ok(tx)) = decoded() else {
            return;
        };
        let from_app = matches!(
            (form.source)(),
            Some(TxSource::Build | TxSource::Wallets | TxSource::Stake)
        );
        if !online || !from_app {
            return;
        }
        let digest = sha256_bytes(&tx.message);
        if *form.auto_rebuilt_for.peek() == Some(digest) {
            return;
        }
        if !matches!(session.plan_adaptation(tx.as_ref()), Ok(Some(_))) {
            return;
        }
        let mut marked = form.auto_rebuilt_for;
        marked.set(Some(digest));
        rebuild_loaded(session);
    });

    // Whatever the inbox takes in changes the transaction; close it then.
    let mut inbox_open = use_signal(|| false);
    use_effect(move || {
        form.wire.read();
        inbox_open.set(false);
    });

    let wire = form.wire.read().clone();
    let loaded = wire.is_some();
    let error = (form.error)();

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "page-head",
                    h2 { "Review and Sign" }
                    if loaded {
                        div { class: "btn-row",
                            div { class: "segmented", role: "group", aria_label: "How much to show",
                                button {
                                    class: if view() == SignView::Summary { "tab on" } else { "tab" },
                                    onclick: move |_| view.set(SignView::Summary),
                                    "Summary"
                                }
                                button {
                                    class: if view() == SignView::Inspect { "tab on" } else { "tab" },
                                    onclick: move |_| view.set(SignView::Inspect),
                                    "Inspect"
                                }
                            }
                            button { class: "ghost",
                                onclick: move |_| form.clear(),
                                "Clear"
                            }
                        }
                    }
                }

                if let Some(text) = error {
                    p { class: "error", role: "alert", "{text}" }
                }

                match decoded() {
                    None => rsx! {
                        div { class: "card",
                            Inbox { loaded: false }
                        }
                        p { class: "hint",
                            "Build sends its transactions here on its own. So does Review on a browser request."
                        }
                    },
                    Some(Err(err)) => {
                        let problem = sentence(err);
                        let size = wire.as_ref().map(Vec::len).unwrap_or(0);
                        rsx! {
                            p { class: "error", "{problem}" }
                            p { class: "muted",
                                "The payload is {size} bytes and could not be read as a transaction."
                            }
                        }
                    }
                    Some(Ok(tx)) => rsx! {
                        Loaded { tx, inbox_open }
                    },
                }
            }
        }
    }
}

/// Everything shown for a transaction that decoded.
#[component]
fn Loaded(tx: Rc<DecodedTx>, mut inbox_open: Signal<bool>) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let inspect = (form.view)() == SignView::Inspect;
    let known = (form.vector)();
    let vector_here = session.local_vector_accounts();
    let local = LocalKeys {
        keys: &session.local_signer_pubkeys(),
        vector: &vector_here,
    };
    let ctx = AuthContext { vector: &known };
    let status = signing_status_in(tx.as_ref(), &local, &ctx);
    // A programmatic signer or a Squads vault among the signers. No key
    // signs as either, so the transaction has to be rebuilt first.
    let plan = session
        .plan_adaptation(tx.as_ref())
        .map_err(sentence)
        .transpose();
    let needs_rebuild = plan.is_some();
    let relayed = !authority_slots_in(tx.as_ref(), &ctx).is_empty();
    let online = use_network().is_some();
    let busy = use_signal(|| false);
    let mut share_open = use_signal(|| false);
    let source = (form.source)();
    let editable = tx.clone();
    let step = next_step(&status);
    let lifetime = Some(tx_lifetime(tx.as_ref()));
    let clock_expired = use_signal(|| false);
    let countdown_label = use_signal(|| String::from("…"));
    let uses_nonce = tx.uses_durable_nonce;

    // Sign moves a relayed transaction to a fresh blockhash by itself, so an
    // expired one does not hold Sign back. Vector keys are not transaction
    // signers, so wants_fresh_blockhash only needs the hot keys.
    let refreshes_on_sign = online && wants_fresh_blockhash(tx.as_ref(), local.keys);
    let expired =
        clock_expired() && matches!(step, NextSignature::Transaction | NextSignature::Nothing);
    // Signatures over the transaction itself. Refreshing keeps an
    // authorization, so one does not count here.
    let untouched = !tx.signers.iter().any(|slot| slot.valid);
    let from_browser = matches!(source, Some(TxSource::Browser { .. }));
    let app_built = matches!(
        source,
        Some(TxSource::Build | TxSource::Wallets | TxSource::Stake)
    );
    let exact = (form.clock.expiry)().is_some_and(|expiry| expiry.exact);
    // A site's own transaction goes back to it, and it signs for the accounts
    // it creates, such as a new mint, once this device has signed.
    let site_signs = if from_browser && tx.format != TxFormat::V1 {
        let mut here = session.known_wallet_pubkeys();
        here.extend(local.keys.iter().copied());
        let created = session.forms.sign.created_in(tx.as_ref());
        wallet_flows::request::left_for_site(tx.as_ref(), &here, &created).len()
    } else {
        0
    };
    // Before expiry, only a transaction this app made (or relays) is moved:
    // a site's or another device's is left as it came. Once expired, any
    // transaction on a blockhash can be rebuilt on a fresh one.
    let blockhash_gone = clock_expired() && !refreshes_on_sign;
    let show_refresh = online
        && !uses_nonce
        && (blockhash_gone || (untouched && ((relayed && !from_browser) || app_built)));
    let site_blockhash = from_browser && !exact;
    // Every address a signature row draws. The change the simulation finds
    // for one is shown on its row.
    let signer_rows: Vec<Pubkey> = status
        .lines
        .iter()
        .flat_map(|line| match line.role {
            SignerRole::Authority { address } => vec![line.authority_chip(), address],
            _ => vec![line.authority_chip()],
        })
        .collect();

    rsx! {
        BlockhashCountdown {
            clock: form.clock,
            lifetime,
            expired: clock_expired,
            label: countdown_label,
        }

        if let Some(plan) = plan {
            RebuildBanner { plan, online }
        }

        WhatYouAreSigning {
            status: status.clone(),
            tx: tx.clone(),
            online,
            expired: clock_expired,
            label: countdown_label,
        }
        PersonalSignBox { tx: tx.clone(), status: status.clone() }

        SimulatedDeltas {
            TxView { tx: tx.as_ref().clone(), inspect, signers: signer_rows,
                if let Some(source) = source.clone() {
                    SourceChip { source }
                }
            }

            div { class: "sign-status",
                h3 { "Signatures" }
                span {
                    class: if status.complete() { "chip ok" } else { "chip warn" },
                    "{status.signed()} of {status.total()} signed"
                }
            }
            SignerList { status: status.clone(), inspect }
        }

        SimulationStrip { tx: tx.clone(), inspect }

        SignActions {
            status: status.clone(),
            step,
            online,
            busy,
            needs_rebuild,
            expired,
            refreshes_on_sign,
            show_refresh,
            site_blockhash,
            site_signs,
        }

        div { class: "btn-row",
            button {
                onclick: move |_| inbox_open.set(true),
                "Add a signature, or load another transaction"
            }
            button {
                title: if from_browser {
                    "Change it on Build. What Build compiles is a new transaction, and it does not go back to the site."
                } else {
                    "Change it on Build. What Build compiles is a new transaction, signed again."
                },
                onclick: move |_| edit_on_build(&session, &editable),
                "Edit on Build"
            }
            if tx.format == TxFormat::V1 {
                button {
                    onclick: move |_| share_open.set(true),
                    "Share with another device"
                }
            }
        }
        if tx.format != TxFormat::V1 {
            p { class: "hint",
                "Sharing is only for v1 transactions. This one is signed only with hot keys on this device."
            }
        }

        if inbox_open() {
            SignModal { title: "Add a signature, or load another transaction", open: inbox_open,
                Inbox { loaded: true }
                if let Some(text) = (form.error)() {
                    p { class: "error", role: "alert", "{text}" }
                }
            }
        }
        if share_open() && tx.format == TxFormat::V1 {
            SignModal { title: "Share with another device", open: share_open,
                Share { tx: tx.clone() }
            }
        }
    }
}

/// A dialog over the Sign screen, closed by its button or the backdrop.
#[component]
fn SignModal(title: &'static str, mut open: Signal<bool>, children: Element) -> Element {
    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| open.set(false),
            div { class: "modal wide",
                role: "dialog",
                aria_label: title,
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "{title}" }
                    {children}
                    div { class: "btn-row",
                        button { class: "ghost",
                            onclick: move |_| open.set(false),
                            "Close"
                        }
                    }
                }
            }
        }
    }
}

/// The Sign / Submit / Refresh buttons and the hints under them.
#[component]
fn SignActions(
    status: SigningStatus,
    step: NextSignature,
    online: bool,
    busy: Signal<bool>,
    needs_rebuild: bool,
    expired: bool,
    refreshes_on_sign: bool,
    show_refresh: bool,
    site_blockhash: bool,
    site_signs: usize,
) -> Element {
    let session = use_context::<Session>();
    let rebuilding = session.forms.sign.rebuilding;
    let signing_auth = matches!(step, NextSignature::Authorization);
    let can_sign = status.signable > 0
        && !needs_rebuild
        && !busy()
        && (signing_auth || !expired || refreshes_on_sign);
    let can_submit = online && status.complete() && !needs_rebuild && !busy() && !expired;
    let elsewhere = status.missing_elsewhere();
    let authorizations = status.authorizations_elsewhere();
    let sign_label = match step {
        // The authorizations go on a press of their own.
        NextSignature::Authorization => {
            let here = status.authorizations_here();
            if here > 1 {
                format!("Sign authorization ({here})")
            } else {
                "Sign authorization".into()
            }
        }
        NextSignature::Transaction => {
            if status.signable > 1 {
                format!("Sign transaction ({})", status.signable)
            } else {
                "Sign transaction".into()
            }
        }
        NextSignature::Nothing => "Sign".into(),
    };

    rsx! {
        div { class: "btn-row",
            button {
                class: "primary",
                disabled: !can_sign,
                onclick: move |_| sign_loaded(session, busy),
                "{sign_label}"
            }
            button {
                class: if status.complete() { "primary" } else { "" },
                disabled: !can_submit,
                onclick: move |_| submit_loaded(session, busy),
                "Submit"
            }
            if show_refresh {
                RefreshBlockhash { expired }
            }
        }
        if expired && !refreshes_on_sign {
            // With the rebuild button there, it says what to do.
            if !show_refresh {
                p { class: "hint",
                    if site_blockhash {
                        "The site's blockhash has expired. Ask the site to send the transaction again."
                    } else {
                        "The blockhash has expired. Whoever made the transaction has to make it again on a fresh blockhash."
                    }
                }
            }
        } else if needs_rebuild && rebuilding() {
            p { class: "hint", "Rebuilding the transaction for the wallet it asks to sign…" }
        } else if needs_rebuild {
            p { class: "hint", "Rebuild the transaction for the wallet it asks to sign before signing." }
        } else if status.waiting_for_authority {
            p { class: "hint",
                "{authorizations} authorization signature(s) are still needed from other devices. Programmatic authorities sign first; Vector keys sign after those signatures are in. This device then moves the transaction to a fresh blockhash and signs as fee payer."
            }
        } else if elsewhere > 0 && elsewhere == site_signs && status.signable == 0 {
            p { class: "hint",
                "{elsewhere} signature(s) are the site's: it signs for the account(s) it creates, such as a new mint, once it has the transaction back."
            }
        } else if elsewhere > 0 && status.signable == 0 {
            p { class: "hint",
                "{elsewhere} signature(s) are needed from other devices. Share the transaction, then add what they return."
            }
        } else if status.complete() && !online {
            p { class: "hint", "Fully signed. Go online to submit, or share it with a device that is." }
        }
    }
}

#[component]
fn SourceChip(source: TxSource) -> Element {
    match source {
        TxSource::Build => rsx! { span { class: "chip", "from Build" } },
        TxSource::Wallets => rsx! { span { class: "chip", "from Wallets" } },
        TxSource::Stake => rsx! { span { class: "chip", "from Stake" } },
        TxSource::Pasted => rsx! { span { class: "chip", "pasted" } },
        TxSource::Scanned => rsx! { span { class: "chip", "scanned" } },
        TxSource::Browser { origin, .. } => rsx! {
            span { class: "chip kind", title: "{origin}", "requested by {origin}" }
        },
    }
}
