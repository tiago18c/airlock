//! The program's instructions, built by its client crate after every length
//! and count has been checked here. The crate's builders `assert!` on what
//! they are given, and nothing the app is handed may reach one unchecked.
//!
//! Each builds for [`PROGRAM_ID`](super::PROGRAM_ID), the program the crate
//! was compiled for. A key is always what `Initialize` took: for XMSS, the
//! first key, which names the account after it has moved on.

use super::scheme::SchemeId;
use super::VectorError;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use std::collections::HashSet;

/// Most accounts one instruction of a v1 message can list.
pub const MAX_INSTRUCTION_ACCOUNTS: usize = u8::MAX as usize;

/// Most accounts one `Passthrough` runs as: the runtime's limit on the
/// signers of one call, which the program keeps.
pub const MAX_PASSTHROUGH_SIGNERS: usize = 16;

/// Every `Initialize` that creates the account of `key`, for one
/// transaction. Anyone may send them and `payer` pays the rent. The first
/// carries the key, and an ML-DSA-44 account takes two more that grow it to
/// its full size.
pub fn initialize(
    payer: &Pubkey,
    scheme: SchemeId,
    key: &[u8],
) -> Result<Vec<Instruction>, VectorError> {
    scheme.check_key(key)?;
    Ok(vector_core::create_initialize_instructions(
        payer,
        &scheme.core(),
        key,
    ))
}

/// One account of an `Advance`: its scheme, the key it was created with,
/// the key it signs with next, and its signature. The next key is empty for
/// a scheme whose key never changes. For Winternitz it is a new key; for
/// XMSS a new key, or the one the account holds now to keep it. A signature
/// not made yet is zeros of its scheme's length.
#[derive(Clone, Copy, Debug)]
pub struct AdvanceEntry<'a> {
    pub scheme: SchemeId,
    pub key: &'a [u8],
    pub next_key: &'a [u8],
    pub signature: &'a [u8],
}

/// The `Advance` for `signers`, in the order their next keys and signatures
/// are laid out.
pub fn advance(signers: &[AdvanceEntry]) -> Result<Instruction, VectorError> {
    check_signers(signers.iter().map(|entry| (entry.scheme, entry.key)))?;
    for entry in signers {
        entry.scheme.check_next_key(entry.next_key)?;
        entry.scheme.check_signature(entry.signature)?;
    }
    let schemes: Vec<_> = signers.iter().map(|entry| entry.scheme.core()).collect();
    let signers: Vec<_> = schemes
        .iter()
        .zip(signers)
        .map(|(core, entry)| {
            (
                vector_core::AdvanceSigner {
                    scheme: core,
                    identity: entry.key,
                    next_key: entry.next_key,
                },
                entry.signature,
            )
        })
        .collect();
    Ok(vector_core::create_multi_advance_instruction(&signers))
}

/// A transaction that only moves the account of `key` to `next_key`: one
/// `Advance`, and nothing else that its signature covers.
pub fn rotation(scheme: SchemeId, key: &[u8], next_key: &[u8]) -> Result<Instruction, VectorError> {
    advance(&[AdvanceEntry {
        scheme,
        key,
        next_key,
        signature: &vec![0; scheme.signature_len()],
    }])
}

/// What the key of `key`'s account signs in a [`rotation`]. The `Advance`
/// is the transaction's only instruction and names no fee payer, so this is
/// the same whoever pays and whatever its blockhash and compute limits: the
/// device that holds the key can make it without a node.
pub fn rotation_message(
    scheme: SchemeId,
    key: &[u8],
    next_key: &[u8],
) -> Result<[u8; 32], VectorError> {
    // An address the `Advance` does not list, so the fee payer's flags
    // touch none of its accounts.
    let anyone = Pubkey::new_from_array([0xfe; 32]);
    super::advance_message(&[rotation(scheme, key, next_key)?], &anyone)
}

/// Check what an `Advance` lists: at least one account, each key the length
/// of its scheme, no account twice, and all of them in one instruction.
///
/// The program would take an account twice and check its second signature
/// against the nonce the first one moved it to. Nothing here signs that way.
fn check_signers<'a>(
    signers: impl ExactSizeIterator<Item = (SchemeId, &'a [u8])>,
) -> Result<Vec<Pubkey>, VectorError> {
    if signers.len() == 0 {
        return Err(VectorError::NoSigners);
    }
    // The accounts, then the instructions sysvar.
    if signers.len() + 1 > MAX_INSTRUCTION_ACCOUNTS {
        return Err(VectorError::TooManySigners(signers.len()));
    }
    let mut seen = HashSet::new();
    let mut accounts = Vec::with_capacity(signers.len());
    for (scheme, key) in signers {
        let account = super::address(&super::PROGRAM_ID, scheme, key)?;
        if !seen.insert(account) {
            return Err(VectorError::SignerTwice(account));
        }
        accounts.push(account);
    }
    Ok(accounts)
}

