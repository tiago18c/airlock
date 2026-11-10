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
use crate::state::forms::{ShareTab, SignView, SimState, SiteSend, TxSource};
use crate::state::session::Session;
use crate::state::use_network;
use crate::ui::clock::BlockhashCountdown;
use crate::ui::transfer::receive::QrScanner;
use crate::ui::transfer::send::{CopyableText, PayloadShare, QrCarousel};
use crate::ui::tx_view::{
    use_explained, FeeLine, ReadInstructions, SimulatedDeltas, TxView, WithoutDeltas,
};
use crate::ui::{Address, AddressRow, CopyButton, Glyph, Icon};
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
    context_note_for, missing_catalog_programs, next_step, sha256_bytes, sign_steps,
    signature_objects, signing_status_in, tx_lifetime, DecodedTx, LocalKeys, NextSignature,
    SignFinish, SignStepInputs, SignStepKind, SignerRole, SignerState, SigningStatus,
    SimulationStage, TxFormat,
};
use wallet_flows::offer::{
    batch_keepable, blockhash_plan, may_sign, may_submit, BatchFacts, BlockhashFacts,
    BlockhashPlan, ButtonFacts,
};
use wallet_flows::sign::wants_fresh_blockhash;

mod banners;
mod blind;
mod inbox;
mod share;
mod simulation;
mod steps;
mod submit;

use blind::{offer_on_screen, use_stored_behind, BlindApprovals};

pub(in crate::ui) use banners::*;
pub(in crate::ui) use inbox::*;
pub(in crate::ui) use share::*;
pub(in crate::ui) use simulation::*;
pub(in crate::ui) use steps::*;
use submit::SubmitProgress;

