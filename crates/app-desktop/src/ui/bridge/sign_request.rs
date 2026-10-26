//! A site asking for a signature.

use super::prepared::{use_prepared_request, Prepared};

use super::*;

#[component]
pub(in crate::ui) fn SignRequest(current: BridgePrompt, on_close: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let prep = use_prepared_request(session, current.clone());
    let mut attempt = prep.attempt;
    let mut paid_here = prep.paid_here;
    let fetching = prep.fetching;
    let countdown_label = prep.countdown_label;
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let reject_id = current.id.clone();
    let review_id = current.id.clone();
    let sign_id = current.id.clone();
    let origin = current.origin.clone();
    let origin_for_sign = origin.clone();
    // The paid-here offer, when the user switched to it. Logged as a
    // transform only once Review or Sign actually uses it.
    let taken_for_review = prep.offered.clone().filter(|_| prep.replaced_payer);
    let taken_for_sign = taken_for_review.clone();
    let site_signs = prep.site_signs.clone();

    rsx! {
        BlockhashCountdown {
            clock: prep.clock,
            lifetime: prep.lifetime,
            expired: prep.clock_expired,
            label: countdown_label,
        }
        RequestHead { current: current.clone(), title: "Sign a transaction".to_string(), on_close }
        div { class: "request-body",
            if let Some(account) = prep.account {
                div { class: "chip-row",
                    span { class: "muted", "asked of" }
                    Address { pubkey: account }
                }
            }
            if prep.original.is_none() {
                p { class: "warn", "{current.detail}" }
            } else if prep.locked {
                p { class: "warn",
                    "The vault is locked. Unlock it to prepare and sign this request."
                }
                div { class: "btn-row",
                    button {
                        onclick: move |_| {
                            session.when_unlocked("Unlock the vault to prepare and sign this request.", || {});
                        },
                        "Unlock"
                    }
                }
            } else {
                match prep.state {
                    Prepared::Working => rsx! {
                        p { class: "muted", role: "status", "Preparing the transaction…" }
                    },
                    Prepared::Failed(problem) => rsx! {
                        p { class: "error", role: "alert", "{problem}" }
                        p { class: "hint",
                            "This transaction asks a wallet without a key to sign, and it could not be rebuilt for it. It cannot be reviewed or signed as the site sent it."
                        }
                        div { class: "btn-row",
                            button {
                                onclick: move |_| attempt.set(attempt() + 1),
                                "Try again"
                            }
                        }
                    },
                    Prepared::Ready { .. } => rsx! {
                        if let Some(converted) = prep.converted.clone() {
                            ConvertedNotice { converted: converted.clone() }
                            if prep.adapted.is_some() && !converted.replaced.is_empty() {
                                p { class: "hint",
                                    "Review it: the rebuilt transaction is simulated once its authorization is signed, and that shows whether the key made here works for this site."
                                }
                            }
                        }
                        if let Some(adapted) = prep.adapted {
                            AdaptedNotice { adapted }
                        }
                        if let Some(paid) = prep.offered {
                            div { class: "callout",
                                if paid_here() {
                                    p {
                                        Address { pubkey: paid.to }
                                        " pays the fee in place of "
                                        Address { pubkey: paid.from }
                                        ", which is no longer in the transaction and no longer has to sign. The site gets this transaction back, not the one it sent, and may refuse it."
                                    }
                                    div { class: "btn-row",
                                        button {
                                            onclick: move |_| paid_here.set(false),
                                            "Keep the site's fee payer"
                                        }
                                    }
                                } else {
                                    p {
                                        "The site named "
                                        Address { pubkey: paid.from }
                                        " to pay the fee, and its key is not on this device. Simulated with "
                                        Address { pubkey: paid.to }
                                        " paying instead, the transaction would succeed."
                                    }
                                    div { class: "btn-row",
                                        button {
                                            onclick: move |_| paid_here.set(true),
                                            "Pay with this wallet instead"
                                        }
                                    }
                                }
                            }
                        }
                        if let (Some(tx), Some(explained), Some(verdict)) = (prep.shown, prep.explained, prep.verdict) {
                            if fetching() {
                                p { class: "muted", role: "status", "Looking for the IDLs of its programs…" }
                            } else if verdict.cleartext() {
                                h4 { "What it does" }
                                InstructionList { instructions: explained.instructions.clone(), inspect: false }
                                FeeLine { tx: tx.clone() }
                                StageLine {
                                    step: prep.step,
                                    lifetime: prep.lifetime,
                                    expired: prep.modal_expired,
                                    label: countdown_label,
                                }
                                // The summary is only as good as the IDLs it
                                // was read with. One read from the chain was
                                // written by whoever controls the program.
                                if verdict.vetted() {
                                    p { class: "hint",
                                        "Read with IDLs that ship with the app or that you imported."
                                    }
                                } else {
                                    p { class: "warn",
                                        "{verdict.unvetted} instruction(s) above were put in words by an IDL read from the chain, which whoever controls the program wrote. Review the transaction before signing it."
                                    }
                                }
                            } else {
                                p { class: "warn",
                                    "{verdict.unread} instruction(s) could not be put in words with the IDLs on this device, so there is no summary to sign from. Review the transaction to see everything in it."
                                }
                            }
                            if !prep.site_signs.is_empty() {
                                p { class: "hint",
                                    "The site signs for {prep.site_signs.len()} account(s) it creates, such as a new mint, once this device has signed. Its transaction is signed as it sent it."
                                }
                            }
                            if verdict.elsewhere > prep.site_signs.len() {
                                p { class: "hint",
                                    "{verdict.elsewhere - prep.site_signs.len()} signature(s) have to come from other devices. Review collects them."
                                }
                            } else if verdict.signable == 0 {
                                p { class: "hint", "Nothing in this transaction is left for this device to sign." }
                            }
                        } else {
                            p { class: "error", "The transaction could not be read." }
                        }
                    },
                }
            }
            if let Some(text) = error() {
                p { class: "error", role: "alert", "{text}" }
            }
        }
        div { class: "btn-row",
            button {
                onclick: move |_| {
                    session.bridge_answer(&reject_id, BridgeAnswer::Rejected);
                },
                "Reject"
            }
            button {
                class: if prep.can_sign { "" } else { "primary" },
                disabled: !prep.can_review,
                onclick: move |_| {
                    let Some((wire, adapted, _, _, tables, _, flow)) = prep.for_review.clone() else {
                        return;
                    };
                    if let Some(paid) = &taken_for_review {
                        paid.log_taken(session);
                    }
                    let expiry = *prep.clock.expiry.peek();
                    let rebuilt = match (adapted.is_some(), prep.replaced_payer) {
                        (true, true) => "It was rebuilt for the wallet it asks to sign, and a wallet of this device pays its fee in place of the one the site named, so read it again here.",
                        (true, false) => "It was rebuilt for the wallet it asks to sign, so read it again here.",
                        (false, true) => "A wallet of this device pays its fee in place of the one the site named, so read it again here.",
                        (false, false) => "",
                    };
                    let vector = adapted
                        .as_ref()
                        .map(|adapted| adapted.context.clone())
                        .unwrap_or_default();
                    session.open_in_sign_flow(
                        wire,
                        TxSource::Browser {
                            id: review_id.clone(),
                            origin: origin.clone(),
                        },
                        // The request toast names the site. Say only what
                        // changed on the way here.
                        rebuilt.to_string(),
                        expiry,
                        tables,
                        vector,
                        Some(flow),
                    );
                    on_close.call(());
                },
                "Review"
            }
            button {
                class: if prep.can_sign { "primary" } else { "" },
                disabled: !prep.can_sign || busy(),
                title: if prep.can_sign { "" } else { "Sign is offered when the whole transaction reads in words, this device holds every missing signature, and none of them is an authorization. Review signs an authorization first, then the transaction." },
                onclick: move |_| {
                    let Some((wire, tables, known, flow)) = prep.for_sign.clone() else {
                        return;
                    };
                    error.set(None);
                    busy.set(true);
                    match session.sign_locally(&wire, &known) {
                        Ok(done) => {
                            let ctx = wallet_core::relay::AuthContext { vector: &known };
                            let complete = wallet_core::decode_transaction(&done.wire, &tables)
                                .is_ok_and(|tx| {
                                // The site adds what is left for it.
                                tx.signers
                                    .iter()
                                    .all(|slot| slot.valid || site_signs.contains(&slot.pubkey))
                                    && (tx.format != wallet_core::TxFormat::V1
                                        || wallet_core::relay::authority_slots_in(&tx, &ctx)
                                            .iter()
                                            .all(|slot| slot.signed))
                            });
                            if complete {
                                if let Some(paid) = &taken_for_sign {
                                    paid.log_taken(session);
                                }
                                let cluster = session.cluster();
                                let code = crate::bridge::request_code(&sign_id);
                                app_core::diagnostics::TxLog {
                                    flow,
                                    wire: &done.wire,
                                    cluster: Some(&cluster),
                                }
                                .log_signed("hot key");
                                app_core::diagnostics::TxLog {
                                    flow,
                                    wire: &done.wire,
                                    cluster: Some(&cluster),
                                }
                                .log_returned(&origin_for_sign, &code);
                                if !session.bridge_answer(
                                    &sign_id,
                                    BridgeAnswer::Signed(done.wire.clone()),
                                ) {
                                    error.set(Some(
                                        "The site stopped waiting for this request.".into(),
                                    ));
                                }
                            } else {
                                error.set(Some(
                                    "Signing here did not complete the transaction. Review it instead.".into(),
                                ));
                            }
                        }
                        Err(err) => error.set(Some(sentence(err))),
                    }
                    busy.set(false);
                },
                "Sign"
            }
        }
    }
}

