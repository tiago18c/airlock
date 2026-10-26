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

/// Raydium's concentrated-liquidity AMM on devnet, from the IDL pack.
pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH");

/// Raydium's concentrated-liquidity AMM on mainnet, from the IDL pack.
pub const RAYDIUM_CLMM_MAINNET_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK");

/// Raydium's constant-product AMM on devnet, from the IDL pack.
pub const RAYDIUM_CP_SWAP_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb");

/// Raydium's AMM routing program on devnet, from the IDL pack.
pub const RAYDIUM_ROUTER_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("DRaybByLpbUL57LJARs3j8BitTxVfzBg351EaMr5UTCd");

/// Orca's Whirlpool AMM, on devnet and mainnet, from the IDL pack.
pub const ORCA_WHIRLPOOL_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");

/// Every IDL the app ships, the core and the IDL pack's, on `network`.
pub fn shipped_catalog(
    network: crate::IdlNetwork,
) -> crate::IdlStore<wallet_vault::store::MemoryStore, crate::MemoryBodies> {
    let mut store = crate::IdlStore::open_on(
        wallet_vault::store::MemoryStore::new(),
        crate::MemoryBodies::new(),
        network,
    )
    .with_builtins();
    let refused = store.add_embedded(crate::idl::pack::shipped_idls());
    assert!(refused.is_empty(), "{refused:?}");
    store
}
