//! Stake accounts: reading them, saying what state each is in, and building
//! the instructions that create, delegate, split, merge and empty them.
//!
//! Nothing here opens a socket. The app sends the requests this module
//! describes and hands back the JSON, so the reading can be tested on
//! recorded answers.

use crate::chain::RawAccount;
pub use crate::chain::{EpochInfo, StakeReward};
use crate::ids::STAKE_PROGRAM_ID;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use solana_stake_interface::instruction as stake_ix;
pub use solana_stake_interface::stake_history::StakeHistory;
use solana_stake_interface::state::{Authorized, Delegation, Lockup, StakeAuthorize, StakeStateV2};
use thiserror::Error;

mod account;
mod instructions;
mod snapshot;

#[cfg(test)]
mod tests;

pub use account::*;
pub use instructions::*;
pub use snapshot::*;

pub const STAKE_HISTORY_ID: Pubkey =
    Pubkey::from_str_const("SysvarStakeHistory1111111111111111111111111");

/// Bytes of every stake account.
pub const STAKE_ACCOUNT_LEN: u64 = StakeStateV2::size_of() as u64;

/// What a stake account has to hold to exist, at the rent every cluster
/// charges today. Used only when a node could not be asked.
pub const STAKE_RENT_LAMPORTS: u64 = 2_282_880;

/// Where the two authorities sit in a stake account, for `memcmp` filters.
const STAKER_OFFSET: usize = 12;
const WITHDRAWER_OFFSET: usize = 44;

/// The warmup rate has been 9% on every cluster for years. Naming epoch 0
/// applies it to the whole stake history.
const NEW_RATE_ACTIVATION_EPOCH: Option<u64> = Some(0);

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StakeError {
    #[error("Enter an amount in SOL, such as 1.5.")]
    Amount,
    #[error("Nothing can be withdrawn from this account yet.")]
    NothingToWithdraw,
    #[error("At most {available} lamports can be withdrawn now.")]
    TooMuch { available: u64 },
    #[error("Withdraw everything, or leave at least the rent reserve of {reserve} lamports.")]
    BelowReserve { reserve: u64 },
    #[error("A new stake account needs more than its rent reserve of {reserve} lamports.")]
    BelowRent { reserve: u64 },
    #[error("No address could be derived for the new stake account.")]
    Seed,
}

/// Read an amount typed in SOL, with up to nine decimals.
pub fn sol_to_lamports(text: &str) -> Result<u64, StakeError> {
    crate::units::parse_amount(text, 9).ok_or(StakeError::Amount)
}

// ---------- Instructions ----------

// ---------- Requests ----------
