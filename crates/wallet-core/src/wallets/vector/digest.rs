//! What the accounts of an `Advance` sign, and the one case where the program
//! would hash something other than what the app can compute.
//!
//! The program hashes the instructions sysvar with the `Advance`'s data cut
//! out after its first byte: one message for every account, then for each
//! account `sha256(message ‖ nonce ‖ address ‖ next key)`, which is also its
//! next nonce. The next key is the one the `Advance` names for the account,
//! and empty for a scheme whose key never changes.
//!
//! The program's client crate computes the message too, but it makes the
//! `Advance` again from every account's key. A device that signs for one
//! account seldom has the others' keys: a context note carries addresses and
//! schemes, not keys. So the message is computed here from the `Advance` the
//! transaction holds, with the sysvar laid out by the same crate the client
//! uses, and a test holds it equal to the client's.
//!
//! The sysvar flags an account the way the runtime does, and the runtime
//! clears some writable flags the message sets. Neither this nor the crate
//! models that, so [`demoted_writable`] finds a transaction where it would
//! apply, and such a transaction is refused instead of signed.

use super::instructions::MAX_INSTRUCTION_ACCOUNTS;
use super::{VectorError, PROGRAM_ID};
use sha2::{Digest, Sha256};
use solana_instruction::{BorrowedAccountMeta, BorrowedInstruction, Instruction};
use solana_instructions_sysvar::construct_instructions_data;
use solana_pubkey::Pubkey;
use solana_sdk_ids::{
    address_lookup_table, bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable,
    compute_budget, config, ed25519_program, feature, loader_v4, native_loader, secp256k1_program,
    secp256r1_program, stake, system_program, sysvar, vote, zk_elgamal_proof_program,
    zk_token_proof_program,
};
use std::collections::HashMap;

/// The message every account of the transaction's `Advance` signs.
///
/// `instructions` are the transaction's, in order, and one of them is its
/// `Advance`. Whatever its signatures hold, they are not part of the
/// message. Each account is flagged as the message flags it: signer or
/// writable when any instruction says so, and the fee payer both.
pub fn advance_message(
    instructions: &[Instruction],
    fee_payer: &Pubkey,
) -> Result<[u8; 32], VectorError> {
    let index = advance_index(instructions)?;
    let mut flags: HashMap<Pubkey, (bool, bool)> = HashMap::new();
    flags.insert(*fee_payer, (true, true));
    for meta in instructions
        .iter()
        .flat_map(|instruction| &instruction.accounts)
    {
        let entry = flags.entry(meta.pubkey).or_default();
        entry.0 |= meta.is_signer;
        entry.1 |= meta.is_writable;
    }
    let borrowed: Vec<BorrowedInstruction> = instructions
        .iter()
        .map(|instruction| BorrowedInstruction {
            program_id: &instruction.program_id,
            accounts: instruction
                .accounts
                .iter()
                .map(|meta| {
                    let (is_signer, is_writable) = flags[&meta.pubkey];
                    BorrowedAccountMeta {
                        pubkey: &meta.pubkey,
                        is_signer,
                        is_writable,
                    }
                })
                .collect(),
            data: &instruction.data,
        })
        .collect();
    let mut sysvar = construct_instructions_data(&borrowed);
    // The runtime ends the sysvar with the running instruction's index. The
    // crate leaves it zero.
    let len = sysvar.len();
    let running = u16::try_from(index).map_err(|_| VectorError::AdvanceMismatch)?;
    sysvar[len - 2..].copy_from_slice(&running.to_le_bytes());

    // The sysvar starts with a count and one u16 offset per instruction.
    // At the Advance's offset: its account count, 33 bytes per account, the
    // program, the data's length, then the data, whose first byte is the
    // discriminator and the rest the next keys and signatures.
    let at = 2 + 2 * index;
    let offset = usize::from(u16::from_le_bytes([sysvar[at], sysvar[at + 1]]));
    let advance = &instructions[index];
    let data = offset + 2 + 33 * advance.accounts.len() + 32 + 2;
    let mut hasher = Sha256::new();
    hasher.update(&sysvar[..data + 1]);
    hasher.update(&sysvar[data + advance.data.len()..]);
    Ok(hasher.finalize().into())
}

