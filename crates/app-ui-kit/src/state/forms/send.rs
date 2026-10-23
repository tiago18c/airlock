//! Slot for the Send modal: any screen opens it, the online shell mounts it.

use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::WalletId;

/// What a send moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendAsset {
    Sol,
    Token(Pubkey),
}

/// What the screen that opened the modal already knows. Every field can
/// still be changed in the modal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SendOpen {
    pub from: Option<WalletId>,
    pub asset: Option<SendAsset>,
    pub to: Option<Pubkey>,
}

#[derive(Clone, Copy)]
pub struct SendForm {
    /// Open while `Some`. Cleared when the modal closes.
    pub open: Signal<Option<SendOpen>>,
}