/// The next step under the summary. A leaf, because it reads the countdown
/// label that changes every second; the modal above it does not.
#[component]
fn StageLine(
    step: Option<wallet_core::NextSignature>,
    lifetime: Option<wallet_core::TxLifetime>,
    expired: bool,
    label: Signal<String>,
) -> Element {
    let Some(line) = modal_stage_line(step, lifetime, expired, Some(label())) else {
        return rsx! {};
    };
    rsx! {
        p { class: "muted", "{line}" }
    }
}

fn modal_stage_line(
    step: Option<wallet_core::NextSignature>,
    lifetime: Option<wallet_core::TxLifetime>,
    expired: bool,
    countdown: Option<String>,
) -> Option<String> {
    match step? {
        wallet_core::NextSignature::Authorization => {
            Some("Next: sign the authorization (intent).".into())
        }
        wallet_core::NextSignature::Nothing => Some("Nothing left to sign here.".into()),
        wallet_core::NextSignature::Transaction => match lifetime {
            Some(wallet_core::TxLifetime::DurableNonce { .. }) => {
                Some("Next: sign the transaction (durable nonce).".into())
            }
            Some(wallet_core::TxLifetime::Blockhash(_)) if expired => {
                Some("The blockhash has expired. Ask the site to send it again.".into())
            }
            Some(wallet_core::TxLifetime::Blockhash(_)) => {
                let when = countdown.unwrap_or_else(|| "…".into());
                Some(format!(
                    "Next: sign the transaction. Blockhash expires in {when}."
                ))
            }
            _ => Some("Next: sign the transaction.".into()),
        },
    }
}

