//! One proposal: its status, what it stores, and what this device may do with it.

use super::*;

/// The button of what one member does to a proposal.
fn action_label(action: ProposalAction) -> &'static str {
    match action {
        ProposalAction::Approve => "Approve",
        ProposalAction::Execute => "Execute",
        ProposalAction::ApproveAndExecute => "Approve and execute",
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
    let session = use_context::<Session>();
    let index = entry.index;
    let (chip_class, chip_text) = status_chip(&entry);
    let approved = entry
        .proposal
        .as_ref()
        .map(|proposal| proposal.approved.clone())
        .unwrap_or_default();
    let voting = matches!(
        entry.proposal.as_ref().map(|proposal| proposal.status),
        Some(ProposalStatus::Active | ProposalStatus::Approved)
    );
    let mine = acting.is_some_and(|(member, _)| approved.contains(&member));
    let verdict = acting.map(|(member, _)| available_action(&state, &entry, &member, now));
    // Every vote the proposal still needs, cast by the wallets the row names.
    let combined = acting.and_then(|_| combined_approval(&state, &entry, &chosen, approval_nonce));
    let summary = match &entry.stored {
        Stored::Vault(tx) => match tx.instructions.len() {
            1 => "1 instruction".to_string(),
            count => format!("{count} instructions"),
        },
        Stored::LookupTables => "uses lookup tables".to_string(),
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
                if voting {
                    span { class: "chip", "{approved.len()} of {state.threshold} approved" }
                }
                if mine {
                    span { class: "chip ok", "you approved" }
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
                            InstructionList {
                                instructions: explain_instructions(&*session.idls.read(), &tx.instructions),
                                inspect: false,
                            }
                        },
                        Stored::LookupTables => rsx! {
                            p { class: "warn", "This transaction names accounts through address lookup tables. This app cannot resolve those yet, so it cannot show or execute it." }
                        },
                        Stored::Other => rsx! {
                            p { class: "warn", "This is a change to the multisig itself, a batch or a policy payload. This app cannot read those yet." }
                        },
                        Stored::Missing => rsx! {
                            p { class: "muted", "The account that held this transaction no longer exists." }
                        },
                    }
                    if !approved.is_empty() {
                        p { class: "chip-row",
                            span { class: "muted", "Approved by" }
                            for key in approved.iter().copied() {
                                Address { key: "{key}", pubkey: key }
                            }
                        }
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
                                    button { class: "primary", disabled: busy,
                                        onclick: move |_| on_act.call((for_together.clone(), together.clone(), payer)),
                                        "Approve as {votes} members and execute"
                                    }
                                    if let Ok(action) = verdict {
                                        button { disabled: busy,
                                            onclick: move |_| on_act.call((
                                                for_action.clone(),
                                                ProposalRequest::Single { member, action },
                                                payer,
                                            )),
                                            {action_label(action)}
                                        }
                                    }
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
                            }
                        }
                        (Some(Ok(action)), Some((member, Some(payer))), None) => rsx! {
                            div { class: "btn-row",
                                button { class: "primary", disabled: busy,
                                    onclick: move |_| on_act.call((
                                        for_action.clone(),
                                        ProposalRequest::Single { member, action },
                                        payer,
                                    )),
                                    {action_label(action)}
                                }
                            }
                            if action == ProposalAction::ApproveAndExecute {
                                p { class: "hint", "This approval is the last one {label} needs, so the same transaction executes it." }
                            }
                        },
                        (Some(Ok(_)), _, _) => rsx! {
                            p { class: "hint", "Pick a hot wallet to pay fees for this wallet first." }
                        },
                        (Some(Err(reason)), _, _) => rsx! {
                            p { class: "hint", "{reason}" }
                        },
                        (None, _, _) => rsx! {},
                    }
                }
            }
        }
    }
}