#[component]
pub fn SignPanel() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let mut view = form.view;
    let online = use_network().is_some();
    // A phone has no browser requests, and no room to say where loads come from.
    let phone = crate::ui::use_phone_layout();
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
            match report.missing {
                0 => {}
                1 => extra.push("No on-chain IDL for 1 program.".into()),
                count => extra.push(format!("No on-chain IDL for {count} programs.")),
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
                        // Without a node, what was signed here goes out again from here.
                        if !online {
                            KeptRotations {}
                        }
                        if !phone {
                            p { class: "hint",
                                "Build sends its transactions here on its own. So does Review on a browser request."
                            }
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
        SubmitProgress {}
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
    let devices = session.local_device_keys();
    let local = LocalKeys {
        keys: &session.local_signer_pubkeys(),
        vector: &vector_here,
        devices: &devices,
    };
    let ctx = AuthContext { vector: &known };
    let status = signing_status_in(tx.as_ref(), &local, &ctx);
    // Keys on a hardware wallet of this vault that Sign asks next, once
    // every authorization is in.
    let on_device = match next_step(&status) {
        NextSignature::Transaction => device_lines(tx.as_ref(), &known, &devices),
        NextSignature::Authorization => device_authorities(tx.as_ref(), &known, &devices),
        NextSignature::Nothing => Vec::new(),
    };
    let device_vendor = on_device.first().and_then(|key| {
        devices
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, vendor)| *vendor)
    });
    let signable_here = status.signable + on_device.len();
    // A programmatic signer or a Squads vault among the signers. No key
    // signs as either, so the transaction has to be rebuilt first.
    let plan = session
        .plan_adaptation(tx.as_ref())
        .map_err(sentence)
        .transpose();
    let needs_rebuild = plan.is_some();
    let relayed = !authority_slots_in(tx.as_ref(), &ctx).is_empty();
    let online = use_network().is_some();
    let reading = use_explained(tx.as_ref().clone());
    let approvals = wallet_core::blind_in(&reading.explained);
    let reading_stored = use_stored_behind(&approvals, online);
    let blind = offer_on_screen(session, tx.as_ref(), &approvals, reading_stored);
    let busy = use_signal(|| false);
    let share_open = use_signal(|| false);
    let source = (form.source)();
    let editable = tx.clone();
    let step = next_step(&status);
    let lifetime = Some(tx_lifetime(tx.as_ref()));
    let clock_expired = use_signal(|| false);
    let countdown_label = use_signal(|| String::from("…"));
    let uses_nonce = tx.uses_durable_nonce;

    // Sign moves a relayed transaction to a fresh blockhash by itself, so an
    // expired one does not hold Sign back. Vector keys are not transaction
    // signers, so wants_fresh_blockhash only needs the keys that sign it,
    // here and on a device.
    let signing_keys: Vec<Pubkey> = local
        .keys
        .iter()
        .copied()
        .chain(on_device.iter().copied())
        .collect();
    let refreshes_on_sign = online && wants_fresh_blockhash(tx.as_ref(), &signing_keys);
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
    let blockhash = blockhash_plan(BlockhashFacts {
        online,
        uses_nonce,
        refreshes_on_sign,
        clock_expired: clock_expired(),
        step,
        signable: signable_here,
        untouched: !tx.signers.iter().any(|slot| slot.valid),
        relayed,
        from_browser,
        app_built,
    });
    let expired = blockhash.expired;
    let site_blockhash = from_browser && !exact;
    // The node has answered for this very message: after an authorization,
    // the transaction is signed only once its simulation is on screen.
    let simulated = (form.simulated_for)() == Some(sha256_bytes(&tx.message))
        && matches!(
            &*form.simulation.read(),
            SimState::Done(_) | SimState::Failed(_)
        );
    let simulating = online && !simulated;
    let site_waiting = match &source {
        Some(TxSource::Browser { id, .. }) => session.bridge.read().get(id).is_some(),
        _ => false,
    };
    // The site asked this app to send it: Submit is what comes last.
    let app_sends = match &source {
        Some(TxSource::Browser { id, .. }) => session
            .bridge
            .read()
            .get(id)
            .is_some_and(|request| request.sends()),
        _ => false,
    };
    let returned = (form.returned)();
    // The site that has it back may send it itself.
    let site_send = match &source {
        Some(TxSource::Browser { origin, .. }) => {
            form.site_send(&tx.wire).map(|send| (send, origin.clone()))
        }
        _ => None,
    };
    // The batch this transaction is one of, when the site sent several.
    let batch = (form.batch)()
        .filter(|batch| matches!(&source, Some(TxSource::Browser { id, .. }) if *id == batch.id));
    let batch_signed = batch
        .as_ref()
        .is_some_and(|batch| batch.items[batch.current].signed);
    let batch_keepable = batch_keepable(BatchFacts {
        in_batch: batch.is_some(),
        already_kept: batch_signed,
        site_waiting,
        signable: signable_here,
        needs_rebuild,
        waiting_for_authority: status.waiting_for_authority,
        step,
        missing_elsewhere: status.missing_elsewhere(),
        left_for_site: site_signs,
        expired,
    });
    let mut steps = sign_steps(
        &status,
        &SignStepInputs {
            needs_rebuild,
            rebuilt: (form.rebuilt)(),
            simulation: simulation_stage(&(form.simulation)(), online),
            finish: if !app_sends && (site_waiting || (from_browser && returned)) {
                SignFinish::ReturnToSite
            } else if online {
                SignFinish::Submit
            } else {
                SignFinish::ShareToSubmit
            },
            finished: returned,
            expired: clock_expired(),
            // Sign moves the transaction off an expired blockhash, so it
            // holds nothing back.
            refreshes_on_sign: refreshes_on_sign || blockhash.on_sign != BlockhashOnSign::Keep,
            left_for_site: site_signs,
        },
    );
    // Collecting counts every signature, in the words of the badge beside
    // "Signatures", so the two never disagree.
    for step in &mut steps {
        if step.kind == SignStepKind::CollectSignatures {
            step.progress = Some((status.signed(), status.total()));
        }
    }
    // Nothing is left for this device to sign, and another device is next:
    // it signs what is missing, or submits what this offline one cannot.
    let share_next = tx.format == TxFormat::V1
        && !needs_rebuild
        && signable_here == 0
        && (status.missing_elsewhere() > site_signs || (status.complete() && !online));
    // Share is offered in one place at a time: the primary button while it
    // is the next step, else on the row of a signature still missing, else
    // outlined among the buttons.
    let shareable = tx.format == TxFormat::V1;
    let row_share = (shareable && !share_next).then_some(share_open);
    let row_offers_share = row_share.is_some()
        && status
            .lines
            .iter()
            .any(|line| line.state == SignerState::Missing);
    let site_send_for_hint = site_send.clone();
    let hint = sign_hint(HintFacts {
        status: &status,
        on_device: on_device.len(),
        online,
        blockhash,
        refreshes_on_sign,
        site_blockhash,
        needs_rebuild,
        rebuilding: (form.rebuilding)(),
        simulating,
        step,
        site_signs,
        site_send: site_send_for_hint,
    });
    // What is still needed of others is said under the signatures; what
    // holds the buttons back, under them.
    let (signer_hint, action_hint) = match hint {
        Some(hint) if hint.about_signers() => (Some(hint), None),
        other => (None, other),
    };
    let mut inbox_title = use_signal(|| "Add a signature");
    let mut view = form.view;

    // In a batch, the return is the batch's, not this transaction's.
    let batch_steps = batch.as_ref().map(|batch| {
        let mut own = steps.clone();
        own.pop();
        let signed: Vec<bool> = batch.items.iter().map(|item| item.signed).collect();
        (
            wallet_core::batch_steps(&signed, returned),
            batch.signed(),
            batch.current,
            own,
        )
    });

    rsx! {
        // Every transaction shows its steps, however few, so the screen
        // reads the same from one to the next.
        if let Some((states, signed, on_screen, own)) = batch_steps {
            BatchSteps { states, signed, on_screen, steps: own }
        } else {
            SignSteps { steps }
        }

        BlockhashCountdown {
            clock: form.clock,
            lifetime,
            expired: clock_expired,
            label: countdown_label,
        }

        if let Some(plan) = plan {
            RebuildBanner { plan, online }
        }

        PersonalSignBox { tx: tx.clone(), status: status.clone() }

        SimulatedDeltas {
            // The signature rows draw no balance change, so a signer's change
            // that no sentence names is listed under the transaction.
            TxView { tx: tx.as_ref().clone(), reading: reading.clone(), inspect,
                on_inspect: move |_| view.set(SignView::Inspect),
                fee_line: true,
                if let Some(source) = source.clone() {
                    SourceChip { source }
                }
            }

            FeeLine { tx: tx.as_ref().clone() }

            // What the format allows limits who can sign, so it is said
            // before the signatures, not after the buttons.
            match tx.format {
                TxFormat::V1 => rsx! {},
                TxFormat::Legacy => rsx! {
                    p { class: "note format-note",
                        Icon { glyph: Glyph::Info }
                        span {
                            "This transaction is in the legacy format, which cannot be handed to another device. Only keys on this device can sign it."
                        }
                    }
                },
                TxFormat::V0 => rsx! {
                    p { class: "note format-note",
                        Icon { glyph: Glyph::Info }
                        span {
                            "This transaction is in the older v0 format, which cannot be handed to another device. Only keys on this device can sign it."
                        }
                    }
                },
            }

            div { class: "sign-status",
                h3 { "Signatures" }
                span {
                    class: if status.complete() { "chip ok" } else { "chip warn" },
                    "{status.signed()} of {status.total()} signed"
                }
            }
            // What signing changes in a signer's balance is not a matter of
            // signing: it stays in the sentences and the Accounts list.
            WithoutDeltas {
                SignerList { status: status.clone(), inspect, share: row_share }
            }
            if let Some(hint) = signer_hint {
                SignHintText { hint, class: "signer-hint" }
            }
        }

        SimulationStrip { tx: tx.clone(), inspect }

        if !approvals.is_empty() {
            BlindApprovals {
                tx: tx.clone(),
                approvals,
                offer: blind,
                reading: reading_stored,
                online,
                inspect,
            }
        }

        SignActions {
            status: status.clone(),
            on_device: on_device.len(),
            device_vendor,
            step,
            online,
            busy,
            blind_holds: blind.holds_sign,
            reading: reading.unsettled(),
            simulating,
            needs_rebuild,
            refreshes_on_sign,
            blockhash,
            site_signs,
            site_send,
            from_site: from_browser,
            share_next,
            share_open,
            shareable: shareable && !row_offers_share,
            hint: action_hint,
            on_add_signature: move |_| {
                inbox_title.set("Add a signature");
                inbox_open.set(true);
            },
            on_load_another: move |_| {
                inbox_title.set("Load another transaction");
                inbox_open.set(true);
            },
            edit_title: if from_browser {
                "Change it on Build. What Build compiles is a new transaction, and it does not go back to the site."
            } else {
                "Change it on Build. What Build compiles is a new transaction, signed again."
            },
            on_edit: move |_| edit_on_build(&session, &editable),

            if let Some(batch) = batch.as_ref().filter(|_| site_waiting) {
                if batch_signed {
                    p { class: "hint",
                        "Signed. It goes back to the site with the others, once all {batch.len()} are signed."
                    }
                } else if batch_keepable {
                    div { class: "btn-row",
                        button {
                            class: "primary",
                            onclick: move |_| keep_batch_item(session),
                            if batch.signed() + 1 == batch.len() {
                                "Keep as is and return all {batch.len()}"
                            } else {
                                "Keep as is and go to the next"
                            }
                        }
                    }
                    p { class: "hint",
                        "This device has nothing to sign in this transaction. The site gets it back as it is, with the others."
                    }
                }
            }
        }

        if inbox_open() {
            SignModal { title: inbox_title(), open: inbox_open,
                Inbox { loaded: true }
                if let Some(text) = (form.error)() {
                    p { class: "error", role: "alert", "{text}" }
                }
            }
        }
        if share_open() && tx.format == TxFormat::V1 {
            SignModal { title: "Share with another device", class: "share", open: share_open,
                Share { tx: tx.clone(), complete: status.complete(), status: status.clone() }
            }
        }
    }
}

