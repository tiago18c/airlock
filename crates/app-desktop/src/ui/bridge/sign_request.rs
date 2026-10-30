//! A site asking for a signature: one transaction, or a batch of several
//! that goes back whole.

use super::prepared::{use_prepared_request, Prepared, PreparedRequest, ReadyWire, SignWire};
use app_ui_kit::state::forms::{BatchItem, BlockhashClock, BrowserBatch};

use super::*;

#[component]
pub(in crate::ui) fn SignRequest(current: BridgePrompt, on_close: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let prep = use_prepared_request(session, current.clone(), 0);
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
    let for_review = prep.for_review.clone();
    let for_sign = prep.for_sign.clone();
    let clock = prep.clock;
    let replaced_payer = prep.replaced_payer;

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
                LockedNotice {}
            } else {
                {prepared_body(&prep)}
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
                    let Some((wire, adapted, _, _, tables, _, flow)) = for_review.clone() else {
                        return;
                    };
                    if let Some(paid) = &taken_for_review {
                        paid.log_taken(session);
                    }
                    let expiry = *clock.expiry.peek();
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
                        review_note(adapted.is_some(), replaced_payer).to_string(),
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
                class: sign_class(prep.can_sign, prep.unread_sign),
                disabled: !prep.can_sign || busy(),
                title: if prep.unread_sign { UNREAD_SIGN } else if prep.can_sign { "" } else { "Sign is offered when this device holds every missing signature and none of them is an authorization. Review signs an authorization first, then the transaction." },
                onclick: move |_| {
                    let Some(sign) = for_sign.clone() else {
                        return;
                    };
                    error.set(None);
                    busy.set(true);
                    match sign_for_site(session, &sign, &site_signs) {
                        Ok(wire) => {
                            if let Some(paid) = &taken_for_sign {
                                paid.log_taken(session);
                            }
                            log_returned(session, sign.3, &wire, &origin_for_sign, &sign_id);
                            if !session.bridge_answer(&sign_id, BridgeAnswer::Signed(vec![wire])) {
                                error.set(Some("The site stopped waiting for this request.".into()));
                            }
                        }
                        Err(err) => error.set(Some(err)),
                    }
                    busy.set(false);
                },
                "Sign"
            }
        }
    }
}

/// Why Sign is amber: it signs more than the modal could put in words.
const UNREAD_SIGN: &str = "Not every instruction was put in words by an IDL that ships with the app or that you imported. Sign signs it anyway. Review shows everything in it.";

/// The Sign button's look: amber when it signs without a trusted summary.
fn sign_class(can_sign: bool, unread: bool) -> &'static str {
    match (can_sign, unread) {
        (true, true) => "warn solid",
        (true, false) => "primary",
        (false, _) => "",
    }
}

/// What the Sign screen says about a transaction Review hands it: only what
/// changed on the way there.
fn review_note(rebuilt: bool, replaced_payer: bool) -> &'static str {
    match (rebuilt, replaced_payer) {
        (true, true) => "It was rebuilt for the wallet it asks to sign, and a wallet of this device pays its fee in place of the one the site named, so read it again here.",
        (true, false) => "It was rebuilt for the wallet it asks to sign, so read it again here.",
        (false, true) => "A wallet of this device pays its fee in place of the one the site named, so read it again here.",
        (false, false) => "",
    }
}

/// Sign a prepared transaction with this device's keys. Answers the signed
/// wire only when nothing but the site's own signatures is left in it.
fn sign_for_site(
    session: Session,
    (wire, tables, known, flow): &SignWire,
    site_signs: &[Pubkey],
) -> Result<Vec<u8>, String> {
    let done = session.sign_locally(wire, known).map_err(sentence)?;
    let ctx = wallet_core::relay::AuthContext { vector: known };
    let complete = wallet_core::decode_transaction(&done.wire, tables).is_ok_and(|tx| {
        // The site adds what is left for it.
        tx.signers
            .iter()
            .all(|slot| slot.valid || site_signs.contains(&slot.pubkey))
            && (tx.format != wallet_core::TxFormat::V1
                || wallet_core::relay::authority_slots_in(&tx, &ctx)
                    .iter()
                    .all(|slot| slot.signed))
    });
    if !complete {
        return Err("Signing here did not complete the transaction. Review it instead.".into());
    }
    let cluster = session.cluster();
    app_core::diagnostics::TxLog {
        flow: *flow,
        wire: &done.wire,
        cluster: Some(&cluster),
    }
    .log_signed("hot key");
    Ok(done.wire)
}

