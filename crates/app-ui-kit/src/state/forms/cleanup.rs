//! Slot for the Burn and close modal, and the cleanup it runs one step at a
//! time: any screen opens it, the online shell mounts it.

use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;
use wallet_core::token_plan::Choice;
use wallet_core::WalletId;

/// A cleanup under way: what was chosen, how many of its steps landed, and
/// the log flow of the step on the Sign screen, so Submit can offer the next
/// one once that step lands.
#[derive(Clone, Debug, PartialEq)]
pub struct CleanupRun {
    pub wallet: WalletId,
    pub chosen: BTreeMap<Pubkey, Choice>,
    /// Steps that landed before the one on the Sign screen.
    pub done: usize,
    pub flow: Option<u64>,
}

#[derive(Clone, Copy)]
pub struct CleanupForm {
    /// The wallet whose token accounts the modal shows, while it is open.
    pub open: Signal<Option<WalletId>>,
    pub run: Signal<Option<CleanupRun>>,
    /// What to say at the top of the modal when it opens again: that a step
    /// landed, or that the next one could not be made.
    pub notice: Signal<Option<String>>,
}
