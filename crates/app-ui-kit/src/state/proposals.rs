//! Slot for the Proposals modal: Wallets opens it; the online shell mounts it.

use wallet_core::squads::Squad;
use wallet_core::WalletId;

/// What Wallets asked to browse. Cleared when the modal closes.
#[derive(Clone, PartialEq)]
pub struct ProposalsOpen {
    pub wallet: WalletId,
    pub label: String,
    pub squad: Squad,
}
