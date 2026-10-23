//! The proposals of one Squads multisig: what is stored under each
//! transaction number, how far its approval has come, and what the member
//! this device acts as can do about it.
//!
//! A proposal is acted on as that one member, or, when the multisig's row
//! names other wallets that approve together with it and the proposal still
//! needs more than one of their votes, as all of them in one transaction
//! that also executes it.
//!
//! Nothing is signed here. Approve and Execute build a transaction and hand
//! it to the Sign screen, where it is read, signed and submitted like any
//! other. When a member in it is a programmatic signer, Sign rebuilds it
//! around the authorization its authority signs.

use crate::actions::proposals::{load_page, open_request};
use app_ui_kit::actions::unix_now;
use app_ui_kit::ui::tx_view::InstructionList;
use app_ui_kit::{Address, Session};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::proposals::{
    available_action, combined_approval, ChosenMember, ProposalAction, ProposalEntry,
    ProposalRequest, Stored,
};
use wallet_core::squads::{ProposalStatus, Squad, SquadState};
use wallet_core::{explain_instructions, RouteStep, WalletId};

mod host;
mod row;

pub use host::ProposalsHost;
pub(in crate::ui) use row::*;

/// What was read so far: the multisig as it is now, and its transactions
/// from the newest down.
#[derive(Clone, PartialEq)]
pub(in crate::ui) struct Loaded {
    pub(in crate::ui) state: SquadState,
    pub(in crate::ui) entries: Vec<ProposalEntry>,
}

impl Loaded {
    /// The transaction number the next page starts at.
    fn older(&self) -> Option<u64> {
        let last = self.entries.last().map(|entry| entry.index)?;
        (last > 1).then(|| last - 1)
    }
}

fn status_chip(entry: &ProposalEntry) -> (&'static str, &'static str) {
    match entry.proposal.as_ref().map(|proposal| proposal.status) {
        None => ("chip", "no proposal"),
        Some(ProposalStatus::Draft) => ("chip", "draft"),
        Some(ProposalStatus::Active) if entry.stale => ("chip warn", "stale"),
        Some(ProposalStatus::Active) => ("chip info", "open for votes"),
        Some(ProposalStatus::Approved) => ("chip ok", "approved"),
        Some(ProposalStatus::Executing) => ("chip info", "executing"),
        Some(ProposalStatus::Executed) => ("chip", "executed"),
        Some(ProposalStatus::Rejected) => ("chip danger", "rejected"),
        Some(ProposalStatus::Cancelled) => ("chip", "cancelled"),
    }
}

