//! The Stake screen: what it read from the chain.

use super::*;

/// What the Stake screen read from the chain. Kept here so it survives a
/// visit to another screen. Nothing is stored on disk.
#[derive(Clone, Copy)]
pub struct StakeForm {
    pub snapshot: Signal<Option<wallet_core::stake::StakeSnapshot>>,
    /// The endpoint, the addresses and the last submitted transaction the
    /// snapshot was read for. A different one means it is out of date.
    pub loaded_for: Signal<Option<String>>,
    pub loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    /// Newest epoch first.
    pub rewards: Signal<Vec<wallet_core::stake::StakeReward>>,
    /// Epochs that were asked about, newest first, whether or not they paid.
    pub reward_epochs: Signal<Vec<u64>>,
    pub rewards_loading: Signal<bool>,
    pub rewards_error: Signal<Option<String>>,
}

impl StakeForm {
    pub fn clear_rewards(&self) {
        let mut this = *self;
        this.rewards.set(Vec::new());
        this.reward_epochs.set(Vec::new());
        this.rewards_error.set(None);
    }
}
