//! Instructions for tests, built without the IDL path under test.
//!
//! System and stake come from the programs' own interface crates. Token and
//! associated-token come from `spl`, which writes their layouts out from the
//! programs' documented instruction formats.

use crate::tx::builder::{compile_v1, CompiledTx};
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

pub fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

pub fn blockhash(n: u8) -> Hash {
    Hash::new_from_array([n; 32])
}

/// Compile against a recent blockhash.
pub fn compile(fee_payer: Pubkey, instructions: Vec<Instruction>) -> CompiledTx {
    compile_v1(fee_payer, instructions, None, Some(blockhash(1))).expect("compiles")
}

pub fn transfer(from: Pubkey, to: Pubkey, lamports: u64) -> Instruction {
    solana_system_interface::instruction::transfer(&from, &to, lamports)
}

pub fn stake_delegate(stake: Pubkey, authority: Pubkey, vote: Pubkey) -> Instruction {
    solana_stake_interface::instruction::delegate_stake(&stake, &authority, &vote)
}

/// Token instructions, from the hand-written layouts in `spl`.
pub use crate::spl::{
    associated_token_address, create_ata_idempotent, transfer_checked as token_transfer_checked,
};