#[component]
pub fn ProposalsModal(
    wallet: WalletId,
    label: String,
    squad: Squad,
    on_close: EventHandler<()>,
) -> Element {
    let session = use_context::<Session>();
    let mut loaded = use_signal(|| None::<Loaded>);
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);
    let mut opened = use_signal(|| None::<u64>);
    let mut busy = use_signal(|| false);
    // A copy the action handler of every row can take.
    let label_for_note = use_signal(|| label.clone());

    // `from` is `None` to start over at the newest transaction.
    let mut fetch = move |from: Option<u64>| {
        if loading() {
            return;
        }
        loading.set(true);
        error.set(None);
        spawn(async move {
            let page = load_page(session, &squad, from).await;
            match page {
                Ok((state, entries)) => {
                    let mut all = match (from, loaded.peek().clone()) {
                        (Some(_), Some(before)) => before.entries,
                        _ => Vec::new(),
                    };
                    all.extend(entries);
                    loaded.set(Some(Loaded {
                        state,
                        entries: all,
                    }));
                }
                Err(err) => error.set(Some(err)),
            }
            loading.set(false);
        });
    };
    use_hook(|| fetch(None));

    // The member this device acts as, and who pays for what it does.
    let route = session.route_of(wallet);
    let acting = route.as_ref().ok().and_then(|route| {
        let member = match route.steps.first()? {
            RouteStep::Squads { member, .. } => *member,
            _ => return None,
        };
        Some((member, route.fee_payer.map(|(_, pubkey)| pubkey)))
    });
    let route_problem = route.as_ref().err().map(|err| err.to_string());
    let now = unix_now();
    let current = loaded();
    // The wallets the row names to approve together, as the chain knows them.
    let (chosen, approval_nonce) = current
        .as_ref()
        .map(|current| session.chosen_members(wallet, &current.state))
        .unwrap_or_default();
    let together: Vec<ChosenMember> = chosen.iter().skip(1).cloned().collect();

    rsx! {
        div { class: "modal-backdrop", onclick: move |_| on_close.call(()),
            div { class: "modal wide proposals", onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    div { class: "page-head",
                        div {
                            h3 { "Proposals of {label}" }
                            if let Some(current) = &current {
                                p { class: "muted",
                                    "{current.state.threshold} of {current.state.members.len()} approvals needed"
                                    if current.state.time_lock > 0 {
                                        " · {current.state.time_lock} s time lock"
                                    }
                                    " · {current.state.transaction_index} transaction(s)"
                                }
                            }
                        }
                        div { class: "btn-row",
                            button { class: "ghost sm", disabled: loading(),
                                onclick: move |_| fetch(None),
                                if loading() { "Reading…" } else { "Refresh" }
                            }
                            button { class: "ghost sm", onclick: move |_| on_close.call(()), "Close" }
                        }
                    }
                    match &acting {
                        Some((member, payer)) => rsx! {
                            p { class: "chip-row",
                                span { class: "muted", "Acting as member" }
                                Address { pubkey: *member }
                                if let Some(payer) = payer {
                                    span { class: "muted", "fees paid by" }
                                    Address { pubkey: *payer }
                                }
                            }
                            if !together.is_empty() {
                                p { class: "chip-row",
                                    span { class: "muted", "Approves together with" }
                                    for other in together.iter() {
                                        Address {
                                            key: "{other.address}",
                                            pubkey: other.address,
                                            warn: other.permissions == 0,
                                        }
                                    }
                                }
                                if together.iter().any(|other| other.permissions == 0) {
                                    p { class: "hint", "A wallet marked here is not a member of this multisig on this cluster, so it does not vote." }
                                }
                            }
                            if payer.is_none() {
                                p { class: "warn", "Nothing on this device can pay fees for this wallet. Pick a hot wallet on its row." }
                            }
                        },
                        None => rsx! {
                            p { class: "warn",
                                {route_problem.clone().unwrap_or_else(|| "Choose which wallet acts as a member on this wallet's row.".into())}
                            }
                        },
                    }
                    if let Some(text) = error() {
                        p { class: "error", role: "alert", "{text}" }
                    }
                    match current {
                        None if error().is_none() => rsx! {
                            p { class: "muted", role: "status", "Reading the multisig…" }
                        },
                        None => rsx! {},
                        Some(current) if current.entries.is_empty() => rsx! {
                            p { class: "muted", "This multisig has no transactions yet." }
                        },
                        Some(current) => {
                            let older = current.older();
                            let state = current.state.clone();
                            rsx! {
                                ul { class: "book-list",
                                    for entry in current.entries.iter().cloned() {
                                        ProposalRow {
                                            key: "{entry.index}",
                                            label: label.clone(),
                                            squad,
                                            state: state.clone(),
                                            entry: entry.clone(),
                                            acting,
                                            chosen: chosen.clone(),
                                            approval_nonce,
                                            now,
                                            open: opened() == Some(entry.index),
                                            busy: busy(),
                                            on_toggle: move |index: u64| {
                                                opened.set(if opened() == Some(index) { None } else { Some(index) });
                                            },
                                            on_act: move |(entry, request, payer): (ProposalEntry, ProposalRequest, Pubkey)| {
                                                let label = label_for_note();
                                                busy.set(true);
                                                error.set(None);
                                                spawn(async move {
                                                    match open_request(session, &squad, &entry, &request, payer, &label).await {
                                                        Ok(()) => on_close.call(()),
                                                        Err(err) => error.set(Some(err)),
                                                    }
                                                    busy.set(false);
                                                });
                                            },
                                        }
                                    }
                                }
                                if let Some(from) = older {
                                    div { class: "btn-row",
                                        button { disabled: loading(),
                                            onclick: move |_| fetch(Some(from)),
                                            if loading() { "Reading…" } else { "Older" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    p { class: "hint",
                        "Approve, Execute and Together open a transaction on the Sign screen. Nothing is signed here. Stored instructions are read with the IDLs on this device, which are not vetted yet."
                    }
                }
            }
        }
    }
}