/// Run `instructions` as the account of `key`, which signs each of them. The
/// transaction's `Advance` has to list the account, and is where a
/// hash-based key moves on.
pub fn passthrough(
    scheme: SchemeId,
    key: &[u8],
    instructions: &[Instruction],
) -> Result<Instruction, VectorError> {
    multi_passthrough(&[(scheme, key)], instructions)
}

/// Run `instructions` as every account of `signers`, given as scheme and the
/// key each was created with. Each instruction is signed by whichever of
/// them it names, so instructions of several accounts share one
/// `Passthrough`. The transaction's `Advance` has to list every account.
pub fn multi_passthrough(
    signers: &[(SchemeId, &[u8])],
    instructions: &[Instruction],
) -> Result<Instruction, VectorError> {
    if signers.is_empty() {
        return Err(VectorError::NoSigners);
    }
    if signers.len() > MAX_PASSTHROUGH_SIGNERS {
        return Err(VectorError::TooManyPassthroughSigners(signers.len()));
    }
    let mut seen = HashSet::new();
    for (scheme, key) in signers {
        scheme.check_key(key)?;
        let account = super::address(&super::PROGRAM_ID, *scheme, key)?;
        if !seen.insert(account) {
            return Err(VectorError::SignerTwice(account));
        }
    }
    check_inner(signers.len(), instructions)?;
    let schemes: Vec<_> = signers.iter().map(|(scheme, _)| scheme.core()).collect();
    let listed: Vec<_> = schemes
        .iter()
        .zip(signers)
        .map(|(core, (_, key))| vector_core::PassthroughSigner::vector(core, key))
        .collect();
    Ok(vector_core::create_multi_passthrough_instruction(
        &listed,
        instructions,
    ))
}

/// Whether one `Passthrough` running as `signers` accounts can carry
/// `instructions`.
pub fn passthrough_fits(signers: usize, instructions: &[Instruction]) -> bool {
    signers <= MAX_PASSTHROUGH_SIGNERS && check_inner(signers, instructions).is_ok()
}

/// What one `Passthrough` can carry: 255 instructions, each with at most 255
/// accounts and 65,535 bytes of data, and every account of all of them in
/// the one list a message gives an instruction.
pub(super) fn check_inner(signers: usize, instructions: &[Instruction]) -> Result<(), VectorError> {
    if instructions.len() > usize::from(u8::MAX) {
        return Err(VectorError::TooManyInstructions(instructions.len()));
    }
    // The instructions sysvar and the accounts it runs as come first.
    let mut accounts = 1 + signers;
    for (index, instruction) in instructions.iter().enumerate() {
        if instruction.accounts.len() > MAX_INSTRUCTION_ACCOUNTS {
            return Err(VectorError::InnerAccounts {
                index,
                found: instruction.accounts.len(),
            });
        }
        if instruction.data.len() > usize::from(u16::MAX) {
            return Err(VectorError::InnerData {
                index,
                found: instruction.data.len(),
            });
        }
        accounts += 1 + instruction.accounts.len();
    }
    if accounts > MAX_INSTRUCTION_ACCOUNTS {
        return Err(VectorError::PassthroughAccounts(accounts));
    }
    Ok(())
}

/// Move `lamports` from the account of `key` to `receiver`. It runs only
/// inside that account's `Passthrough`.
pub fn withdraw(
    scheme: SchemeId,
    key: &[u8],
    receiver: &Pubkey,
    lamports: u64,
) -> Result<Instruction, VectorError> {
    scheme.check_key(key)?;
    Ok(vector_core::create_withdraw_subinstruction(
        &scheme.core(),
        key,
        receiver,
        lamports,
    ))
}

/// Close the account of `key` and send what it holds to `receiver`. It runs
/// only inside that account's `Passthrough`.
pub fn close(scheme: SchemeId, key: &[u8], receiver: &Pubkey) -> Result<Instruction, VectorError> {
    scheme.check_key(key)?;
    Ok(vector_core::create_close_subinstruction(
        &scheme.core(),
        key,
        receiver,
    ))
}
