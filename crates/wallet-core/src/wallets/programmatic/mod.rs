//! The Ed25519 programmatic signer: sign now, let someone else submit later.
//!
//! Three programs work together (`github.com/solana-program/ed25519-programmatic-signer`):
//!
//! - The **signer** program verifies an authority's Ed25519 signature over an
//!   *authorization message* and, when it holds, signs as that authority's
//!   derived address (its *programmatic signer*) for one call into the executor.
//! - The **executor** program runs every instruction of an *execution message*,
//!   after consuming a nonce so the same message cannot run twice.
//! - The **nonce** program holds that nonce.
//!
//! So a transaction has three layers:
//!
//! ```text
//! relay transaction      fee payer signs; has a recent blockhash
//! └─ Submit              authority signatures + authorization message
//!    └─ Execute          the authorization message's only instruction
//!       └─ instructions  the execution message, signed for by the derived address
//! ```
//!
//! The authority's signature covers the authorization message only. That
//! message has no blockhash and names no fee payer, so it can be signed on an
//! offline device at any time and relayed by any funded wallet.

use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_message::v1::{self, TransactionConfig, MAX_ADDRESSES, MAX_SIGNATURES};
use solana_message::{AccountKeys, MessageHeader};
use solana_pubkey::Pubkey;
use solana_signature::{Signature, SIGNATURE_BYTES};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub use crate::idl::builtin::ED25519_SIGNER_PROGRAM_ID;

mod authorization;
mod nonce;
mod submit;

#[cfg(test)]
mod tests;

pub use authorization::*;
pub use nonce::*;
pub use submit::*;

pub const MESSAGE_EXECUTOR_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ExecxgyHYsAXB4c5dZodV1zJZ9hqfsDCYkRDRATrpkFR");
pub const EXECUTOR_NONCE_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("Noncediea1fH12usShuQAz28UhgAeuE5Maf32LsMUQB");
const SLOT_HASHES_SYSVAR_ID: Pubkey =
    Pubkey::from_str_const("SysvarS1otHashes111111111111111111111111111");

/// Size of an executor nonce account: nonce, authority, slot it was initialized at.
pub const EXECUTOR_NONCE_LEN: usize = 72;

/// Lamports that are enough to keep an executor nonce account alive on any
/// cluster: two years of rent, at the highest rate Solana has charged, for its
/// data plus the 128 bytes every account is charged for. A cluster with a
/// lower rate needs less. The surplus stays in the account and comes back
/// when it is closed.
pub const EXECUTOR_NONCE_RENT_LAMPORTS: u64 = (128 + EXECUTOR_NONCE_LEN as u64) * 3_480 * 2;

const PROGRAMMATIC_SIGNER_SEED: &[u8] = b"programmatic-signer";
const NONCE_STEP_TAG: &[u8] = b"spl-nonce::step::v1";
const SUBMIT_TAG: u8 = 0;
const EXECUTE_TAG: u8 = 0;
const NONCE_INITIALIZE_TAG: u8 = 0;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProgrammaticError {
    #[error("Add at least one instruction.")]
    NoInstructions,
    #[error("Add at least one authority.")]
    NoAuthorities,
    #[error("{authority} signs for {derived}, which no instruction needs as a signer and which is not the nonce authority.")]
    UnusedAuthority { authority: Pubkey, derived: Pubkey },
    #[error("This needs {count} signers. One authorization holds at most {max}.", max = MAX_SIGNATURES)]
    TooManySigners { count: usize },
    #[error("This touches {count} accounts. One authorization holds at most {max}.", max = MAX_ADDRESSES)]
    TooManyAccounts { count: usize },
    #[error("The signature from {0} is missing.")]
    MissingSignature(Pubkey),
    #[error("The signature from {0} does not match this authorization.")]
    BadSignature(Pubkey),
    #[error("{0} is not an authority of this authorization.")]
    NotAnAuthority(Pubkey),
    #[error("This is not a Submit instruction of the signer program.")]
    NotASubmit,
    #[error("{0}")]
    Message(String),
}

/// The address the signer program signs as on behalf of `authority`.
///
/// This, not the authority's own key, is what a program sees as the signer.
/// Make it the Squads member, the token owner, the nonce authority.
pub fn programmatic_signer_address(authority: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PROGRAMMATIC_SIGNER_SEED, authority.as_ref()],
        &ED25519_SIGNER_PROGRAM_ID,
    )
    .0
}
