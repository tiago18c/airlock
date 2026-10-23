//! Stake accounts of this device's wallets, what state each is in, and the
//! operations on them. Every operation is compiled here and opened in Sign.

use crate::actions::stake_screen::*;
use app_ui_kit::actions::unix_now;
use app_ui_kit::{
    format_sol, Address, AddressHeader, AddressPicker, Glyph, Icon, VaultUnlock, WalletPicker,
};
use app_ui_kit::{Session, WalletSummary};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::stake::{
    sol_to_lamports, ListedStake, StakePhase, StakeReward, StakeRole, StakeSnapshot,
    STAKE_RENT_LAMPORTS,
};
use wallet_core::{short_pubkey, WalletId};
use wallet_flows::stake::StakeRequest;

mod action_modal;
mod earnings;
mod row;

pub(in crate::ui) use action_modal::*;
pub(in crate::ui) use earnings::*;
pub(in crate::ui) use row::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum Kind {
    New,
    Delegate,
    Deactivate,
    Withdraw,
    Split,
    Merge,
    Authorize,
}

/// The stake account an operation is about, and the wallet that acts on it.
#[derive(Clone, PartialEq)]
pub(in crate::ui) struct Target {
    pub(in crate::ui) wallet: WalletId,
    pub(in crate::ui) owner: Pubkey,
    pub(in crate::ui) stake: ListedStake,
}

#[derive(Clone, PartialEq)]
pub(in crate::ui) struct Action {
    pub(in crate::ui) kind: Kind,
    /// `None` only for [`Kind::New`].
    pub(in crate::ui) target: Option<Target>,
}

#[component]
pub fn StakePanel() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.stake;
    let mut action = use_signal(|| None::<Action>);

    // Read again when the endpoint or the wallets changed, or a transaction
    // was submitted since the last read.
    use_effect(move || {
        if session.is_initialized() && (form.loaded_for)() != Some(load_key(session)) {
            refresh(session);
        }
    });

    if !session.is_initialized() {
        return rsx! { VaultUnlock { show_skip: false } };
    }

    let loading = (form.loading)();
    let wallets = owners(session);
    let snapshot = (form.snapshot)();
    let epoch = snapshot
        .as_ref()
        .map(|found| found.epoch.epoch)
        .unwrap_or(0);
    let groups: Vec<(WalletSummary, Pubkey, Vec<ListedStake>)> = wallets
        .iter()
        .filter_map(|wallet| {
            let address = wallet.address?;
            let stakes = snapshot.as_ref()?.of(&address);
            (!stakes.is_empty()).then(|| (wallet.clone(), address, stakes))
        })
        .collect();
    let without: Vec<String> = wallets
        .iter()
        .filter(|wallet| !groups.iter().any(|(listed, ..)| listed.id == wallet.id))
        .map(|wallet| wallet.label.clone())
        .collect();
    let failed: Vec<String> = snapshot
        .as_ref()
        .map(|found| {
            wallets
                .iter()
                .filter(|wallet| {
                    wallet
                        .address
                        .is_some_and(|address| found.failed.contains(&address))
                })
                .map(|wallet| wallet.label.clone())
                .collect()
        })
        .unwrap_or_default();

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "pane-head",
                    div {
                        h2 { "Stake" }
                    }
                    div { class: "btn-row",
                        button {
                            disabled: loading,
                            onclick: move |_| refresh(session),
                            if loading { "Reading…" } else { "Refresh" }
                        }
                        button { class: "primary",
                            disabled: wallets.is_empty(),
                            onclick: move |_| action.set(Some(Action { kind: Kind::New, target: None })),
                            Icon { glyph: Glyph::Plus }
                            "New stake"
                        }
                    }
                }

                if let Some(err) = (form.error)() {
                    p { class: "error", "{err}" }
                }
                if wallets.is_empty() {
                    div { class: "empty",
                        Icon { glyph: Glyph::Growth }
                        p { "Add a wallet first. Stake accounts are found through the wallets that control them." }
                    }
                }

                if let Some(found) = snapshot.clone() {
                    Totals { snapshot: found.clone() }
                    if !failed.is_empty() {
                        p { class: "warn",
                            "The node did not list the stake accounts of {failed.join(\", \")}. Press Refresh to ask again."
                        }
                    }
                    if groups.is_empty() && !wallets.is_empty() {
                        div { class: "empty",
                            Icon { glyph: Glyph::Growth }
                            p { "No stake accounts yet. New stake creates one and delegates it to a validator." }
                        }
                    }
                    for (wallet, address, stakes) in groups {
                        div { class: "stack", key: "{wallet.id}",
                            h3 { class: "group-title", "{wallet.label}" }
                            ul { class: "tree",
                                for stake in stakes {
                                    StakeRow {
                                        key: "{stake.account.pubkey}",
                                        target: Target { wallet: wallet.id, owner: address, stake: stake.clone() },
                                        epoch,
                                        mergeable: found.stakes.iter().any(|other| {
                                            stake.account.can_absorb(&stake.status, &other.account, &other.status, epoch)
                                        }),
                                        on_action: move |chosen| action.set(Some(chosen)),
                                    }
                                }
                            }
                        }
                    }
                    if !found.stakes.is_empty() && !without.is_empty() {
                        p { class: "muted", "No stake accounts: {without.join(\", \")}." }
                    }
                    if !found.stakes.is_empty() {
                        Earnings { accounts: found.addresses(), epoch }
                    }
                } else if loading {
                    p { class: "muted", "Reading stake accounts…" }
                }
            }
        }
        if let Some(current) = action() {
            ActionModal { action: current, on_close: move |_| action.set(None) }
        }
    }
}

#[component]
fn Totals(snapshot: StakeSnapshot) -> Element {
    let epoch = snapshot.epoch;
    let sum =
        |amount: &dyn Fn(&ListedStake) -> u64| -> u64 { snapshot.stakes.iter().map(amount).sum() };
    let total = sum(&|stake| stake.account.lamports);
    let active = sum(&|stake| stake.status.effective);
    let activating = sum(&|stake| stake.status.activating);
    let withdrawable = sum(&|stake| stake.withdrawable(epoch.epoch));
    rsx! {
        div { class: "stats",
            div { class: "stat",
                span { class: "stat-label", "In stake accounts" }
                span { class: "stat-value", "{format_sol(total)}" }
            }
            div { class: "stat",
                span { class: "stat-label", "Earning" }
                span { class: "stat-value", "{format_sol(active)}" }
            }
            if activating > 0 {
                div { class: "stat",
                    span { class: "stat-label", "Activating" }
                    span { class: "stat-value", "{format_sol(activating)}" }
                }
            }
            div { class: "stat",
                span { class: "stat-label", "Withdrawable" }
                span { class: "stat-value", "{format_sol(withdrawable)}" }
            }
            div { class: "stat",
                span { class: "stat-label", "Epoch" }
                span { class: "stat-value", "{epoch.epoch} · {epoch.percent()}%" }
            }
        }
    }
}

fn phase_chip(phase: StakePhase) -> (&'static str, &'static str) {
    match phase {
        StakePhase::Undelegated => ("chip", "Not delegated"),
        StakePhase::Activating => ("chip info", "Activating"),
        StakePhase::Active => ("chip ok", "Active"),
        StakePhase::Deactivating => ("chip warn", "Deactivating"),
        StakePhase::Inactive => ("chip", "Inactive"),
    }
}