/// A job on a hardware wallet is on screen.
fn device_busy(session: Session) -> bool {
    crate::actions::device::busy(session)
}

/// Where the simulation stands, for the steps.
fn simulation_stage(state: &SimState, online: bool) -> SimulationStage {
    match state {
        SimState::Idle if online => SimulationStage::Waiting,
        SimState::Idle => SimulationStage::Offline,
        SimState::Waiting(_) => SimulationStage::Waiting,
        SimState::Running => SimulationStage::Running,
        SimState::Done(simulation) if simulation.error.is_some() => SimulationStage::Failed,
        SimState::Done(_) => SimulationStage::Succeeded,
        SimState::Failed(_) => SimulationStage::Unanswered,
    }
}

/// A dialog over the Sign screen, closed by its button or the backdrop.
#[component]
fn SignModal(
    title: &'static str,
    /// A class of its own beside `modal wide`, for what it holds.
    #[props(default)]
    class: &'static str,
    mut open: Signal<bool>,
    children: Element,
) -> Element {
    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| open.set(false),
            div { class: "modal wide {class}",
                role: "dialog",
                aria_label: title,
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "{title}" }
                    {children}
                    div { class: "btn-row modal-foot",
                        button {
                            r#type: "button",
                            onclick: move |_| open.set(false),
                            "Close"
                        }
                    }
                }
            }
        }
    }
}

