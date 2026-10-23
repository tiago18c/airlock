//! Wallets that are not a key on this device: offline wallets, programmatic
//! signers and Squads multisigs. Also the switcher that picks which wallet
//! the app acts as.

use super::CHANGE_WALLET;
use crate::actions::wallets::member_candidates;
use crate::state::forms::SendOpen;
use crate::state::session::{Session, WalletSummary};
use crate::ui::{Address, AddressHeader, AddressRow, AddressSelect, CopyButton, WalletPicker};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use wallet_core::programmatic::programmatic_signer_address;
use wallet_core::squads::{Squad, SquadsProgram};
use wallet_core::{
    Authority, Category, KeypairSigner, NewSigner, Origin, WalletClass, WalletId, WalletKind,
    WalletRecord, WalletSigner,
};

mod checklist;
mod contract_row;
mod key_share;
mod offline_form;
mod programmatic_form;
mod squads_form;
mod switcher;
mod together;
mod vector_form;
mod vector_row;

pub(in crate::ui) use checklist::*;
pub use contract_row::*;
pub use key_share::*;
pub use offline_form::*;
pub use programmatic_form::*;
pub use squads_form::*;
pub use switcher::*;
pub(in crate::ui) use together::*;
pub use vector_form::*;
pub use vector_row::use_vector_reads;
use vector_row::VectorFacts;

pub fn find_id(wallets: &[WalletSummary], value: &str) -> Option<WalletId> {
    wallets
        .iter()
        .find(|wallet| wallet.id.to_string() == value)
        .map(|wallet| wallet.id)
}

pub(in crate::ui) fn hot(wallets: &[WalletSummary]) -> Vec<WalletSummary> {
    wallets
        .iter()
        .filter(|wallet| wallet.class == WalletClass::Hot)
        .cloned()
        .collect()
}

/// Wallets that can pay fees for another. In the offline app, which never
/// submits, an offline wallet can too: its key is on the online device,
/// which signs as the fee payer and submits.
pub(in crate::ui) fn payers(wallets: &[WalletSummary], offline_app: bool) -> Vec<WalletSummary> {
    if offline_app {
        keys(wallets)
    } else {
        hot(wallets)
    }
}

/// Wallets that are a key: the hot ones, and the offline ones.
fn keys(wallets: &[WalletSummary]) -> Vec<WalletSummary> {
    wallets
        .iter()
        .filter(|wallet| matches!(wallet.class, WalletClass::Hot | WalletClass::Offline))
        .cloned()
        .collect()
}

/// Marks the wallet the app is acting as.
#[component]
pub(in crate::ui) fn CurrentChip() -> Element {
    rsx! {
        span { class: "chip ok", title: "The wallet the app is acting as", "Current" }
    }
}