/// The accounts the transaction's `Advance` lists, in the order of their
/// signatures.
pub fn advance_accounts(instructions: &[Instruction]) -> Result<Vec<Pubkey>, VectorError> {
    let advance = &instructions[advance_index(instructions)?];
    let listed = &advance.accounts[..advance.accounts.len() - 1];
    Ok(listed.iter().map(|meta| meta.pubkey).collect())
}

/// What `account` signs for `message`, at the nonce it holds now, handing
/// itself to `next_key` (empty for a scheme whose key never changes). It is
/// the account's nonce once the transaction lands.
pub fn signer_digest(
    message: &[u8; 32],
    nonce: &[u8; 32],
    account: &Pubkey,
    next_key: &[u8],
) -> [u8; 32] {
    vector_core::signer_digest(message, nonce, account, next_key)
}

/// The position of the transaction's one `Advance`. A second could not be
/// signed: each would have to cover the other's signatures.
fn advance_index(instructions: &[Instruction]) -> Result<usize, VectorError> {
    let mut found = instructions.iter().enumerate().filter(|(_, instruction)| {
        instruction.program_id == PROGRAM_ID
            && instruction.data.first() == Some(&super::ADVANCE_DISCRIMINATOR)
    });
    let (index, _) = found.next().ok_or(VectorError::NoAdvance)?;
    if found.next().is_some() {
        return Err(VectorError::TwoAdvances);
    }
    let advance = &instructions[index];
    if advance.accounts.is_empty()
        || advance.accounts.len() > MAX_INSTRUCTION_ACCOUNTS
        || advance.data.len() > usize::from(u16::MAX)
    {
        return Err(VectorError::MalformedAdvance);
    }
    Ok(index)
}

/// An account the message marks writable that the runtime will not let be
/// written, so the instructions sysvar flags it read-only and the digest
/// made here would not be the program's.
///
/// The runtime clears the flag of a reserved key, and of a program called
/// by an instruction of the transaction unless the upgradeable loader is
/// among its accounts. `instructions` and `fee_payer` are as in
/// [`advance_message`].
pub fn demoted_writable(instructions: &[Instruction], fee_payer: &Pubkey) -> Option<Pubkey> {
    let loader_present = instructions.iter().any(|instruction| {
        instruction.program_id == bpf_loader_upgradeable::ID
            || instruction
                .accounts
                .iter()
                .any(|meta| meta.pubkey == bpf_loader_upgradeable::ID)
    });
    let called = |key: &Pubkey| {
        instructions
            .iter()
            .any(|instruction| instruction.program_id == *key)
    };
    let mut writable = instructions
        .iter()
        .flat_map(|instruction| &instruction.accounts)
        .filter(|meta| meta.is_writable)
        .map(|meta| &meta.pubkey)
        // The fee payer is always writable in the message.
        .chain(std::iter::once(fee_payer));
    writable
        .find(|key| is_reserved(key) || (called(key) && !loader_present))
        .copied()
}

/// Keys the runtime never lets a transaction write: built-in programs and
/// sysvars. The runtime's list grows with feature activations. One it adds
/// later, not here, is a transaction that fails on chain when it lands,
/// with nothing signed for anything else.
fn is_reserved(key: &Pubkey) -> bool {
    RESERVED.contains(key)
}

pub(super) const RESERVED: [Pubkey; 31] = [
    address_lookup_table::ID,
    bpf_loader::ID,
    bpf_loader_deprecated::ID,
    bpf_loader_upgradeable::ID,
    compute_budget::ID,
    config::ID,
    ed25519_program::ID,
    feature::ID,
    loader_v4::ID,
    secp256k1_program::ID,
    secp256r1_program::ID,
    #[allow(deprecated)]
    stake::config::ID,
    stake::ID,
    system_program::ID,
    vote::ID,
    zk_elgamal_proof_program::ID,
    zk_token_proof_program::ID,
    sysvar::clock::ID,
    sysvar::epoch_rewards::ID,
    sysvar::epoch_schedule::ID,
    sysvar::fees::ID,
    sysvar::instructions::ID,
    sysvar::last_restart_slot::ID,
    sysvar::recent_blockhashes::ID,
    sysvar::rent::ID,
    sysvar::rewards::ID,
    sysvar::slot_hashes::ID,
    sysvar::slot_history::ID,
    sysvar::stake_history::ID,
    native_loader::ID,
    sysvar::ID,
];