/// What converting a legacy or v0 request to v1 did.
#[component]
pub(in crate::ui) fn ConvertedNotice(converted: wallet_core::Converted) -> Element {
    let format = match converted.from {
        wallet_core::TxFormat::Legacy => "legacy",
        wallet_core::TxFormat::V0 => "v0",
        wallet_core::TxFormat::V1 => "v1",
    };
    let loaded = converted.loaded;
    let tables = converted.tables;
    let voided = converted.voided;
    let limit = converted.limit_from.describe(converted.cu_limit);
    rsx! {
        div { class: "callout",
            p {
                "The site sent a {format} transaction"
                if tables > 0 {
                    " that loads {loaded} account(s) from {tables} lookup table(s)"
                }
                ". It was rewritten as v1, with every account listed, so the device that signs can read all of it. The blockhash and the price per compute unit are the site's. {limit}"
                if voided > 0 {
                    " {voided} signature(s) the site had added no longer apply."
                }
                " The site gets the v1 transaction back."
            }
            for (old, new) in converted.replaced.iter().copied() {
                p {
                    Address { pubkey: old }
                    " was never used on chain, so it was a one-time key the site made, such as a new mint. "
                    Address { pubkey: new }
                    ", made on this device, takes its place and signs with this device's keys, and the result simulated. The account is created at the new address, not the one the site shows."
                }
            }
        }
    }
}