/// The screen's buttons, in one row and one order in every state: the
/// state's primary button (Sign, Share with another device or Submit), then
/// Add a signature and Share with another device, outlined, when each applies
/// and is not the primary (Share only where no signer row offers it), then
/// Load another transaction and Edit on Build, quiet, at the far end. A hint
/// about what holds the buttons back goes under the row; one about what is
/// still needed of others, under the signatures.
#[component]
fn SignActions(
    status: SigningStatus,
    /// Signatures a hardware wallet of this vault adds on this press.
    on_device: usize,
    /// The vendor of the first of them.
    device_vendor: Option<wallet_core::HardwareVendor>,
    step: NextSignature,
    online: bool,
    busy: Signal<bool>,
    needs_rebuild: bool,
    /// It says yes to something it does not hold, and that is not settled.
    blind_holds: bool,
    /// It is still being read.
    reading: bool,
    /// The node has not answered for this message yet.
    simulating: bool,
    refreshes_on_sign: bool,
    blockhash: BlockhashPlan,
    site_signs: usize,
    /// What came of the transaction since it went back to the site, and
    /// the site.
    site_send: Option<(SiteSend, String)>,
    /// A site asked for it: it goes back there, and this press does not send it.
    from_site: bool,
    /// Nothing is left for this device to sign and another device is next:
    /// Share leads the row.
    share_next: bool,
    mut share_open: Signal<bool>,
    /// The transaction can be handed to another device, and no signer row
    /// offers it already: Share with another device is among the outlined.
    shareable: bool,
    /// The sentence under the buttons, about what holds them back.
    hint: Option<SignHint>,
    on_add_signature: EventHandler<()>,
    on_load_another: EventHandler<()>,
    /// Edit on Build's tooltip.
    edit_title: String,
    on_edit: EventHandler<()>,
    /// The rows under these buttons, pinned with them.
    children: Element,
) -> Element {
    let session = use_context::<Session>();
    let BlockhashPlan {
        on_sign: blockhash_on_sign,
        offer_refresh: show_refresh,
        expired,
    } = blockhash;
    let buttons = ButtonFacts {
        online,
        needs_rebuild,
        busy: busy() || device_busy(session),
        step,
        signable: status.signable + on_device,
        complete: status.complete(),
        refreshes_on_sign,
        blockhash,
        // Submit waits while the site may be sending it, and stays off once
        // it has landed.
        site_holds: matches!(site_send, Some((SiteSend::Watching | SiteSend::Landed, _))),
        blind_holds,
        reading,
        simulating,
    };
    let can_sign = may_sign(buttons);
    let can_submit = may_submit(buttons);
    let elsewhere = status.missing_elsewhere();
    // This press leaves nothing missing, so it submits too.
    let submits = online && !from_site && elsewhere == 0;
    let sign_label = match step {
        // The authorizations go on a press of their own.
        NextSignature::Authorization => {
            let here = status.authorizations_here();
            if let (0, Some(vendor)) = (here, device_vendor.filter(|_| on_device > 0)) {
                format!("Sign authorization on {}", vendor.label())
            } else if here > 1 {
                format!("Sign authorization ({here})")
            } else {
                "Sign authorization".into()
            }
        }
        NextSignature::Transaction => {
            let label = match (status.signable, on_device, device_vendor) {
                (0, 1, Some(vendor)) => format!("Sign on {}", vendor.label()),
                (0, count, Some(vendor)) if count > 1 => {
                    format!("Sign on {} ({count})", vendor.label())
                }
                (here, device, _) if here + device > 1 => {
                    format!("Sign transaction ({})", here + device)
                }
                _ => "Sign transaction".into(),
            };
            if submits {
                format!("{label} and submit")
            } else {
                label
            }
        }
        NextSignature::Nothing => "Sign".into(),
    };
    // Sign is left out while this device has nothing to sign, now, once an
    // authorization from elsewhere is in, or once it is rebuilt: it could
    // not be pressed.
    let sign_shown =
        status.signable + on_device > 0 || status.waiting_for_authority || needs_rebuild;
    let submit_waits = (!can_submit).then(|| {
        submit_waits_for(
            &status,
            SubmitHold {
                online,
                needs_rebuild,
                busy: buttons.busy,
                expired,
                site_send: site_send.as_ref().map(|(send, _)| *send),
                site_signs,
            },
        )
    });

    rsx! {
        // Pinned to the bottom of the pane, so the next step is in reach
        // however long the transaction is.
        div { class: "sign-actions",
            div { class: "sign-actions-bar",
                div { class: "btn-row sign-actions-row",
                    // The state's own buttons, its primary first.
                    if share_next {
                        button {
                            class: "primary",
                            onclick: move |_| share_open.set(true),
                            "Share with another device"
                        }
                    }
                    if sign_shown {
                        button {
                            class: if share_next { "" } else { "primary" },
                            disabled: !can_sign,
                            onclick: move |_| sign_loaded(session, busy, blockhash_on_sign),
                            "{sign_label}"
                        }
                    }
                    // Submit could not be pressed while a signature is missing,
                    // so it waits until every one is in.
                    if status.complete() {
                        button {
                            class: if share_next || sign_shown { "" } else { "primary" },
                            disabled: !can_submit,
                            title: submit_waits,
                            onclick: move |_| submit_loaded(session, busy),
                            "Submit"
                        }
                    }
                    if show_refresh {
                        RefreshBlockhash {}
                    }
                    // What else applies, outlined.
                    button {
                        onclick: move |_| on_add_signature.call(()),
                        "Add a signature"
                    }
                    if shareable && !share_next {
                        button {
                            onclick: move |_| share_open.set(true),
                            "Share with another device"
                        }
                    }
                    // The ways out, quiet, at the far end.
                    div { class: "sign-actions-end",
                        button { class: "ghost",
                            onclick: move |_| on_load_another.call(()),
                            "Load another transaction"
                        }
                        button { class: "ghost",
                            title: edit_title,
                            onclick: move |_| on_edit.call(()),
                            "Edit on Build"
                        }
                    }
                }
                {children}
            }
        }
        if let Some(hint) = hint {
            SignHintText { hint, class: "hint" }
        }
    }
}

