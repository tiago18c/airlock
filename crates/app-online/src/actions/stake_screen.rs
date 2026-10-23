//! Reading stake accounts and their rewards into the Stake screen's state.

use app_ui_kit::actions::spawn_app_task;
use app_ui_kit::{Session, WalletSummary};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
/// How many epochs of rewards one press asks for. Each is one request.
pub const REWARD_EPOCHS_PER_LOAD: u64 = 5;

/// The wallets whose stake accounts are listed: every one with an address.
pub fn owners(session: Session) -> Vec<WalletSummary> {
    session
        .wallet_summaries()
        .into_iter()
        .filter(|wallet| wallet.address.is_some())
        .collect()
}

/// What a snapshot was read for. A different value means it is out of date.
pub fn load_key(session: Session) -> String {
    let url = session.rpc_url();
    let addresses: Vec<String> = owners(session)
        .iter()
        .filter_map(|wallet| wallet.address)
        .map(|address| address.to_string())
        .collect();
    let submitted = (session.forms.sign.last_signature)().unwrap_or_default();
    format!("{url}|{}|{submitted}", addresses.join(","))
}

pub fn refresh(session: Session) {
    let form = session.forms.stake;
    let mut loading = form.loading;
    let mut error = form.error;
    let mut snapshot = form.snapshot;
    let mut loaded_for = form.loaded_for;
    if *loading.peek() {
        return;
    }
    let key = load_key(session);
    let addresses: Vec<Pubkey> = owners(session)
        .iter()
        .filter_map(|wallet| wallet.address)
        .collect();
    error.set(None);
    if addresses.is_empty() {
        snapshot.set(None);
        loaded_for.set(Some(key));
        return;
    }
    let rpc = session.rpc();
    loading.set(true);
    // UI follow-up: writes Stake form signals; must outlive leaving the pane.
    spawn_app_task(async move {
        let read = match rpc {
            Ok(rpc) => wallet_flows::stake::fetch_stakes(rpc.as_ref(), &addresses).await,
            Err(err) => Err(err),
        };
        match read {
            Ok(found) => {
                if found.lists_other_accounts_than(snapshot.peek().as_ref()) {
                    form.clear_rewards();
                }
                // Each validator stake is delegated to goes in the book, so it
                // reads as a validator wherever it appears, and can be named.
                let voters: Vec<Pubkey> = found
                    .stakes
                    .iter()
                    .filter_map(|listed| listed.account.delegation.map(|d| d.voter))
                    .filter(|voter| (session.book)().get(voter).is_none())
                    .collect();
                if !voters.is_empty() {
                    session.edit_workspace(|ws| {
                        for voter in &voters {
                            ws.book.insert_discovered(*voter, wallet_core::AddressKind::Validator);
                        }
                    });
                }
                snapshot.set(Some(found));
            }
            Err(err) => error.set(Some(format!(
                "Stake accounts could not be read: {err}. Some RPC providers do not answer getProgramAccounts. A custom RPC URL can be set in Settings."
            ))),
        }
        // A failed read is not repeated until Refresh is pressed.
        loaded_for.set(Some(key));
        loading.set(false);
    });
}

pub fn load_rewards(session: Session, accounts: Vec<Pubkey>, current_epoch: u64) {
    let form = session.forms.stake;
    let mut loading = form.rewards_loading;
    let mut error = form.rewards_error;
    let mut rewards = form.rewards;
    let mut asked = form.reward_epochs;
    if *loading.peek() {
        return;
    }
    // The epoch before the oldest one asked about, or the last finished one.
    let newest = asked.peek().last().copied().unwrap_or(current_epoch);
    let rpc = session.rpc();
    error.set(None);
    loading.set(true);
    // UI follow-up: writes rewards form signals; must outlive leaving the pane.
    spawn_app_task(async move {
        for back in 1..=REWARD_EPOCHS_PER_LOAD {
            let Some(epoch) = newest.checked_sub(back) else {
                break;
            };
            let paid = match &rpc {
                Ok(rpc) => rpc.get_stake_rewards(&accounts, epoch).await,
                Err(err) => Err(err.clone()),
            };
            match paid {
                Ok(found) => {
                    rewards.write().extend(found);
                    asked.write().push(epoch);
                }
                Err(err) => {
                    error.set(Some(format!(
                        "The node has no rewards for epoch {epoch}: {err}"
                    )));
                    break;
                }
            }
        }
        loading.set(false);
    });
}
