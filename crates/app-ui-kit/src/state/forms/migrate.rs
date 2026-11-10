//! Slot for the Migrate wallet modal, and the move it runs one step at a time:
//! Wallets opens it, the online shell mounts it.

use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::WalletId;
use wallet_flows::migrate::Choices;

/// A move under way: from where to where, what was chosen, how many of its
/// steps landed, and the log flow of the step on the Sign screen, so Submit
/// can offer the next one once that step lands.
#[derive(Clone, Debug, PartialEq)]
pub struct MigrateRun {
    pub from: WalletId,
    pub to: Pubkey,
    pub choices: Choices,
    /// Steps that landed before the one on the Sign screen.
    pub done: usize,
    pub flow: Option<u64>,
    /// How many steps were left when the step on screen was built. A read
    /// after it lands that finds as many left means it did not run yet: a
    /// multisig proposal that waits for more approvals.
    pub left: usize,
    /// Its last step landed as a proposal that has not executed yet.
    pub waiting: bool,
}

#[derive(Clone, Copy)]
pub struct MigrateForm {
    /// The modal is open, on this wallet when one was picked already.
    pub open: Signal<Option<Option<WalletId>>>,
    pub run: Signal<Option<MigrateRun>>,
    /// What to say at the top of the modal when it opens again.
    pub notice: Signal<Option<String>>,
    /// The last move that finished, from which wallet to where, so the
    /// modal opens on it again to say what is left to do.
    pub finished: Signal<Option<(WalletId, Pubkey)>>,
}
