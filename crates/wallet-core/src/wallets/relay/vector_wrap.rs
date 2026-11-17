//! Rebuilding a transaction so the Vector wallets it names as signers can
//! sign it. A wallet's address is its account's system PDA, which the
//! System program owns, so what it signs runs unchanged: a transfer, rent
//! for a new account, a token account it owns.
//!
//! ```text
//! before                         after
//! ix 0  signed by A              Advance      A, B, C: next keys, zeroed signatures
//! ix 1  signed by B              Passthrough  A, B: ix 0, ix 1
//! ix 2  no Vector signer         ix 2
//! ix 3  signed by C              Passthrough  C: ix 3
//! ```
//!
//! Instructions next to each other that Vector accounts sign share one
//! `Passthrough`, which runs as every account they name, as far as one can
//! carry them. The order of the instructions is kept, and one that no
//! Vector account signs stays where it was. A precompile, or a program that
//! reads the instructions sysvar, sees what the site built.

use super::*;
use crate::tx::builder::compile_message;
use crate::wallets::programmatic::ED25519_SIGNER_PROGRAM_ID;
use crate::wallets::vector::{
    advance, demoted_writable, multi_passthrough, passthrough_fits, system_pda, valid_until,
    AdvanceEntry, SchemeId, VectorError,
};
use solana_instruction::Instruction;

/// Compute units added for each `Passthrough`: reading what it carries and
/// the calls it makes, and deriving each wallet's system PDA (about 1,500).
/// What runs inside is counted in the stated limit.
pub const PASSTHROUGH_CU: u32 = 5_000;

/// A Vector account a transaction may be wrapped for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorWrap {
    /// The program's account, which the `Advance` lists. The wallet signs as
    /// its system PDA ([`VectorWrap::address`]).
    pub account: Pubkey,
    pub scheme: SchemeId,
    /// The key the account was created with. The builders derive the
    /// account from it, so it stays the first key after a rotation.
    pub key: Vec<u8>,
    /// The key the `Advance` names for the account to sign with next, which
    /// a Winternitz or XMSS account needs and no other scheme takes. For
    /// Winternitz it is a new key every time. For XMSS it is the key the
    /// account holds now to keep it, or a new one when few leaves are left.
    pub next_key: Option<Vec<u8>>,
}

impl VectorWrap {
    /// The wallet's address: the account's system PDA, which the
    /// transaction names as a signer and its `Passthrough` signs as.
    pub fn address(&self) -> Pubkey {
        system_pda(&self.account)
    }
}

