//! One proposal: its status, how its votes stand, what it stores, and what
//! this device may do with it.
//!
//! What the acting member may do comes from `wallet_core::proposals`
//! (`member_choices`): the action that moves the proposal forward, which is
//! the primary button (Open for votes on a draft, Approve on an open
//! proposal, Execute on an approved one), and the careful choice beside it:
//! Reject on an open proposal, Vote to cancel on an approved one. That one is
//! never the primary: a danger-outlined button whose tooltip says what it
//! does, such as rejecting or cancelling the proposal for good. A draft has
//! no such choice, since neither program rejects or cancels one. Every
//! button opens a transaction in Review and Sign.
//!
//! The row says little beside its buttons, on purpose: what the chips of
//! its head already tell (how many approved, that nothing is left to do) is
//! not said again in a sentence, and a reason is shown only where a person
//! would otherwise not know why a button is missing.
//!
//! A vault transaction that can still run (a draft or an open proposal not
//! made stale, or an approved one) has a quiet Simulate button under its
//! instructions: it asks the node what the vault would do if it ran them now
//! (`wallet_core::squads::VaultRun`) and shows the answer under the button,
//! the way Review and Sign shows a simulation. It signs nothing.
//!
//! A change to the multisig itself reads as its actions in words, one card
//! each (`app_ui_kit::ui::MultisigChange`), where a vault transaction lists
//! its instructions. It is approved and executed the same way.
//!
//! Once the program allows it (`available_close`), a row also offers to
//! close the proposal's accounts: a quiet button, since it is housekeeping.
//! No member signs it, only the wallet paying the fee. A row that cannot be
//! closed says nothing of closing, except a finished one of a v4 multisig
//! without a rent collector, whose button is there, disabled, and says in
//! its tooltip what is missing. A transaction whose accounts were closed
//! reads as closed.

use super::votes::{vote_chips, ProposalVotes};
use super::*;
use app_ui_kit::state::forms::SimState;
use app_ui_kit::ui::{MultisigChange, SimulationStrip};
use wallet_core::proposals::{available_close, standing};
use wallet_core::squads::change_summary;
use wallet_flows::simulate::simulate_vault_run;

/// What a disabled Close says when the multisig has no rent collector.
const SET_RENT_COLLECTOR: &str = "Set a rent collector for this multisig to close accounts.";

/// Whether `reason` is worth a line under the row's buttons. What the head's
/// chips already tell is not: how many approvals are missing, and that a
/// proposal was executed, rejected or cancelled. Nor is what the row says
/// above: why a change is refused, and that a transaction cannot be read.
fn worth_a_line(reason: &NoAction) -> bool {
    !matches!(
        reason,
        NoAction::WaitingForOthers { .. }
            | NoAction::Finished
            | NoAction::Rejected
            | NoAction::Cancelled
            | NoAction::Refused(_)
            | NoAction::Unreadable
    )
}

/// The button of what one member does to a proposal.
fn action_label(action: ProposalAction) -> &'static str {
    match action {
        ProposalAction::Approve => "Approve",
        ProposalAction::Execute => "Execute",
        ProposalAction::ApproveAndExecute => "Approve and execute",
        ProposalAction::Activate => "Open for votes",
        ProposalAction::Reject => "Reject",
        ProposalAction::Cancel => "Vote to cancel",
    }
}