/// The one sentence under the signatures or the buttons that says what the
/// transaction still waits for.
#[derive(Clone, Debug, PartialEq)]
enum SignHint {
    /// What came of it since it went back to the site, and the site.
    SiteSend(SiteSend, String),
    /// The blockhash expired, and nothing here moves it: the site's, or not.
    Expired {
        site: bool,
    },
    Rebuilding,
    Rebuild,
    Simulating,
    /// Authorization signatures still needed from other devices.
    Authorizations(usize),
    /// Signatures only the site adds.
    SiteSigns(usize),
    OnDevice,
    /// Signatures still needed from other devices.
    Elsewhere(usize),
    CompleteOffline,
}

impl SignHint {
    /// It says what is still needed of others, so it goes under the
    /// signatures, not under the buttons.
    fn about_signers(&self) -> bool {
        matches!(
            self,
            SignHint::Authorizations(_) | SignHint::SiteSigns(_) | SignHint::Elsewhere(_)
        )
    }
}

/// What [`sign_hint`] reads.
struct HintFacts<'a> {
    status: &'a SigningStatus,
    on_device: usize,
    online: bool,
    blockhash: BlockhashPlan,
    refreshes_on_sign: bool,
    site_blockhash: bool,
    needs_rebuild: bool,
    rebuilding: bool,
    simulating: bool,
    step: NextSignature,
    site_signs: usize,
    site_send: Option<(SiteSend, String)>,
}

