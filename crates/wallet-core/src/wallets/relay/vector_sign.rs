//! Signing the `Advance` of a transaction for the Vector accounts whose keys
//! this device holds, and taking in signatures made on other devices.
//!
//! Three rules decide what a key here signs, beyond whether its place in the
//! `Advance` is empty:
//!
//! - A hash-based key signs last. Each signature spends a leaf, or the whole
//!   Winternitz key, so it waits until every other account of the `Advance`
//!   has signed, or signs in the same press.
//! - A hash-based key signs only as the key the account holds now. After a
//!   rotation lands, the next key has to be installed first.
//! - An `Advance` that hands the account to a new key is signed only when
//!   that key is the one this device made for it. The device that builds the
//!   transaction chooses the next key, and the device that holds the key does
//!   not take its word for it.

use super::vector::{message_of, next_key_of, slots, write, AuthContext, VectorContext};
use super::*;
use crate::wallets::vector::{
    parse_personal_sign, salt_of_signature, signer_digest, verify, SchemeId, VectorError,
    VectorKey, XmssRetryCache,
};
use std::collections::HashMap;

/// Sign every open place of an `Advance` whose account one of `keys` signs
/// for, at the nonce `ctx` gives it. A transaction with nothing for them
/// comes back unchanged.
pub fn sign_vector_authorizations(
    wire: &[u8],
    keys: &[VectorKey],
    ctx: &AuthContext,
) -> Result<Authorized, SignError> {
    sign_vector_authorizations_in(wire, keys, ctx, None)
}

/// The accounts of `held` that sign `tx` now, each with the digest it signs:
/// its place is known and empty, and a hash-based account is not held back
/// to sign last. One whose nonce `ctx` does not hold is an error: what it
/// signs depends on it.
fn signing_now(
    tx: &DecodedTx,
    held: &[Pubkey],
    ctx: &AuthContext,
) -> Result<Vec<(AuthoritySlot, [u8; 32])>, SignError> {
    // The Vector digest covers each `Submit`'s signature, so those
    // authorities have to sign first.
    if super::submit::slots(tx).iter().any(|slot| !slot.signed) {
        return Ok(Vec::new());
    }
    let all = slots(tx, ctx);
    let open: Vec<&AuthoritySlot> = all
        .iter()
        .filter(|slot| !slot.signed && slot.place.is_some() && held.contains(&slot.address))
        .collect();
    let stateful = |slot: &AuthoritySlot| {
        ctx.of(&slot.address)
            .is_some_and(|known| known.scheme.is_stateful())
    };
    // Every other account has signed, with nothing known against it, or
    // signs in this press.
    let others_in = |slot: &AuthoritySlot| {
        all.iter()
            .filter(|other| other.address != slot.address)
            .all(|other| {
                (other.signed && other.verified != Some(false))
                    || open.iter().any(|mine| mine.address == other.address)
            })
    };
    let ready: Vec<&AuthoritySlot> = open
        .iter()
        .copied()
        .filter(|slot| !stateful(slot) || others_in(slot))
        .collect();
    if ready.is_empty() {
        return Ok(Vec::new());
    }
    let message = message_of(tx)?;
    ready
        .into_iter()
        .map(|slot| {
            let nonce = ctx
                .of(&slot.address)
                .and_then(|known| known.nonce)
                .ok_or(SignError::NoNonce(slot.address))?;
            let next_key = next_key_of(tx, slot);
            Ok((
                *slot,
                signer_digest(&message, &nonce, &slot.address, next_key),
            ))
        })
        .collect()
}

/// The digest each account of `held` signs in `wire` now, by the rules of
/// [`sign_vector_authorizations`]. What a hash-based key is about to sign is
/// recorded from this before it signs, and nothing else is.
pub fn digests_to_sign(
    wire: &[u8],
    held: &[Pubkey],
    known: &[VectorContext],
) -> HashMap<Pubkey, [u8; 32]> {
    let Ok(tx) = decode_v1_transaction(wire) else {
        return HashMap::new();
    };
    signing_now(&tx, held, &AuthContext { vector: known })
        .map(|now| {
            now.into_iter()
                .map(|(slot, digest)| (slot.address, digest))
                .collect()
        })
        .unwrap_or_default()
}