#[component]
pub(in crate::ui) fn ProposalRow(
    squad: Squad,
    state: SquadState,
    entry: ProposalEntry,
    acting: Option<(Pubkey, Option<Pubkey>)>,
    chosen: Vec<ChosenMember>,
    approval_nonce: Option<Pubkey>,
    now: i64,
    open: bool,
    busy: bool,
    on_toggle: EventHandler<u64>,
    on_act: EventHandler<(ProposalEntry, ProposalRequest, Pubkey)>,
) -> Element {
    let index = entry.index;
    let (chip_class, chip_text) = status_chip(&entry);
    let status = entry.proposal.as_ref().map(|proposal| proposal.status);
    let voting = matches!(
        status,
        Some(ProposalStatus::Active | ProposalStatus::Approved)
    );
    let standing = standing(&state, &entry);
    let member = acting.map(|(member, _)| member);
    let chips = match (&entry.proposal, &standing) {
        (Some(proposal), Some(standing)) => vote_chips(proposal, &standing.counts, member),
        _ => Vec::new(),
    };
    let choices = acting.map(|(member, _)| member_choices(&state, &entry, &member, now));
    // Nothing is said of what can be done to a transaction whose accounts
    // were closed: it reads as closed, and nothing can.
    let closed = entry.stored == Stored::Missing && entry.proposal.is_none();
    let verdict = choices
        .as_ref()
        .map(|choices| choices.forward.clone())
        .filter(|_| !closed);
    let payer = acting.and_then(|(_, payer)| payer);
    // The careful choice: Reject on an open proposal, Vote to cancel on an
    // approved one. Offered only with someone to pay for it, like every
    // other action.
    let turn_down = choices
        .as_ref()
        .and_then(|choices| choices.turn_down())
        .filter(|_| payer.is_some());
    let for_turn_down = entry.clone();
    let turn_down_button = match (acting, payer, turn_down.clone()) {
        (Some((member, _)), Some(payer), Some((action, hint))) => rsx! {
            button { class: "danger sm", disabled: busy, title: "{hint}",
                onclick: move |_| on_act.call((
                    for_turn_down.clone(),
                    ProposalRequest::Single { member, action },
                    payer,
                )),
                {action_label(action)}
            }
        },
        _ => rsx! {},
    };
    // Every vote the proposal still needs, cast by the wallets the row names.
    let combined = acting.and_then(|_| combined_approval(&state, &entry, &chosen, approval_nonce));
    // Another wallet of this device that executes it when the acting member
    // may not.
    let instead = acting
        .and_then(|_| executor_instead(&state, &entry, &chosen, now))
        .map(|member| member.address);
    let summary = match &entry.stored {
        Stored::Vault(tx) => match tx.instructions.len() {
            1 => "1 instruction".to_string(),
            count => format!("{count} instructions"),
        },
        Stored::LookupTables(_) => "lookup tables not read".to_string(),
        Stored::Config(change) => change_summary(&change.actions),
        Stored::Other => "not read".to_string(),
        Stored::Missing if entry.proposal.is_none() => "accounts closed".to_string(),
        Stored::Missing => "transaction account closed".to_string(),
    };
    // Closing its accounts: the fee payer alone signs, so it is offered
    // with someone to pay, whoever the acting member is.
    let closing = available_close(squad.program, &state, &entry);
    let for_close = entry.clone();
    // What the vault would do if it ran the stored transaction now: for one
    // that can still run. The wallet that pays the row's fees pays the
    // simulation's, else the vault itself; either way the fee is not
    // counted (`VaultRun::read`).
    let session = use_context::<Session>();
    let mut simulated = use_signal(|| SimState::Idle);
    let runnable = match (&entry.stored, status) {
        (Stored::Vault(tx), Some(ProposalStatus::Approved)) => Some(tx.clone()),
        (Stored::Vault(tx), Some(ProposalStatus::Draft | ProposalStatus::Active))
            if !entry.stale =>
        {
            Some(tx.clone())
        }
        _ => None,
    };
    let simulating = matches!(simulated(), SimState::Running);
    let simulate_block = match runnable {
        Some(tx) => {
            let fee_payer = payer.unwrap_or(tx.vault);
            let simulate = move |_| {
                let run = match tx.run_now(fee_payer) {
                    Ok(run) => run,
                    Err(why) => {
                        simulated.set(SimState::Waiting(why.to_string()));
                        return;
                    }
                };
                let rpc = match session.rpc() {
                    Ok(rpc) => rpc,
                    Err(err) => {
                        simulated.set(SimState::Failed(err.to_string()));
                        return;
                    }
                };
                simulated.set(SimState::Running);
                spawn(async move {
                    let result = simulate_vault_run(rpc.as_ref(), &run).await;
                    simulated.set(match result {
                        Ok(simulation) => SimState::Done(simulation),
                        Err(err) => SimState::Failed(err.to_string()),
                    });
                });
            };
            rsx! {
                div { class: "btn-row",
                    button { class: "sm", r#type: "button",
                        disabled: simulating,
                        aria_busy: "{simulating}",
                        onclick: simulate,
                        if simulating { "Simulating…" } else { "Simulate" }
                    }
                }
                if !matches!(simulated(), SimState::Idle | SimState::Running) {
                    SimulationStrip { state: simulated, limit: None, inspect: false, changes: true }
                }
            }
        }
        None => rsx! {},
    };
    let for_action = entry.clone();
    let for_together = entry.clone();

    rsx! {
        li { class: "book-row proposal-row",
            button { class: "proposal-head", aria_expanded: open,
                onclick: move |_| on_toggle.call(index),
                span { class: "proposal-number", "#{index}" }
                span { class: "{chip_class}", "{chip_text}" }
                if let (true, Some(standing)) = (voting, &standing) {
                    span { class: "chip",
                        "{standing.counts.approved.count} of {standing.counts.approved.needed} approved"
                    }
                }
                for (class, text, title) in chips {
                    span { key: "{text}", class: "{class}", title: title.unwrap_or_default(), "{text}" }
                }
                span { class: "muted", "{summary}" }
            }
            if open {
                div { class: "proposal-body stack",
                    match &entry.stored {
                        Stored::Vault(tx) => rsx! {
                            p { class: "chip-row",
                                span { class: "muted", "Runs as vault" }
                                Address { pubkey: tx.vault }
                                span { class: "muted", "stored by" }
                                Address { pubkey: tx.creator }
                            }
                            ReadInstructions {
                                instructions: tx.instructions.clone(),
                                inspect: false,
                            }
                            if !tx.other_signers.is_empty() {
                                p { class: "chip-row hint",
                                    span { "Executing it also needs a signature from" }
                                    for key in tx.other_signers.iter().copied() {
                                        Address { key: "{key}", pubkey: key }
                                    }
                                }
                            }
                        },
                        Stored::LookupTables(TableProblem::Missing { table }) => rsx! {
                            p { class: "warn chip-row",
                                span { "This transaction reads accounts from lookup table" }
                                Address { pubkey: *table }
                                span { ", which does not exist on this cluster: it was closed, or never made. It cannot be read or executed." }
                            }
                        },
                        Stored::LookupTables(TableProblem::Short { table, index, holds }) => rsx! {
                            p { class: "warn chip-row",
                                span { "This transaction reads entry {index} of lookup table" }
                                Address { pubkey: *table }
                                span { ", which holds only {holds} addresses. What that entry will hold is not known yet, so it cannot be read, approved or executed." }
                            }
                        },
                        Stored::Config(change) => rsx! {
                            p { class: "chip-row",
                                span { class: "muted", "Changes multisig" }
                                Address { pubkey: squad.address }
                                span { class: "muted", "stored by" }
                                Address { pubkey: change.creator }
                            }
                            MultisigChange {
                                change: change.clone(),
                                state: Some(state.clone()),
                                acting: member,
                                now,
                                settled: entry.stale
                                    || matches!(
                                        status,
                                        Some(ProposalStatus::Executed | ProposalStatus::Rejected | ProposalStatus::Cancelled)
                                    ),
                            }
                        },
                        Stored::Other => rsx! {
                            p { class: "warn", "This is a batch or a policy payload. This app cannot read those yet." }
                        },
                        Stored::Missing if entry.proposal.is_none() => rsx! {
                            p { class: "muted", "Its accounts were closed." }
                        },
                        Stored::Missing => rsx! {
                            p { class: "muted", "The account that held this transaction no longer exists." }
                        },
                    }
                    {simulate_block}
                    if let Some(standing) = standing.clone() {
                        ProposalVotes { standing, acting: member }
                    }
                    match (verdict, acting, combined) {
                        // Nobody to pay the fee: the modal's head says so, once.
                        (Some(_), Some((_, None)), Some(_)) => rsx! {},
                        (Some(verdict), Some((member, Some(payer))), Some(combined)) => {
                            let votes = combined.voters.len();
                            let together = ProposalRequest::Combined(combined);
                            rsx! {
                                div { class: "btn-row",
                                    button { class: "primary sm", disabled: busy,
                                        onclick: move |_| on_act.call((for_together.clone(), together.clone(), payer)),
                                        "Approve as {votes} members and execute"
                                    }
                                    if let Ok(action) = verdict {
                                        button { class: "sm", disabled: busy,
                                            onclick: move |_| on_act.call((
                                                for_action.clone(),
                                                ProposalRequest::Single { member, action },
                                                payer,
                                            )),
                                            {action_label(action)}
                                        }
                                    }
                                    {turn_down_button.clone()}
                                }
                                if let Err(reason) = verdict {
                                    if worth_a_line(&reason) {
                                        p { class: "hint", "{reason}" }
                                    }
                                }
                            }
                        }
                        (Some(Ok(action)), Some((member, Some(payer))), None) => rsx! {
                            div { class: "btn-row",
                                button { class: "primary sm", disabled: busy,
                                    onclick: move |_| on_act.call((
                                        for_action.clone(),
                                        ProposalRequest::Single { member, action },
                                        payer,
                                    )),
                                    {action_label(action)}
                                }
                                {turn_down_button.clone()}
                            }
                        },
                        (Some(Err(NoAction::MayNotExecute)), Some((_, Some(payer))), None) if instead.is_some() => {
                            let member = instead.unwrap_or_default();
                            rsx! {
                                div { class: "btn-row",
                                    button { class: "primary sm", disabled: busy,
                                        onclick: move |_| on_act.call((
                                            for_action.clone(),
                                            ProposalRequest::Single { member, action: ProposalAction::Execute },
                                            payer,
                                        )),
                                        "Execute"
                                    }
                                    {turn_down_button.clone()}
                                }
                                p { class: "chip-row hint",
                                    span { "Executed by" }
                                    Address { pubkey: member }
                                }
                            }
                        }
                        (Some(Ok(_)), _, _) => rsx! {},
                        // Nothing moves it forward, but it can be turned down:
                        // what could not be read, what the member approved, or
                        // what a time lock holds.
                        (Some(Err(reason)), _, _) if turn_down.is_some() => rsx! {
                            div { class: "btn-row", {turn_down_button.clone()} }
                            if worth_a_line(&reason) {
                                p { class: "hint", "{reason}" }
                            }
                        },
                        (Some(Err(reason)), _, _) => rsx! {
                            if worth_a_line(&reason) {
                                p { class: "hint", "{reason}" }
                            }
                        },
                        (None, _, _) => rsx! {},
                    }
                    match (&closing, payer) {
                        (Ok(closing), Some(payer)) => {
                            let request = ProposalRequest::Close(vec![closing.clone()]);
                            rsx! {
                                div { class: "btn-row",
                                    button { class: "sm", disabled: busy,
                                        onclick: move |_| on_act.call((for_close.clone(), request.clone(), payer)),
                                        "Close accounts"
                                    }
                                }
                            }
                        }
                        // Where a person would look for Close and not find
                        // it: the button is there, and says what is missing.
                        (Err(why), _) if why.worth_saying(&entry) => rsx! {
                            div { class: "btn-row",
                                button { class: "sm", disabled: true, title: SET_RENT_COLLECTOR,
                                    "Close accounts"
                                }
                            }
                        },
                        _ => rsx! {},
                    }
                }
            }
        }
    }
}
