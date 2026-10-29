//! The authorization of the Vector program: one `Advance` that lists every
//! Vector account the transaction acts as and holds each one's signature,
//! then a `Passthrough` for each run of instructions one of them signs.
//!
//! The `Advance` names accounts and nothing else. Its data holds, for each
//! account in turn, the key it signs with next (Winternitz and XMSS only)
//! and its signature. How long they are, and so where they sit, follows from
//! the account's scheme, which comes from the context: the account read from
//! a node, a wallet record, or a note from another device. Without it the
//! place is still known when the data's length allows only one reading, as
//! it does for any number of Ed25519 accounts.

use super::*;
use crate::transport::handoff::SignatureObject;
use crate::transport::protocol::ContextAuthorization;
use crate::wallets::vector::{
    advance_accounts, advance_message, demoted_writable, signer_digest, verify, SchemeId,
    VectorError, ADVANCE_DISCRIMINATOR, PROGRAM_ID,
};
use solana_instruction::Instruction;

/// What this device knows of one Vector account beyond the transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorContext {
    pub account: Pubkey,
    pub scheme: SchemeId,
    /// What its next signature has to be made over, when known: read from
    /// the chain, or from the note of the device that did.
    pub nonce: Option<[u8; 32]>,
    /// The key that verifies its signatures, when known: the one the account
    /// keeps, or the one a wallet record was made with.
    pub key: Option<Vec<u8>>,
    /// For a key that signs once per leaf, the lowest leaf the chain still
    /// accepts. Zero otherwise.
    pub leaf_floor: u32,
}

impl VectorContext {
    /// The key that signs for this account, as a Solana address, when it is
    /// one: Ed25519's Initialize key is a 32-byte pubkey. Other schemes keep
    /// a different shape and stay off this chip.
    pub fn signing_pubkey(&self) -> Option<Pubkey> {
        let key = self.key.as_ref()?;
        match self.scheme {
            SchemeId::Ed25519 if key.len() == 32 => {
                Some(Pubkey::new_from_array(key.as_slice().try_into().ok()?))
            }
            _ => None,
        }
    }

    /// What a context note carries of this account: scheme, nonce and leaf
    /// floor, and never the key.
    pub fn for_note(&self) -> Option<ContextAuthorization> {
        Some(ContextAuthorization {
            account: self.account.to_bytes(),
            scheme: self.scheme.byte(),
            nonce: self.nonce?,
            leaf_floor: self.leaf_floor,
        })
    }

    /// What a context note said of an account. The key is not in the note.
    pub fn from_note(authorization: &ContextAuthorization) -> Option<Self> {
        Some(Self {
            account: Pubkey::new_from_array(authorization.account),
            scheme: SchemeId::from_byte(authorization.scheme)?,
            nonce: Some(authorization.nonce),
            key: None,
            leaf_floor: authorization.leaf_floor,
        })
    }
}

/// The authorization entries a context note carries for `known`.
pub fn authorizations_for_note(known: &[VectorContext]) -> Vec<ContextAuthorization> {
    known.iter().filter_map(VectorContext::for_note).collect()
}

/// Lay what a context note says of its Vector accounts over what `known`
/// already holds. The note gives the nonce and leaf floor; the key, when
/// `known` has one, stays. `recorded` answers the scheme of a Vector wallet
/// of this device at an address.
///
/// The scheme decides where an account's signature sits, whether it signs
/// last and whether it spends a leaf, so the note does not get to change
/// it: a note that names another scheme than the wallet here, or than what
/// was read before, is refused whole and `known` is left as it was.
pub fn overlay_note(
    known: &mut Vec<VectorContext>,
    authorizations: &[ContextAuthorization],
    recorded: impl Fn(&Pubkey) -> Option<SchemeId>,
) -> Result<(), VectorError> {
    let mut merged = known.clone();
    for authorization in authorizations {
        let Some(from_note) = VectorContext::from_note(authorization) else {
            continue;
        };
        let expected = merged
            .iter()
            .find(|known| known.account == from_note.account)
            .map(|known| known.scheme)
            .or_else(|| recorded(&from_note.account));
        if expected.is_some_and(|scheme| scheme != from_note.scheme) {
            return Err(VectorError::NoteScheme(from_note.account));
        }
        match merged
            .iter_mut()
            .find(|known| known.account == from_note.account)
        {
            Some(existing) => {
                existing.nonce = from_note.nonce;
                existing.leaf_floor = from_note.leaf_floor;
            }
            None => merged.push(from_note),
        }
    }
    *known = merged;
    Ok(())
}

