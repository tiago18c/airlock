//! One proposal: its status, how its votes stand, what it stores, and what
//! this device may do with it.
//!
//! What the acting member may do comes from `wallet_core::proposals`
//! (`member_choices`): the action that moves the proposal forward, which is
//! the primary button (Open for votes on a draft, Approve on an open
//! proposal, Execute on an approved one), and the careful choice beside it:
//! Reject on an open proposal, Vote to cancel on an approved one. That one is
//! never the primary: a danger-outlined button whose hint says what it does,
//! such as rejecting or cancelling the proposal for good, before it is
//! pressed. A draft has no such choice, since neither program rejects or
//! cancels one, and its row says so. Every button opens a transaction in
//! Review and Sign.

use super::votes::{vote_chips, ProposalVotes};
use super::*;
use wallet_core::proposals::{activation_hint, standing, DRAFT_CANNOT_BE_CANCELLED};

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
    label: String,
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
    let verdict = choices.as_ref().map(|choices| choices.forward.clone());
    let payer = acting.and_then(|(_, payer)| payer);
    // The careful choice: Reject on an open proposal, Vote to cancel on an
    // approved one. Offered only with someone to pay for it, like every
    // other action.
    let turn_down = choices
        .as_ref()
        .and_then(|choices| choices.turn_down())
        .filter(|_| payer.is_some());
    let turn_down_hint = turn_down.as_ref().map(|(_, hint)| hint.clone());
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
    let draft = status == Some(ProposalStatus::Draft);
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
        Stored::Other => "not a vault transaction".to_string(),
        Stored::Missing => "transaction account closed".to_string(),
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
                            if !tx.lookup_tables.is_empty() {
                                p { class: "chip-row",
                                    span { class: "muted",
                                        if tx.lookup_tables.len() == 1 { "Reads accounts from lookup table" } else { "Reads accounts from lookup tables" }
                                    }
                                    for table in tx.lookup_tables.iter().copied() {
                                        Address { key: "{table}", pubkey: table, warn: tx.deactivated_tables.contains(&table) }
                                    }
                                }
                                if tx.deactivated_tables.is_empty() {
                                    p { class: "hint", "A lookup table's entries never change once written, so what is shown here is what runs." }
                                } else {
                                    p { class: "warn", "A table marked here was deactivated by its authority. It still works until it is closed, and once it is, this transaction can never be executed." }
                                }
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
                        Stored::Other => rsx! {
                            p { class: "warn", "This is a change to the multisig itself, a batch or a policy payload. This app cannot read those yet." }
                        },
                        Stored::Missing => rsx! {
                            p { class: "muted", "The account that held this transaction no longer exists." }
                        },
                    }
                    if let (Some(standing), Some(status)) = (standing.clone(), status) {
                        ProposalVotes { standing, status, acting: member }
                    }
                    match (verdict, acting, combined) {
                        (Some(_), Some((_, None)), Some(_)) => rsx! {
                            p { class: "hint", "Pick a hot wallet to pay fees for this wallet first." }
                        },
                        (Some(verdict), Some((member, Some(payer))), Some(combined)) => {
                            let votes = combined.voters.len();
                            let voters = combined.voters.clone();
                            let executor = Some(combined.executor).filter(|key| !voters.contains(key));
                            let offline = combined.durable_nonce.is_some();
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
                                p { class: "chip-row hint",
                                    span { "Together casts the {votes} approvals this proposal still needs in one transaction and executes it:" }
                                    for key in voters.iter().copied() {
                                        Address { key: "{key}", pubkey: key }
                                    }
                                    if let Some(executor) = executor {
                                        span { "It is executed by" }
                                        Address { pubkey: executor }
                                    }
                                }
                                if offline {
                                    p { class: "hint", "Each offline wallet signs the same transaction on its own device." }
                                }
                                match verdict {
                                    Ok(_) => rsx! {
                                        p { class: "hint", "Separate approves as the acting member only." }
                                    },
                                    Err(reason) => rsx! {
                                        p { class: "hint", "{reason}" }
                                    },
                                }
                                if let Some(hint) = turn_down_hint.clone() {
                                    p { class: "hint", "{hint}" }
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
                            if action == ProposalAction::ApproveAndExecute {
                                p { class: "hint", "This approval is the last one {label} needs, so the same transaction executes it." }
                            }
                            if action == ProposalAction::Activate {
                                p { class: "hint", {activation_hint(&entry)} }
                            }
                            if let Some(hint) = turn_down_hint.clone() {
                                p { class: "hint", "{hint}" }
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
                                    span { "The acting member may not execute, so" }
                                    Address { pubkey: member }
                                    span { "does: another wallet of this device that this multisig lets execute." }
                                }
                                if let Some(hint) = turn_down_hint.clone() {
                                    p { class: "hint", "{hint}" }
                                }
                            }
                        }
                        (Some(Ok(_)), _, _) => rsx! {
                            p { class: "hint", "Pick a hot wallet to pay fees for this wallet first." }
                        },
                        // Nothing moves it forward, but it can be turned down:
                        // what could not be read, what the member approved, or
                        // what a time lock holds.
                        (Some(Err(reason)), _, _) if turn_down.is_some() => {
                            let unread = matches!(reason, NoAction::Unreadable | NoAction::Gone);
                            let needs_no_reading = match turn_down.as_ref().map(|(action, _)| *action) {
                                Some(ProposalAction::Cancel) => " Voting to cancel it needs no reading.",
                                _ => " Rejecting it needs no reading.",
                            };
                            rsx! {
                                div { class: "btn-row", {turn_down_button.clone()} }
                                p { class: "hint",
                                    "{reason}"
                                    if unread { "{needs_no_reading}" }
                                }
                                if let Some(hint) = turn_down_hint.clone() {
                                    p { class: "hint", "{hint}" }
                                }
                            }
                        }
                        (Some(Err(reason)), _, _) => rsx! {
                            p { class: "hint", "{reason}" }
                            // For someone looking for a way to cancel a draft.
                            if draft && reason == NoAction::Draft {
                                p { class: "hint", "{DRAFT_CANNOT_BE_CANCELLED}" }
                            }
                        },
                        (None, _, _) => rsx! {},
                    }
                }
            }
        }
    }
}
