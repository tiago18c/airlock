//! One stake account, with what can be done to it.

use super::*;

#[component]
pub(in crate::ui) fn StakeRow(
    target: Target,
    epoch: u64,
    mergeable: bool,
    on_action: EventHandler<Action>,
) -> Element {
    let account = target.stake.account.clone();
    let status = target.stake.status;
    let (chip_class, chip_text) = phase_chip(status.phase);
    let is_staker = account.has_role(&target.owner, StakeRole::Staker);
    let is_withdrawer = account.has_role(&target.owner, StakeRole::Withdrawer);
    let withdrawable = target.stake.withdrawable(epoch);
    let locked = account.lockup.in_force(epoch, unix_now());
    let validator = account.delegation.map(|delegation| delegation.voter);
    let state_line = match status.phase {
        StakePhase::Undelegated => "Not delegated: it earns nothing until it is.".to_string(),
        StakePhase::Activating => format!(
            "{} activating, {} earning. New stake starts earning at the next epoch.",
            format_sol(status.activating),
            format_sol(status.effective)
        ),
        StakePhase::Active => format!("{} earning.", format_sol(status.effective)),
        StakePhase::Deactivating => format!(
            "{} still earning until the epoch ends, then it can be withdrawn.",
            format_sol(status.effective)
        ),
        StakePhase::Inactive => "Deactivated: everything can be withdrawn.".to_string(),
    };
    let delegable = matches!(status.phase, StakePhase::Undelegated | StakePhase::Inactive);
    let deactivable = matches!(status.phase, StakePhase::Activating | StakePhase::Active);

    let open = move |kind: Kind| {
        let target = target.clone();
        move |_| {
            on_action.call(Action {
                kind,
                target: Some(target.clone()),
            })
        }
    };

    rsx! {
        li { class: "wallet-row",
            div { class: "wallet-head",
                AddressHeader {
                    pubkey: Some(account.pubkey),
                    title: "Stake account".to_string(),
                    subtitle: String::new(),
                    glyph: Glyph::Growth,
                    title_after: rsx! {
                        span { class: "{chip_class}", "{chip_text}" }
                    },
                }
                p { class: "balance", "{format_sol(account.lamports)}" }
            }
            p { class: "muted", "{state_line}" }
            if let Some(voter) = validator {
                p { class: "wallet-fact",
                    span { class: "muted",
                        if status.phase == StakePhase::Inactive { "Last validator" } else { "Validator" }
                    }
                    Address { pubkey: voter }
                }
            }
            if withdrawable > 0 && !matches!(status.phase, StakePhase::Undelegated | StakePhase::Inactive) {
                p { class: "muted", "{format_sol(withdrawable)} is above the stake and can be withdrawn." }
            }
            if !is_staker || !is_withdrawer {
                div { class: "chip-row",
                    if is_staker {
                        span { class: "muted", "Stake authority only. Withdrawals belong to" }
                        Address { pubkey: account.withdrawer }
                    } else {
                        span { class: "muted", "Withdraw authority only. Delegation belongs to" }
                        Address { pubkey: account.staker }
                    }
                }
            }
            if locked {
                p { class: "warn",
                    "Locked up until epoch {account.lockup.epoch}. Until then a withdrawal also needs the signature of the custodian "
                    Address { pubkey: account.lockup.custodian }
                    ", which this screen does not ask for."
                }
            }
            div { class: "btn-row",
                if is_staker && delegable {
                    button { class: "sm", onclick: open(Kind::Delegate), "Delegate" }
                }
                if is_staker && deactivable {
                    button { class: "sm", onclick: open(Kind::Deactivate), "Deactivate" }
                }
                if is_withdrawer && withdrawable > 0 {
                    button { class: "sm", onclick: open(Kind::Withdraw), "Withdraw" }
                }
                if is_staker {
                    button { class: "sm", onclick: open(Kind::Split), "Split" }
                }
                if is_staker && mergeable {
                    button { class: "sm", onclick: open(Kind::Merge), "Merge" }
                }
                button { class: "sm ghost", onclick: open(Kind::Authorize), "Authorities" }
            }
        }
    }
}