/// What signing or checking an authorization needs beyond the transaction.
#[derive(Clone, Copy, Debug, Default)]
pub struct AuthContext<'a> {
    pub vector: &'a [VectorContext],
}

impl AuthContext<'_> {
    pub fn of(&self, account: &Pubkey) -> Option<&VectorContext> {
        self.vector.iter().find(|known| known.account == *account)
    }
}

/// Where each account's next key and signature are in the data of an
/// `Advance`, as `(signature start, signature length, next key length)`. The
/// next key comes just before the signature. `None` when the context leaves
/// more than one reading.
fn places(
    accounts: &[Pubkey],
    data_len: usize,
    ctx: &AuthContext,
) -> Option<Vec<(usize, usize, usize)>> {
    // What one account can take, as (next key, signature) lengths. Schemes
    // that take the same bytes read the same.
    let mut shapes: Vec<(usize, usize)> = SchemeId::ALL
        .map(|scheme| (scheme.next_key_len(), scheme.signature_len()))
        .to_vec();
    shapes.sort_unstable_by_key(|(next, signature)| next + signature);
    shapes.dedup();
    let options: Vec<Vec<(usize, usize)>> = accounts
        .iter()
        .map(|account| match ctx.of(account) {
            Some(known) => vec![(known.scheme.next_key_len(), known.scheme.signature_len())],
            None => shapes.clone(),
        })
        .collect();
    // Each unknown account multiplies the readings to try. Past a few, ask
    // for the context instead.
    if options.iter().filter(|choice| choice.len() > 1).count() > 8 {
        return None;
    }
    let mut found = Vec::new();
    let mut chosen = Vec::with_capacity(accounts.len());
    readings(&options, data_len.checked_sub(1)?, &mut chosen, &mut found);
    let [only] = found.as_slice() else {
        return None;
    };
    let mut start = 1;
    Some(
        only.iter()
            .map(|(next, signature)| {
                let place = (start + next, *signature, *next);
                start += next + signature;
                place
            })
            .collect(),
    )
}

/// Collect up to two ways of choosing one shape per account whose lengths
/// add up to `left`. Two are enough to know the reading is not the only one.
fn readings(
    options: &[Vec<(usize, usize)>],
    left: usize,
    chosen: &mut Vec<(usize, usize)>,
    found: &mut Vec<Vec<(usize, usize)>>,
) {
    let Some((first, rest)) = options.split_first() else {
        if left == 0 {
            found.push(chosen.clone());
        }
        return;
    };
    let least: usize = rest
        .iter()
        .filter_map(|choice| choice.first())
        .map(|(next, signature)| next + signature)
        .sum();
    for &(next, signature) in first {
        if found.len() > 1 {
            return;
        }
        let len = next + signature;
        if len + least > left {
            continue;
        }
        chosen.push((next, signature));
        readings(rest, left - len, chosen, found);
        chosen.pop();
    }
}

/// The transaction's `Advance`s, each with its index and listed accounts.
fn advances(tx: &DecodedTx) -> impl Iterator<Item = (usize, &[Pubkey])> + '_ {
    tx.instructions
        .iter()
        .enumerate()
        .filter(|(_, instruction)| {
            instruction.program_id == PROGRAM_ID
                && instruction.data.first() == Some(&ADVANCE_DISCRIMINATOR)
        })
        .filter_map(|(index, instruction)| {
            let (_sysvar, listed) = instruction.accounts.split_last()?;
            Some((index, listed))
        })
}