/// The sentence that applies, the first of them in this order.
fn sign_hint(facts: HintFacts) -> Option<SignHint> {
    let status = facts.status;
    let elsewhere = status.missing_elsewhere();
    if let Some((send, origin)) = facts.site_send.filter(|_| status.complete()) {
        return Some(SignHint::SiteSend(send, origin));
    }
    if facts.blockhash.expired
        && !facts.refreshes_on_sign
        && facts.blockhash.on_sign == BlockhashOnSign::Keep
    {
        // With the rebuild button there, it says what to do.
        return (!facts.blockhash.offer_refresh).then_some(SignHint::Expired {
            site: facts.site_blockhash,
        });
    }
    if facts.needs_rebuild {
        return Some(if facts.rebuilding {
            SignHint::Rebuilding
        } else {
            SignHint::Rebuild
        });
    }
    let waits_for_simulation = facts.simulating && facts.step == NextSignature::Transaction;
    if waits_for_simulation && status.signable + facts.on_device > 0 {
        return Some(SignHint::Simulating);
    }
    if status.waiting_for_authority {
        return Some(SignHint::Authorizations(status.authorizations_elsewhere()));
    }
    if elsewhere > 0 && elsewhere == facts.site_signs && status.signable == 0 {
        return Some(SignHint::SiteSigns(elsewhere));
    }
    if facts.on_device > 0 && status.signable == 0 && !status.complete() {
        return Some(SignHint::OnDevice);
    }
    if elsewhere > 0 && status.signable == 0 {
        return Some(SignHint::Elsewhere(elsewhere));
    }
    if status.complete() && !facts.online {
        return Some(SignHint::CompleteOffline);
    }
    None
}