/// Refuse to sign for `key`'s account when its place in the `Advance` hands
/// it to a key this device did not make, or when a hash-based key here is
/// not the one the account holds now.
fn check_key(
    tx: &DecodedTx,
    slot: &AuthoritySlot,
    key: &VectorKey,
    ctx: &AuthContext,
) -> Result<(), SignError> {
    let scheme = key.signer.scheme();
    if scheme.is_stateful() {
        let next = next_key_of(tx, slot);
        // XMSS names the key that signs now to keep it. A Winternitz key
        // signs once, and the program refuses it again.
        let keeps = next == key.signer.key();
        if keeps && scheme == SchemeId::Winternitz {
            return Err(VectorError::KeyReused(key.account).into());
        }
        if !keeps && key.next_key.as_deref() != Some(next) {
            return Err(VectorError::RotatesElsewhere(key.account).into());
        }
        let current = ctx.of(&key.account).and_then(|known| known.key.as_deref());
        if current.is_some_and(|current| current != key.signer.key()) {
            return Err(VectorError::NotCurrentKey(key.account).into());
        }
    }
    Ok(())
}

/// [`sign_vector_authorizations`], remembering XMSS salts in `cache` so a
/// retry over the same digest reuses the leaf.
pub fn sign_vector_authorizations_in(
    wire: &[u8],
    keys: &[VectorKey],
    ctx: &AuthContext,
    mut cache: Option<&mut XmssRetryCache>,
) -> Result<Authorized, SignError> {
    let tx = decode(wire)?;
    let held: Vec<Pubkey> = keys.iter().map(|key| key.account).collect();
    let now = signing_now(&tx, &held, ctx)?;
    if now.is_empty() {
        return Ok(Authorized {
            wire: tx.wire.clone(),
            signed: 0,
            dropped: 0,
        });
    }
    let mut filled = Vec::new();
    for (slot, digest) in &now {
        let Some(key) = keys.iter().find(|key| key.account == slot.address) else {
            continue;
        };
        let (start, len) = slot.place.ok_or(VectorError::AdvanceMismatch)?;
        if len != key.signer.scheme().signature_len() {
            return Err(VectorError::AdvanceMismatch.into());
        }
        check_key(&tx, slot, key, ctx)?;
        let signature = match key.signer.scheme() {
            SchemeId::Xmss | SchemeId::Winternitz => {
                let cached = cache
                    .as_ref()
                    .and_then(|cache| cache.get(&slot.address, digest))
                    .map(|cached| (cached.leaf, Some(cached.salt)));
                let (leaf, salt) = match (key.leaf, key.salt, cached) {
                    (Some(leaf), Some(salt), _) => (leaf, Some(salt)),
                    (_, _, Some(cached)) => cached,
                    (Some(leaf), None, None) => (leaf, None),
                    (None, _, None) => return Err(VectorError::NotReserved(slot.address).into()),
                };
                let signature = match salt {
                    Some(salt) => key.signer.sign_leaf_with_salt(leaf, digest, &salt)?,
                    None => key.signer.sign_leaf(leaf, digest)?,
                };
                if let (Some(cache), Some(salt)) = (
                    cache.as_mut(),
                    salt_of_signature(key.signer.scheme(), &signature),
                ) {
                    cache.remember(slot.address, leaf, *digest, salt);
                }
                signature
            }
            _ => key.signer.sign(digest)?,
        };
        filled.push((slot.instruction, start, signature));
    }
    let signed = filled.len();
    let (wire, dropped) = rebuild(&tx, |message| {
        for (instruction, start, signature) in &filled {
            write(message, *instruction, *start, signature)?;
        }
        Ok(())
    })?;
    Ok(Authorized {
        wire,
        signed,
        dropped,
    })
}