/// Log that `wire` goes back to the site of request `id`.
fn log_returned(session: Session, flow: u64, wire: &[u8], origin: &str, id: &str) {
    let cluster = session.cluster();
    let code = crate::bridge::request_code(id);
    app_core::diagnostics::TxLog {
        flow,
        wire,
        cluster: Some(&cluster),
    }
    .log_returned(origin, &code);
}

/// The vault is locked, and preparing needs its wallets.
#[component]
fn LockedNotice() -> Element {
    let session = use_context::<Session>();
    rsx! {
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
    }
}

/// What the modal says about one prepared transaction: what was done to it,
/// what it does, and what signing it here takes.
fn prepared_body(prep: &PreparedRequest) -> Element {
    let mut attempt = prep.attempt;
    let mut paid_here = prep.paid_here;
    let fetching = prep.fetching;
    let countdown_label = prep.countdown_label;
    match prep.state.clone() {
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
            if let Some(adapted) = prep.adapted.clone() {
                AdaptedNotice { adapted }
            }
            if let Some(paid) = prep.offered.clone() {
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
            if let (Some(tx), Some(explained), Some(verdict)) = (prep.shown.clone(), prep.explained.clone(), prep.verdict) {
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
                        "{verdict.unread} instruction(s) could not be put in words with the IDLs on this device, so there is no summary of what Sign signs. Review the transaction to see everything in it."
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

/// Where one transaction of a batch stands, as its card tells the modal.
#[derive(Clone, PartialEq)]
struct BatchReport {
    can_review: bool,
    can_sign: bool,
    unread_sign: bool,
    review: Option<ReadyWire>,
    sign: Option<SignWire>,
    /// The paid-here offer, when the user switched to it.
    taken: Option<PaidHere>,
    site_signs: Vec<Pubkey>,
    clock: BlockhashClock,
}

/// A site asking for several transactions in one request. Each is prepared
/// and explained on its own card. Sign signs them all and answers only once
/// every one is signed; Review takes them to the Sign screen, one at a time.
#[component]
pub(in crate::ui) fn SignBatch(current: BridgePrompt, on_close: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let count = current.transactions;
    let reports = use_signal(|| vec![None::<BatchReport>; count]);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    // Preparing rebuilds around a wallet's keys, so nothing is prepared
    // until the vault is open.
    let locked = session.is_initialized() && !session.is_unlocked();

    let ready: Option<Vec<BatchReport>> = reports().into_iter().collect();
    let can_review = !locked
        && ready
            .as_ref()
            .is_some_and(|ready| ready.iter().all(|item| item.can_review));
    let can_sign = can_review
        && ready
            .as_ref()
            .is_some_and(|ready| ready.iter().all(|item| item.can_sign));
    let unread_sign = can_sign
        && ready
            .as_ref()
            .is_some_and(|ready| ready.iter().any(|item| item.unread_sign));
    let to_sign = ready
        .as_ref()
        .map_or(0, |ready| ready.iter().filter(|item| item.can_sign).count());

    let reject_id = current.id.clone();
    let review_id = current.id.clone();
    let sign_id = current.id.clone();
    let origin = current.origin.clone();
    let origin_for_sign = origin.clone();
    let for_review = ready.clone();
    let for_sign = ready;

    rsx! {
        RequestHead { current: current.clone(), title: format!("Sign {count} transactions"), on_close }
        div { class: "request-body",
            p { class: "muted",
                "The site sent {count} transactions in one request. It gets them all back once every one is signed, or none if you reject."
            }
            if locked {
                LockedNotice {}
            } else {
                for index in 0..count {
                    BatchCard { key: "{index}", current: current.clone(), index, reports }
                }
            }
            if let Some(text) = error() {
                p { class: "error", role: "alert", "{text}" }
            }
        }
        if !locked && !can_sign && to_sign < count {
            p { class: "hint",
                "Sign is offered when this device can finish every transaction. Review takes them to the Sign screen one at a time, and returns them together after the last."
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
                class: if can_sign { "" } else { "primary" },
                disabled: !can_review,
                onclick: move |_| {
                    let Some(reports) = for_review.clone() else {
                        return;
                    };
                    let mut items = Vec::with_capacity(reports.len());
                    for report in &reports {
                        let Some((wire, adapted, _, _, tables, _, flow)) = report.review.clone() else {
                            return;
                        };
                        let note = review_note(adapted.is_some(), report.taken.is_some());
                        items.push(BatchItem {
                            wire,
                            tables,
                            vector: adapted
                                .as_ref()
                                .map(|adapted| adapted.context.clone())
                                .unwrap_or_default(),
                            expiry: *report.clock.expiry.peek(),
                            flow,
                            note: (!note.is_empty()).then(|| note.into()),
                            signed: false,
                        });
                    }
                    for paid in reports.iter().filter_map(|report| report.taken.as_ref()) {
                        paid.log_taken(session);
                    }
                    session.open_batch_in_sign(BrowserBatch {
                        id: review_id.clone(),
                        origin: origin.clone(),
                        items,
                        current: 0,
                    });
                    on_close.call(());
                },
                "Review"
            }
            button {
                class: sign_class(can_sign, unread_sign),
                disabled: !can_sign || busy(),
                title: if unread_sign { UNREAD_SIGN } else { "" },
                onclick: move |_| {
                    let Some(reports) = for_sign.clone() else {
                        return;
                    };
                    error.set(None);
                    busy.set(true);
                    // Every one is signed before any goes back.
                    let mut signed = Vec::with_capacity(reports.len());
                    for (index, report) in reports.iter().enumerate() {
                        let Some(sign) = &report.sign else {
                            busy.set(false);
                            return;
                        };
                        match sign_for_site(session, sign, &report.site_signs) {
                            Ok(wire) => signed.push((sign.3, wire)),
                            Err(err) => {
                                error.set(Some(format!(
                                    "Transaction {}: {err} Nothing was sent to the site.",
                                    index + 1
                                )));
                                busy.set(false);
                                return;
                            }
                        }
                    }
                    for paid in reports.iter().filter_map(|report| report.taken.as_ref()) {
                        paid.log_taken(session);
                    }
                    for (flow, wire) in &signed {
                        log_returned(session, *flow, wire, &origin_for_sign, &sign_id);
                    }
                    let wires = signed.into_iter().map(|(_, wire)| wire).collect();
                    if !session.bridge_answer(&sign_id, BridgeAnswer::Signed(wires)) {
                        error.set(Some("The site stopped waiting for this request.".into()));
                    }
                    busy.set(false);
                },
                "Sign all {count}"
            }
        }
    }
}

/// One transaction of a batch: prepared, explained, and reported to the
/// modal.
#[component]
fn BatchCard(
    current: BridgePrompt,
    index: usize,
    mut reports: Signal<Vec<Option<BatchReport>>>,
) -> Element {
    let session = use_context::<Session>();
    let prep = use_prepared_request(session, current.clone(), index);
    let mut open = use_signal(|| true);

    let report = BatchReport {
        can_review: prep.can_review,
        can_sign: prep.can_sign,
        unread_sign: prep.unread_sign,
        review: prep.for_review.clone(),
        sign: prep.for_sign.clone(),
        taken: prep.offered.clone().filter(|_| prep.replaced_payer),
        site_signs: prep.site_signs.clone(),
        clock: prep.clock,
    };
    use_effect(use_reactive!(|(report)| {
        reports.with_mut(|reports| {
            if let Some(slot) = reports.get_mut(index) {
                *slot = Some(report);
            }
        });
    }));

    let (chip, chip_text) = match (&prep.state, prep.original.is_some()) {
        (_, false) => ("chip danger", "Unreadable"),
        (Prepared::Working, _) => ("chip", "Preparing…"),
        (Prepared::Failed(_), _) => ("chip danger", "Cannot be prepared"),
        (Prepared::Ready { .. }, _) if prep.unread_sign => ("chip warn", "Sign without summary"),
        (Prepared::Ready { .. }, _) if prep.can_sign => ("chip ok", "Can sign here"),
        (Prepared::Ready { .. }, _) => ("chip warn", "Review"),
    };
    let number = index + 1;
    let count = current.transactions;
    let detail = crate::bridge::sign_detail(&current.params, index);

    rsx! {
        BlockhashCountdown {
            clock: prep.clock,
            lifetime: prep.lifetime,
            expired: prep.clock_expired,
            label: prep.countdown_label,
        }
        details { class: "accordion batch-card", open: open(),
            summary { class: "accordion-header",
                onclick: move |e| {
                    e.prevent_default();
                    open.set(!open());
                },
                span { class: "chip-row",
                    span { class: "accordion-title", "Transaction {number} of {count}" }
                    span { class: chip, "{chip_text}" }
                }
                span { class: "accordion-chevron", aria_hidden: "true" }
            }
            if open() {
                div { class: "accordion-body",
                    if let Some(account) = prep.account {
                        div { class: "chip-row",
                            span { class: "muted", "asked of" }
                            Address { pubkey: account }
                        }
                    }
                    if prep.original.is_none() {
                        p { class: "warn", "{detail}" }
                    } else {
                        {prepared_body(&prep)}
                    }
                }
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