/// Rebuild `tx` so that each of `accounts` it needs as a signer signs
/// through one `Advance`, listed in the order of `accounts`. A run of
/// instructions they sign becomes one `Passthrough`, whichever of them signs
/// each one, so an instruction two of them sign runs too. A Squads clear
/// signing instruction they do not sign joins the run after it, so the
/// checks for a vote run in the same `Passthrough` as the vote.
///
/// Each wallet is named by its address, the account's system PDA, and the
/// `Passthrough` signs as it, so the instructions run unchanged. The new
/// transaction is paid for by `relay_payer` and lives on
/// `blockhash`. Its compute limit is the one `tx` stated, or the default
/// for its instructions, plus each signature's and each `Passthrough`'s.
///
/// `slot` is the cluster's slot now. The `Advance` stays valid for
/// [`AUTHORIZATION_SLOTS`](crate::wallets::vector::AUTHORIZATION_SLOTS)
/// after it, or never expires when a Winternitz or XMSS account signs
/// ([`valid_until`]).
pub fn wrap_for_vector(
    tx: &DecodedTx,
    accounts: &[VectorWrap],
    relay_payer: Pubkey,
    blockhash: Hash,
    slot: u64,
) -> Result<Vec<u8>, WrapError> {
    if tx.uses_durable_nonce {
        return Err(WrapError::DurableNonce);
    }
    // A `Submit` may already be here: the programmatic wrap runs first and
    // leaves instructions the Vector accounts do not sign at the top level.
    // An `Advance` means this wrap already ran.
    if !vector_accounts(tx).is_empty() {
        return Err(WrapError::AlreadyWrapped);
    }
    if accounts
        .iter()
        .any(|wrap| wrap.account == relay_payer || wrap.address() == relay_payer)
    {
        return Err(WrapError::PayerCannotSign(relay_payer));
    }

    let mut used: Vec<&VectorWrap> = Vec::new();
    let native = tx.native_instructions();
    let mut signings: Vec<Vec<&VectorWrap>> = native
        .iter()
        .map(|instruction| {
            accounts
                .iter()
                .filter(|wrap| {
                    instruction
                        .accounts
                        .iter()
                        .any(|meta| meta.is_signer && meta.pubkey == wrap.address())
                })
                .collect()
        })
        .collect();
    // A Squads clear signing instruction no Vector account signs, such as a
    // `verifyBufferHash`, runs in the `Passthrough` of the instructions after
    // it, with the vote it belongs to
    // (`crate::wallets::squads::moves_into_authorization`).
    for index in (0..native.len().saturating_sub(1)).rev() {
        if signings[index].is_empty()
            && crate::wallets::squads::moves_into_authorization(&native[index])
        {
            signings[index] = signings[index + 1].clone();
        }
    }
    // Runs of instructions, each with the accounts it runs as. A run no
    // Vector account signs has none and stays at the top level.
    let mut runs: Vec<(Vec<&VectorWrap>, Vec<Instruction>)> = Vec::new();
    for ((index, instruction), signing) in native.into_iter().enumerate().zip(signings) {
        if let Some(wrap) = signing.first() {
            // Only a Vector account signing between two programmatic
            // instructions lands in a `Submit`; it would run its whole
            // authorization inside the `Passthrough`.
            if instruction.program_id == ED25519_SIGNER_PROGRAM_ID {
                return Err(WrapError::VectorInsideSubmit {
                    instruction: index,
                    account: wrap.address(),
                });
            }
        }
        for wrap in &signing {
            if !used.iter().any(|seen| seen.account == wrap.account) {
                used.push(wrap);
            }
        }
        match runs.last_mut() {
            Some((signers, run)) if signers.is_empty() && signing.is_empty() => {
                run.push(instruction)
            }
            Some((signers, run)) if !signers.is_empty() && !signing.is_empty() => {
                let joined = joined(accounts, signers, &signing);
                run.push(instruction);
                if passthrough_fits(joined.len(), run) {
                    *signers = joined;
                } else {
                    // One `Passthrough` cannot take it: the next one does.
                    let instruction = run.pop().expect("just pushed");
                    runs.push((signing, vec![instruction]));
                }
            }
            _ => runs.push((signing, vec![instruction])),
        }
    }
    if used.is_empty() {
        return Err(WrapError::NothingToWrap);
    }
    // The Advance lists the accounts in the order given, not the order met.
    used.sort_by_key(|wrap| {
        accounts
            .iter()
            .position(|other| other.account == wrap.account)
    });

    let zeros: Vec<Vec<u8>> = used
        .iter()
        .map(|wrap| vec![0; wrap.scheme.signature_len()])
        .collect();
    let signers: Vec<AdvanceEntry> = used
        .iter()
        .zip(&zeros)
        .map(|(wrap, signature)| AdvanceEntry {
            scheme: wrap.scheme,
            key: &wrap.key,
            next_key: wrap.next_key.as_deref().unwrap_or_default(),
            signature,
        })
        .collect();
    let expiry = valid_until(slot, used.iter().map(|wrap| wrap.scheme));
    let mut instructions = vec![advance(expiry, &signers)?];
    let mut passthroughs = 0u32;
    for (signers, run) in runs {
        if signers.is_empty() {
            instructions.extend(run);
            continue;
        }
        let keys: Vec<(SchemeId, &[u8])> = signers
            .iter()
            .map(|wrap| (wrap.scheme, wrap.key.as_slice()))
            .collect();
        instructions.push(multi_passthrough(&keys, &run)?);
        passthroughs += 1;
    }
    if let Some(key) = demoted_writable(&instructions, &relay_payer) {
        return Err(VectorError::DemotedWritable(key).into());
    }

    let signatures: u32 = used.iter().map(|wrap| wrap.scheme.advance_cu()).sum();
    let limit = tx
        .stated_cu_limit()
        .saturating_add(signatures)
        .saturating_add(passthroughs.saturating_mul(PASSTHROUGH_CU));
    let config = tx.budget.rebuilt_config(limit);
    Ok(compile_message(&relay_payer, &instructions, blockhash, config)?.wire)
}

/// The accounts of `run` and of `more` together, in the order `accounts`
/// gives them.
fn joined<'a>(
    accounts: &'a [VectorWrap],
    run: &[&'a VectorWrap],
    more: &[&'a VectorWrap],
) -> Vec<&'a VectorWrap> {
    accounts
        .iter()
        .filter(|wrap| {
            run.iter()
                .chain(more)
                .any(|other| other.account == wrap.account)
        })
        .collect()
}