/// Write a signature for `account`, made on another device at `nonce`,
/// into the `Advance` that lists it.
///
/// The nonce has to be the one `ctx` holds for the account, when it holds
/// one: a signature at another nonce cannot land. With the account's key in
/// `ctx` too, the signature is checked before it is written.
pub fn insert_vector_signature(
    wire: &[u8],
    account: &Pubkey,
    nonce: &[u8; 32],
    signature: &[u8],
    ctx: &AuthContext,
) -> Result<Authorized, SignError> {
    let tx = decode(wire)?;
    if super::submit::slots(&tx).iter().any(|slot| !slot.signed) {
        return Err(SignError::Message(
            "the programmatic authorities have to sign before the Vector key".into(),
        ));
    }
    let slot = slots(&tx, ctx)
        .into_iter()
        .find(|slot| slot.address == *account)
        .ok_or_else(|| {
            SignError::Message("this account is not an authority of this transaction".into())
        })?;
    let (start, len) = slot.place.ok_or_else(|| {
        SignError::Message(
            "where this account's signature goes depends on its scheme. Scan the transaction's context note first."
                .into(),
        )
    })?;
    if signature.len() != len {
        return Err(SignError::Message(format!(
            "this account's signature is {len} bytes, and this one is {}",
            signature.len()
        )));
    }
    if let Some(known) = ctx.of(account) {
        if known.nonce.is_some_and(|current| current != *nonce) {
            return Err(SignError::Message(
                "this was signed at another nonce than the account holds now: it has acted since. Rebuild the transaction."
                    .into(),
            ));
        }
        if let Some(key) = &known.key {
            let digest = signer_digest(&message_of(&tx)?, nonce, account, next_key_of(&tx, &slot));
            verify(known.scheme, key, &digest, signature)?;
        }
    }
    let (wire, dropped) = rebuild(&tx, |message| {
        write(message, slot.instruction, start, signature)
    })?;
    Ok(Authorized {
        wire,
        signed: 1,
        dropped,
    })
}

/// The digest the EIP-191 account `account` of `tx` signs with an Ethereum
/// wallet's `personal_sign`, at the nonce `ctx` holds for it.
pub fn personal_sign_digest(
    tx: &DecodedTx,
    account: &Pubkey,
    ctx: &AuthContext,
) -> Result<[u8; 32], SignError> {
    let known = ctx.of(account).ok_or_else(|| {
        SignError::Message(
            "Scan the transaction's context note before pasting a personal_sign result.".into(),
        )
    })?;
    if known.scheme != SchemeId::Eip191 {
        return Err(SignError::Message(
            "personal_sign is only for an EIP-191 Vector account.".into(),
        ));
    }
    let nonce = known.nonce.ok_or(SignError::NoNonce(*account))?;
    // An EIP-191 key never changes, so the Advance names no next key for it.
    Ok(signer_digest(&message_of(tx)?, &nonce, account, &[]))
}

/// Write a `personal_sign` result pasted from an Ethereum wallet into the
/// `Advance` of `wire`, for the EIP-191 account `account`. Its Ethereum
/// address is the one `ctx` holds, or else `recorded`, from the wallet's
/// record on this device.
pub fn insert_personal_sign(
    wire: &[u8],
    account: &Pubkey,
    pasted: &str,
    ctx: &AuthContext,
    recorded: Option<&[u8]>,
) -> Result<Authorized, SignError> {
    let tx = decode(wire)?;
    let digest = personal_sign_digest(&tx, account, ctx)?;
    let known = ctx.of(account).ok_or(SignError::NoNonce(*account))?;
    let nonce = known.nonce.ok_or(SignError::NoNonce(*account))?;
    let address = known.key.as_deref().or(recorded).ok_or_else(|| {
        SignError::Message(
            "This account's Ethereum address is unknown. Scan the context note, or unlock the vault that holds the wallet."
                .into(),
        )
    })?;
    let signature = parse_personal_sign(address, &digest, pasted)?;
    insert_vector_signature(wire, account, &nonce, &signature, ctx)
}