/// What was done to the site's transaction before it was shown.
#[component]
pub(in crate::ui) fn AdaptedNotice(adapted: Adapted) -> Element {
    rsx! {
        div { class: "callout",
            for (i, step) in adapted.steps.iter().cloned().enumerate() {
                match step {
                    AdaptedStep::Squads { vault, member, index, voters, executor, held_back, .. } => {
                        let wait = held_back.as_ref().map(held_back_note);
                        rsx! {
                            p { key: "{i}",
                                Address { pubkey: vault }
                                " is the vault of a Squads multisig, so no key signs as it. The site's instructions are stored as its transaction {index}. "
                                Address { pubkey: member }
                                " creates the proposal"
                                if voters.is_empty() {
                                    "."
                                } else if voters.len() == 1 && executor.is_some() {
                                    ", approves it and executes it."
                                } else if voters.len() == 1 {
                                    " and approves it."
                                } else {
                                    " and these members approve it:"
                                }
                            }
                            if voters.len() > 1 {
                                p { key: "{i}-voters",
                                    for (v, voter) in voters.iter().cloned().enumerate() {
                                        span { key: "{i}-v-{v}",
                                            if v > 0 { " " }
                                            Address { pubkey: voter }
                                        }
                                    }
                                    if executor.is_some() { " It is executed in the same transaction." }
                                    else { " It is not executed in this transaction." }
                                }
                            }
                            if let Some(executor) = executor {
                                if !voters.contains(&executor) {
                                    p { class: "muted", key: "{i}-exec",
                                        Address { pubkey: executor }
                                        " executes it."
                                    }
                                }
                            }
                            if let Some(wait) = wait {
                                p { class: "muted", key: "{i}-wait", "{wait}" }
                            }
                        }
                    },
                    AdaptedStep::Programmatic { authority, address, .. } => rsx! {
                        p { key: "{i}",
                            Address { pubkey: address }
                            " is a programmatic signer, so no key signs as it. Its authority "
                            Address { pubkey: authority }
                            " signs an authorization for these instructions instead."
                        }
                    },
                    AdaptedStep::Vector { scheme, account, .. } => rsx! {
                        p { key: "{i}",
                            Address { pubkey: account }
                            " is a Vector wallet, so no key signs as it. What it signs runs as that account, and its {scheme.label()} key signs the whole transaction instead."
                        }
                    },
                }
            }
            p {
                Address { pubkey: adapted.fee_payer }
                " pays the fee and signs the transaction. The site gets this transaction back, not the one it sent."
            }
            if adapted.voided > 0 {
                p { class: "warn",
                    "{adapted.voided} signature(s) the site had already added covered the old transaction and no longer apply."
                }
            }
        }
    }
}
