//! The authorization of the Ed25519 signer program: one `Submit` instruction
//! that names its authorities and holds a place for each one's signature.

use super::*;
use crate::keys::signer::WalletSigner;
use crate::transport::handoff::SignatureObject;
use crate::transport::protocol::Signed;
use crate::tx::builder::compile_message;
use crate::wallets::programmatic::{
    fill_submit_signature, programmatic_signer_address, Authorization, ExecutorNonce, Submission,
    SubmitRole, ED25519_SIGNER_PROGRAM_ID,
};
use solana_signature::Signature;
use std::collections::BTreeMap;

/// Compute units added to a transaction's limit when it is wrapped, for the
/// signature check, the nonce and the two extra calls on the way in.
pub const RELAY_OVERHEAD_CU: u32 = 100_000;

/// Every authority the transaction's `Submit` instructions name.
pub(super) fn slots(tx: &DecodedTx) -> Vec<AuthoritySlot> {
    submissions(tx)
        .flat_map(|(instruction, submission)| {
            submission
                .authorities()
                .map(|(authority, signed)| AuthoritySlot {
                    instruction,
                    authority: AuthorityId::Ed25519(authority),
                    address: programmatic_signer_address(&authority),
                    place: None,
                    next_key_len: 0,
                    signed,
                    verified: None,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn submissions(tx: &DecodedTx) -> impl Iterator<Item = (usize, Submission)> + '_ {
    tx.instructions
        .iter()
        .enumerate()
        .filter(|(_, instruction)| instruction.program_id == ED25519_SIGNER_PROGRAM_ID)
        .filter_map(|(index, instruction)| Some((index, Submission::parse(&instruction.data)?)))
}

/// Sign every unsigned authorization that one of `signers` is the authority of.
/// A transaction with nothing for them to sign comes back unchanged.
pub fn sign_authorizations(
    wire: &[u8],
    signers: &[&dyn WalletSigner],
) -> Result<Authorized, SignError> {
    sign_authorizations_in(wire, signers, &AuthContext::default())
}

/// [`sign_authorizations`], with what `ctx` says of the Vector accounts: it
/// tells where their signatures are, which a new `Submit` signature voids.
pub fn sign_authorizations_in(
    wire: &[u8],
    signers: &[&dyn WalletSigner],
    ctx: &AuthContext,
) -> Result<Authorized, SignError> {
    let tx = decode(wire)?;
    let mut signed = 0;
    let (wire, dropped) = rebuild(&tx, |message| {
        for (index, submission) in submissions(&tx) {
            for (authority, done) in submission.authorities() {
                if done {
                    continue;
                }
                let Some(signer) = signers.iter().find(|signer| signer.pubkey() == authority)
                else {
                    continue;
                };
                let signature = signer.try_sign_message(&submission.signing_bytes)?;
                fill(message, index, &authority, &signature)?;
                signed += 1;
            }
        }
        if signed > 0 {
            clear_advance_signatures(message, &tx, ctx);
        }
        Ok(())
    })?;
    Ok(Authorized {
        wire,
        signed,
        dropped,
    })
}

/// Write an authorization signature made elsewhere into the first `Submit`
/// that is waiting for it. It is checked against the authorization message.
///
/// Any Vector signatures already in an `Advance` are cleared: the Vector
/// digest covers the `Submit`'s data, so they no longer hold.
pub fn insert_authority_signature(
    wire: &[u8],
    authority: &Pubkey,
    signature: &[u8; SIGNATURE_BYTES],
) -> Result<Authorized, SignError> {
    insert_authority_signature_in(wire, authority, signature, &AuthContext::default())
}

/// [`insert_authority_signature`], with what `ctx` says of the Vector
/// accounts, so their signatures can be found and cleared.
pub fn insert_authority_signature_in(
    wire: &[u8],
    authority: &Pubkey,
    signature: &[u8; SIGNATURE_BYTES],
    ctx: &AuthContext,
) -> Result<Authorized, SignError> {
    let tx = decode(wire)?;
    let signature = Signature::from(*signature);
    let mut known = false;
    let mut open = false;
    let mut placed = false;
    let (wire, dropped) = rebuild(&tx, |message| {
        for (index, submission) in submissions(&tx) {
            let Some((_, done)) = submission.authorities().find(|(key, _)| key == authority) else {
                continue;
            };
            known = true;
            if done {
                continue;
            }
            open = true;
            if !signature.verify(authority.as_ref(), &submission.signing_bytes) {
                continue;
            }
            fill(message, index, authority, &signature)?;
            clear_advance_signatures(message, &tx, ctx);
            placed = true;
            break;
        }
        Ok(())
    })?;
    if !placed {
        return Err(SignError::Message(if !known {
            "this pubkey is not an authority of this transaction".into()
        } else if open {
            "signature does not match this authorization".into()
        } else {
            "this authority's signature is already in the transaction".into()
        }));
    }
    Ok(Authorized {
        wire,
        signed: 1,
        dropped,
    })
}

/// One object per authorization signature in `tx`. Each says it signs an
/// authorization, so the device that receives it does not have to guess.
pub fn authority_signature_objects(tx: &DecodedTx) -> Vec<SignatureObject> {
    let mut out = Vec::new();
    for (index, submission) in submissions(tx) {
        let Some(data) = tx.instructions.get(index).map(|ix| &ix.data) else {
            continue;
        };
        for signer in &submission.signers {
            let SubmitRole::Authority {
                signed: true,
                offset,
            } = signer.role
            else {
                continue;
            };
            let Some(signature) = data
                .get(offset..offset + SIGNATURE_BYTES)
                .and_then(|bytes| <[u8; SIGNATURE_BYTES]>::try_from(bytes).ok())
            else {
                continue;
            };
            out.push(SignatureObject {
                pubkey: signer.pubkey,
                signed: Signed::Authorization {
                    authority: signer.pubkey.to_bytes(),
                    signature,
                },
            });
        }
    }
    out
}

/// Signers of `tx` that are the derived address of one of `authorities`, as
/// `(authority, address)`. No key can sign for such an address, so the
/// transaction cannot be completed as it stands.
pub fn programmatic_signers_of(tx: &DecodedTx, authorities: &[Pubkey]) -> Vec<(Pubkey, Pubkey)> {
    let derived: Vec<(Pubkey, Pubkey)> = authorities
        .iter()
        .map(|authority| (*authority, programmatic_signer_address(authority)))
        .collect();
    tx.signers
        .iter()
        .filter_map(|slot| derived.iter().find(|(_, address)| *address == slot.pubkey))
        .copied()
        .collect()
}

/// Rebuild `tx` so its programmatic signers can sign.
///
/// The instructions from the first that those addresses sign to the last
/// move, unchanged and in order, into one authorization that consumes
/// `nonce`, and a single `Submit` takes their place. An instruction between
/// them that they do not sign moves too, so nothing runs out of order.
/// Instructions before the first and after the last stay at the top level,
/// where a Vector wrap can still cover them. When every instruction is
/// wrapped, the new transaction is only that `Submit`.
///
/// The new transaction is paid for by `relay_payer` and lives on
/// `blockhash`. Ordinary keys the instructions need still sign it. The
/// compute limit carries over with [`RELAY_OVERHEAD_CU`] more, and so does
/// the priority fee. The loaded accounts limit does not: see
/// [`fit_loaded_accounts`].
pub fn wrap_for_programmatic(
    tx: &DecodedTx,
    authorities: &[Pubkey],
    nonce: ExecutorNonce,
    relay_payer: Pubkey,
    blockhash: Hash,
) -> Result<Vec<u8>, WrapError> {
    if tx.uses_durable_nonce {
        return Err(WrapError::DurableNonce);
    }
    if !authority_slots(tx).is_empty() {
        return Err(WrapError::AlreadyWrapped);
    }
    let used = programmatic_signers_of(tx, authorities);
    if used.is_empty() {
        return Err(WrapError::NothingToWrap);
    }
    if authorities
        .iter()
        .any(|authority| programmatic_signer_address(authority) == relay_payer)
    {
        return Err(WrapError::PayerCannotSign(relay_payer));
    }

    let addresses: Vec<Pubkey> = used.iter().map(|(_, address)| *address).collect();
    let mut before = tx.native_instructions();
    let signed_by_programmatic = |instruction: &solana_instruction::Instruction| {
        instruction
            .accounts
            .iter()
            .any(|meta| meta.is_signer && addresses.contains(&meta.pubkey))
    };
    // `used` is not empty, so some instruction is signed by one of them.
    let start = before
        .iter()
        .position(signed_by_programmatic)
        .unwrap_or_default();
    let end = before
        .iter()
        .rposition(signed_by_programmatic)
        .unwrap_or_default();
    let after = before.split_off(end + 1);
    let wrapped = before.split_off(start);
    // The execution message needs a first signer. Nothing is charged to it,
    // so keep the transaction's own when it is a programmatic signer.
    let first = tx
        .fee_payer()
        .filter(|payer| addresses.contains(payer))
        .unwrap_or(used[0].1);
    let signing: Vec<Pubkey> = used.iter().map(|(authority, _)| *authority).collect();
    let authorization = Authorization::new(first, &wrapped, &signing, nonce)?;
    let submit = authorization.submit_instruction_unsigned(&BTreeMap::new())?;
    let instructions: Vec<_> = before
        .into_iter()
        .chain(std::iter::once(submit))
        .chain(after)
        .collect();

    let limit = tx.stated_cu_limit().saturating_add(RELAY_OVERHEAD_CU);
    let config = tx.budget.rebuilt_config(limit);
    Ok(compile_message(&relay_payer, &instructions, blockhash, config)?.wire)
}

fn fill(
    message: &mut v1::Message,
    instruction: usize,
    authority: &Pubkey,
    signature: &Signature,
) -> Result<(), SignError> {
    let data = &mut message
        .instructions
        .get_mut(instruction)
        .ok_or_else(|| SignError::Message("the transaction has no such instruction".into()))?
        .data;
    fill_submit_signature(data, authority, signature)
        .map_err(|err| SignError::Message(err.to_string()))
}

/// Zero every Vector signature in the `Advance` of `tx`, whose message is
/// `message`. A later `Submit` fill changes what those signatures covered.
///
/// The next key a Winternitz or XMSS account moves to sits just before its
/// signature, and is part of what it signs: it stays. A signature whose
/// place `ctx` cannot tell stays too. Nothing here signs for a Vector
/// account before every `Submit` is signed, so only a transaction made
/// elsewhere has one, and it fails on chain.
fn clear_advance_signatures(message: &mut v1::Message, tx: &DecodedTx, ctx: &AuthContext) {
    for slot in super::vector::slots(tx, ctx) {
        let Some((start, len)) = slot.place.filter(|_| slot.signed) else {
            continue;
        };
        if let Some(signature) = message
            .instructions
            .get_mut(slot.instruction)
            .and_then(|instruction| instruction.data.get_mut(start..start + len))
        {
            signature.fill(0);
        }
    }
}
