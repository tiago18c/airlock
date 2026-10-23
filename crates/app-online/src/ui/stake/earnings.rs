//! What the stake accounts earned, epoch by epoch.

use super::*;

/// "0.0412%": what a reward added to the balance it was paid on.
pub(in crate::ui) fn reward_rate(reward: &StakeReward) -> Option<String> {
    let before = reward.post_balance.checked_sub(reward.amount)?;
    (before > 0).then(|| format!("{:.4}%", reward.amount as f64 * 100.0 / before as f64))
}

#[component]
pub(in crate::ui) fn Earnings(accounts: Vec<Pubkey>, epoch: u64) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.stake;
    let loading = (form.rewards_loading)();
    let asked = (form.reward_epochs)();
    let rewards = (form.rewards)();
    let total: u64 = rewards.iter().map(|reward| reward.amount).sum();
    let load = move |_| load_rewards(session, accounts.clone(), epoch);

    rsx! {
        div { class: "holdings",
            div { class: "book-head",
                div {
                    h3 { "Earnings" }
                    if asked.is_empty() {
                        p { class: "muted",
                            "Rewards are paid into each stake account when an epoch ends. An account that was closed or merged away no longer shows what it earned."
                        }
                    } else {
                        p { class: "muted", "{format_sol(total)} earned over the last {asked.len()} epochs." }
                    }
                }
                button { class: "sm", disabled: loading, onclick: load,
                    if loading {
                        "Reading…"
                    } else if asked.is_empty() {
                        "Show the last {REWARD_EPOCHS_PER_LOAD} epochs"
                    } else {
                        "{REWARD_EPOCHS_PER_LOAD} more epochs"
                    }
                }
            }
            if let Some(err) = (form.rewards_error)() {
                p { class: "error", "{err}" }
            }
            ul { class: "holding-list",
                for asked_epoch in asked {
                    EpochRow {
                        key: "{asked_epoch}",
                        epoch: asked_epoch,
                        rewards: rewards.iter().filter(|reward| reward.epoch == asked_epoch).copied().collect::<Vec<_>>(),
                    }
                }
            }
        }
    }
}

#[component]
pub(in crate::ui) fn EpochRow(epoch: u64, rewards: Vec<StakeReward>) -> Element {
    let total: u64 = rewards.iter().map(|reward| reward.amount).sum();
    rsx! {
        li { class: "holding-row",
            div {
                p { "Epoch {epoch}" }
                for reward in rewards.iter() {
                    p { class: "muted", key: "{reward.account}",
                        "{short_pubkey(&reward.account)} · +{format_sol(reward.amount)}"
                        if let Some(rate) = reward_rate(reward) {
                            " · {rate}"
                        }
                        if let Some(commission) = reward.commission {
                            " · validator commission {commission}%"
                        }
                    }
                }
                if rewards.is_empty() {
                    p { class: "muted", "Nothing was paid." }
                }
            }
            if total > 0 {
                span { class: "balance", "+{format_sol(total)}" }
            }
        }
    }
}