#[component]
fn SignHintText(hint: SignHint, class: &'static str) -> Element {
    rsx! {
        p { class,
            match hint {
                SignHint::SiteSend(send, origin) => match send {
                    SiteSend::Watching => rsx! { "Watching for {origin} to send it. Submit waits a few seconds so it is not sent twice." },
                    SiteSend::Landed => rsx! { "{origin} sent it, and it landed. There is nothing left to submit." },
                    SiteSend::NotSent => rsx! { "{origin} did not send it. Submit it here." },
                },
                SignHint::Expired { site: true } => rsx! {
                    "The site's blockhash has expired. Ask the site to send the transaction again."
                },
                SignHint::Expired { site: false } => rsx! {
                    "The blockhash has expired. Whoever made the transaction has to make it again on a fresh blockhash."
                },
                SignHint::Rebuilding => rsx! { "Rebuilding the transaction for the wallet it asks to sign…" },
                SignHint::Rebuild => rsx! { "Rebuild the transaction for the wallet it asks to sign before signing." },
                SignHint::Simulating => rsx! { "Simulating it first, so you can read what it does before you sign." },
                SignHint::Authorizations(count) => rsx! {
                    if count == 1 {
                        "1 authorization signature is still needed from another device."
                    } else {
                        "{count} authorization signatures are still needed from other devices."
                    }
                    " Programmatic authorities sign first; Vector keys sign after those signatures are in. This device then moves the transaction to a fresh blockhash and signs as fee payer."
                },
                SignHint::SiteSigns(count) => rsx! {
                    if count == 1 {
                        "1 signature is the site's: it signs for the account it creates, such as a new mint, once it has the transaction back."
                    } else {
                        "{count} signatures are the site's: it signs for the accounts it creates, such as a new mint, once it has the transaction back."
                    }
                },
                SignHint::OnDevice => rsx! {
                    "Sign asks the device that holds the key for its signature: the Seed Vault on this phone, or a hardware wallet, connected and unlocked with the Solana app open."
                },
                SignHint::Elsewhere(count) => rsx! {
                    if count == 1 {
                        "1 signature is needed from another device. Share the transaction, then add what it returns."
                    } else {
                        "{count} signatures are needed from other devices. Share the transaction, then add what they return."
                    }
                },
                SignHint::CompleteOffline => rsx! {
                    "Fully signed. Go online to submit, or share it with a device that is."
                },
            }
        }
    }
}

/// What holds Submit back, besides the signatures.
#[derive(Clone, Copy)]
struct SubmitHold {
    online: bool,
    needs_rebuild: bool,
    busy: bool,
    expired: bool,
    site_send: Option<SiteSend>,
    /// Missing signatures the site adds once it has the transaction back.
    site_signs: usize,
}

/// Why Submit cannot be pressed, for its tooltip.
fn submit_waits_for(status: &SigningStatus, hold: SubmitHold) -> String {
    if !hold.online {
        return "Submit needs a connection to a node. Share the signed transaction with a device that has one.".into();
    }
    if hold.needs_rebuild {
        return "Waits for the transaction to be rebuilt and signed.".into();
    }
    if !status.complete() {
        let elsewhere = status.missing_elsewhere();
        let here = status.total() - status.signed() - elsewhere;
        let others = elsewhere.saturating_sub(hold.site_signs);
        let site = elsewhere - others;
        let mut waits = Vec::new();
        match here {
            0 => {}
            1 => waits.push("a signature of this device".to_string()),
            n => waits.push(format!("{n} signatures of this device")),
        }
        match others {
            0 => {}
            1 => waits.push("a signature from another device".to_string()),
            n => waits.push(format!("{n} signatures from other devices")),
        }
        match site {
            0 => {}
            1 => waits.push("a signature from the site".to_string()),
            n => waits.push(format!("{n} signatures from the site")),
        }
        return format!("Waits for {}.", and_list(&waits));
    }
    if hold.busy {
        return "Waits for the signing under way.".into();
    }
    if hold.expired {
        return "The blockhash has expired, so it cannot be submitted.".into();
    }
    match hold.site_send {
        Some(SiteSend::Watching) => {
            "Waits a few seconds for the site to send it, so it is not sent twice.".into()
        }
        Some(SiteSend::Landed) => "The site sent it, and it landed.".into(),
        _ => "It cannot be submitted yet.".into(),
    }
}

/// "a", "a and b", "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
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