/// Every account the transaction's `Advance`s list.
pub(super) fn slots(tx: &DecodedTx, ctx: &AuthContext) -> Vec<AuthoritySlot> {
    let instructions = tx.native_instructions();
    let message = tx
        .fee_payer()
        .and_then(|payer| advance_message(&instructions, &payer).ok());
    advances(tx)
        .flat_map(|(index, listed)| {
            let data = &tx.instructions[index].data;
            let places = places(listed, data.len(), ctx);
            listed
                .iter()
                .enumerate()
                .map(|(at, account)| {
                    let found = places.as_ref().map(|places| places[at]);
                    let place = found.map(|(start, len, _)| (start, len));
                    let next_key_len = found.map_or(0, |(_, _, next)| next);
                    let signature = place.and_then(|(start, len)| data.get(start..start + len));
                    let next_key =
                        place.and_then(|(start, _)| data.get(start - next_key_len..start));
                    let signed = signature.is_some_and(|bytes| bytes.iter().any(|byte| *byte != 0));
                    let verified = match (signature, next_key, ctx.of(account), &message) {
                        (Some(signature), Some(next_key), Some(known), Some(message)) if signed => {
                            match (&known.nonce, &known.key) {
                                (Some(nonce), Some(key)) => Some(
                                    verify(
                                        known.scheme,
                                        key,
                                        &signer_digest(message, nonce, account, next_key),
                                        signature,
                                    )
                                    .is_ok(),
                                ),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    AuthoritySlot {
                        instruction: index,
                        authority: AuthorityId::Vector(*account),
                        address: *account,
                        place,
                        next_key_len,
                        signed,
                        verified,
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The message of `tx` that its Vector accounts sign, refused when the
/// runtime would flag an account otherwise than the message does.
pub(super) fn message_of(tx: &DecodedTx) -> Result<[u8; 32], SignError> {
    let payer = tx
        .fee_payer()
        .ok_or_else(|| SignError::Message("the transaction has no fee payer".into()))?;
    let instructions: Vec<Instruction> = tx.native_instructions();
    if let Some(key) = demoted_writable(&instructions, &payer) {
        return Err(VectorError::DemotedWritable(key).into());
    }
    Ok(advance_message(&instructions, &payer)?)
}

/// One object per Vector signature in `tx` whose nonce `ctx` holds. Each
/// says which account it is for and at which nonce it was made.
pub fn vector_signature_objects(tx: &DecodedTx, ctx: &AuthContext) -> Vec<SignatureObject> {
    slots(tx, ctx)
        .into_iter()
        .filter(|slot| slot.signed)
        .filter_map(|slot| {
            let (start, len) = slot.place?;
            let nonce = ctx.of(&slot.address)?.nonce?;
            let signature = tx.instructions[slot.instruction]
                .data
                .get(start..start + len)?
                .to_vec();
            Some(SignatureObject {
                pubkey: slot.address,
                signed: Signed::AccountAuthorization {
                    account: slot.address.to_bytes(),
                    nonce,
                    signature,
                },
            })
        })
        .collect()
}

/// The key `slot`'s account signs with next, as the `Advance` names it:
/// empty for a scheme whose key never changes, and when the place is
/// unknown.
pub(super) fn next_key_of<'a>(tx: &'a DecodedTx, slot: &AuthoritySlot) -> &'a [u8] {
    slot.place
        .and_then(|(start, _)| {
            tx.instructions[slot.instruction]
                .data
                .get(start - slot.next_key_len..start)
        })
        .unwrap_or_default()
}

/// The key each Winternitz and XMSS account of the transaction's
/// `Advance`s signs with next, with its scheme, for every account whose place
/// is known. The scheme is the one `ctx` gives, or else the only one whose
/// lengths fit the place: no two schemes that rotate sign alike.
pub fn advance_next_keys(tx: &DecodedTx, ctx: &AuthContext) -> Vec<(Pubkey, SchemeId, Vec<u8>)> {
    slots(tx, ctx)
        .into_iter()
        .filter(|slot| slot.next_key_len > 0)
        .filter_map(|slot| {
            let (_, len) = slot.place?;
            let scheme = ctx
                .of(&slot.address)
                .map(|known| known.scheme)
                .or_else(|| {
                    SchemeId::ALL.into_iter().find(|scheme| {
                        scheme.next_key_len() == slot.next_key_len && scheme.signature_len() == len
                    })
                })?;
            Some((slot.address, scheme, next_key_of(tx, &slot).to_vec()))
        })
        .collect()
}

/// The accounts the transaction's one `Advance` lists. Empty when it has
/// none, or more than one.
pub fn vector_accounts(tx: &DecodedTx) -> Vec<Pubkey> {
    advance_accounts(&tx.native_instructions()).unwrap_or_default()
}

pub(super) fn write(
    message: &mut v1::Message,
    instruction: usize,
    start: usize,
    signature: &[u8],
) -> Result<(), SignError> {
    message
        .instructions
        .get_mut(instruction)
        .and_then(|instruction| instruction.data.get_mut(start..start + signature.len()))
        .ok_or_else(|| SignError::Message("the Advance has no room for this signature".into()))?
        .copy_from_slice(signature);
    Ok(())
}
