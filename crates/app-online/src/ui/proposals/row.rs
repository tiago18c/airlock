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
//!
//! A change to the multisig itself reads as its actions in words, with what
//! the multisig looks like after it and what to look out for
//! (`app_ui_kit::ui::MultisigChange`), where a vault transaction lists its
//! instructions. It is approved and executed the same way.
//!
//! Once the program allows it (`available_close`), a row also offers to
//! close the proposal's accounts: a quiet button, since it is housekeeping,
//! whose hint says where the rent goes, how much, and that the proposal's
//! record leaves the chain. No member signs it, only the wallet paying the
//! fee. Where someone would wonder why a row cannot be closed, it says why.
//! A transaction whose accounts were closed reads as closed.

use super::votes::{vote_chips, ProposalVotes};
use super::*;
use app_ui_kit::format_sol;
use app_ui_kit::ui::MultisigChange;
use wallet_core::proposals::{
    activation_hint, available_close, standing, Closing, NoClose, DRAFT_CANNOT_BE_CANCELLED,
};
use wallet_core::squads::change_summary;

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
        Stored::Config(change) => change_summary(&change.actions),
        Stored::Other => "not read".to_string(),
        Stored::Missing if entry.proposal.is_none() => "accounts closed".to_string(),
        Stored::Missing => "transaction account closed".to_string(),
    };
    // Closing its accounts: the fee payer alone signs, so it is offered
    // with someone to pay, whoever the acting member is.
    let closing = available_close(squad.program, &state, &entry);
    let for_close = entry.clone();
    let finished = matches!(verdict, Some(Err(NoAction::Finished)));
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
                            p { class: "muted", "Its accounts were closed and their rent returned. What it proposed and who voted on it are no longer on chain." }
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
                            // A refused change says why above, with its addresses.
                            let said_above = matches!(reason, NoAction::Refused(_));
                            let needs_no_reading = match turn_down.as_ref().map(|(action, _)| *action) {
                                Some(ProposalAction::Cancel) => " Voting to cancel it needs no reading.",
                                _ => " Rejecting it needs no reading.",
                            };
                            rsx! {
                                div { class: "btn-row", {turn_down_button.clone()} }
                                if !said_above {
                                    p { class: "hint",
                                        "{reason}"
                                        if unread { "{needs_no_reading}" }
                                    }
                                }
                                if let Some(hint) = turn_down_hint.clone() {
                                    p { class: "hint", "{hint}" }
                                }
                            }
                        }
                        (Some(Err(reason)), _, _) => rsx! {
                            // "Nothing is left to do" is not so beside Close.
                            if !matches!(reason, NoAction::Refused(_)) && !(finished && closing.is_ok() && payer.is_some()) {
                                p { class: "hint", "{reason}" }
                            }
                            // For someone looking for a way to cancel a draft.
                            if draft && reason == NoAction::Draft {
                                p { class: "hint", "{DRAFT_CANNOT_BE_CANCELLED}" }
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
                                        title: "Removes this proposal's record from the chain and returns its rent",
                                        onclick: move |_| on_act.call((for_close.clone(), request.clone(), payer)),
                                        "Close accounts"
                                    }
                                }
                                RentReturned { closings: vec![closing.clone()], payer, one: true }
                            }
                        }
                        (Err(why), _) if why.worth_saying(&entry) && !matches!(why, NoClose::Gone) => rsx! {
                            p { class: "hint", "{why}" }
                        },
                        _ => rsx! {},
                    }
                }
            }
        }
    }
}

/// Where the rent of `closings` goes and how much, said before Close is
/// pressed, and what closing costs: the record of what was proposed. `one`
/// for a single proposal's accounts.
#[component]
pub(in crate::ui) fn RentReturned(closings: Vec<Closing>, payer: Pubkey, one: bool) -> Element {
    let returns = wallet_core::proposals::rent_returned(&closings);
    let to_payer = returns.iter().any(|(collector, _)| *collector == payer);
    let only_payer = to_payer && returns.len() == 1;
    let record = if one {
        "It removes this proposal's record from the chain: what it proposed and who voted."
    } else {
        "It removes these proposals' records from the chain: what they proposed and who voted."
    };
    let payer_words = if only_payer {
        "That is the wallet paying the fee. No member signs a close."
    } else if to_payer {
        "No member signs a close."
    } else {
        "The rent does not go to the wallet paying the fee, and no member signs a close."
    };
    rsx! {
        p { class: "chip-row hint",
            span { "Closing returns" }
            for (at, (collector, lamports)) in returns.into_iter().enumerate() {
                span { key: "{collector}", class: "proposal-voter",
                    span {
                        if at == 0 { "{format_sol(lamports)} of rent to" } else { "and {format_sol(lamports)} to" }
                    }
                    Address { pubkey: collector }
                }
            }
        }
        p { class: "hint", "{record} {payer_words}" }
    }
}
