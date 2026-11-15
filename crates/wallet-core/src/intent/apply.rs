//! Writing the replaced addresses into a request's transactions: derived
//! accounts derived again, the account lists and the `data` entries
//! rewritten, and every signature dropped.

use super::check::{derive, undeclared};
use super::error::NoteError;
use super::notes::{AccountNote, DataHolds, Derivation, Notes};
use crate::tx::renew::{parts, read_message, unsigned_wire};
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet};

/// Every address a request's rewrite replaces, and the new bump of each
/// program-derived one derived again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Replacements {
    /// Old to new: the addresses the plan replaced, and each derived account
    /// that rests on one of them, at any depth.
    pub addresses: BTreeMap<Pubkey, Pubkey>,
    /// Each program-derived account derived again, by its old address, with
    /// its new canonical bump.
    pub bumps: BTreeMap<Pubkey, u8>,
}

/// `replaced`, the addresses the plan replaces (see
/// [`Plan::replaced`](super::Plan::replaced)), with every `derived` account
/// that rests on one of them derived again, to any depth. Each address in
/// `replaced` has to carry a note that lets the wallet replace it.
pub fn replacements(
    notes: &Notes,
    replaced: &BTreeMap<Pubkey, Pubkey>,
) -> Result<Replacements, NoteError> {
    for address in replaced.keys() {
        if !notes
            .accounts
            .get(address)
            .is_some_and(AccountNote::replaceable)
        {
            return Err(NoteError::NotReplaceable { address: *address });
        }
    }
    let mut out = Replacements {
        addresses: replaced.clone(),
        bumps: BTreeMap::new(),
    };
    for (address, note) in notes.accounts.iter() {
        if matches!(note, AccountNote::Derived(_)) {
            resolve(notes, address, &mut out, &mut Vec::new())?;
        }
    }
    Ok(out)
}

/// The address `address` ends up as, deriving it and what it rests on
/// again as needed. `path` holds the derived accounts being resolved, to
/// find a loop.
fn resolve(
    notes: &Notes,
    address: &Pubkey,
    out: &mut Replacements,
    path: &mut Vec<Pubkey>,
) -> Result<Pubkey, NoteError> {
    if let Some(new) = out.addresses.get(address) {
        return Ok(*new);
    }
    let Some(derivation) = notes.derivation(address) else {
        return Ok(*address);
    };
    if path.contains(address) {
        return Err(NoteError::Loop { address: *address });
    }
    path.push(*address);
    let mut inputs = BTreeMap::new();
    for input in derivation.inputs() {
        let resolved = resolve(notes, &input, out, path)?;
        inputs.insert(input, resolved);
    }
    path.pop();
    if inputs.iter().all(|(old, new)| old == new) {
        return Ok(*address);
    }
    let (derived, bump) = derive(derivation, |key| inputs.get(key).copied().unwrap_or(*key))
        .map_err(|reason| NoteError::NotDerivable {
            address: *address,
            reason,
        })?;
    out.addresses.insert(*address, derived);
    if let (Derivation::Pda(_), Some(bump)) = (derivation, bump) {
        out.bumps.insert(*address, bump);
    }
    Ok(derived)
}

/// `wire`, transaction `transaction` of the request, with `replacements`
/// written in: each replaced address in its static account keys, each new
/// address and bump at the `data` entries of this transaction. The account
/// order and the header stay as they are, and every signature slot is
/// emptied. Works on legacy, v0 and v1 wires.
///
/// Refused when a replaced address is left in instruction data that no
/// `data` entry names (check 5), and when two accounts would end up with
/// the same address.
pub fn apply_notes(
    wire: &[u8],
    transaction: usize,
    notes: &Notes,
    replacements: &Replacements,
) -> Result<Vec<u8>, NoteError> {
    let unreadable = |reason: String| NoteError::Unreadable {
        transaction,
        reason,
    };
    let mut message = read_message(wire).map_err(|err| unreadable(err.to_string()))?;
    {
        let (keys, instructions) = parts(&mut message);
        for key in keys.iter_mut() {
            if let Some(new) = replacements.addresses.get(key) {
                *key = *new;
            }
        }
        let mut listed = BTreeSet::new();
        for key in keys.iter() {
            if !listed.insert(*key) {
                return Err(NoteError::SameAddress {
                    transaction,
                    address: *key,
                });
            }
        }

        for (entry, note) in notes.data.iter().enumerate() {
            if note.transaction != transaction {
                continue;
            }
            let Some(ix) = instructions.get_mut(note.instruction) else {
                return Err(NoteError::NoSuchInstruction {
                    entry,
                    transaction,
                    instruction: note.instruction,
                    count: instructions.len(),
                });
            };
            let data_len = ix.data.len();
            let out_of_range = || NoteError::OutOfRange {
                entry,
                offset: note.offset,
                len: note.width(),
                data_len,
            };
            let end = note
                .offset
                .checked_add(note.width())
                .ok_or_else(out_of_range)?;
            let bytes = ix.data.get_mut(note.offset..end).ok_or_else(out_of_range)?;
            match note.holds {
                DataHolds::Account(address) => {
                    if let Some(new) = replacements.addresses.get(&address) {
                        bytes.copy_from_slice(new.as_ref());
                    }
                }
                DataHolds::BumpOf(address) => {
                    if let Some(bump) = replacements.bumps.get(&address) {
                        bytes[0] = *bump;
                    }
                }
            }
        }

        // Check 5: what is left of a replaced address was not declared.
        let written = Notes::default();
        for (instruction, ix) in instructions.iter().enumerate() {
            for old in replacements.addresses.keys() {
                if let Some(offset) = undeclared(&written, transaction, instruction, &ix.data, old)
                {
                    return Err(NoteError::UndeclaredUse {
                        address: *old,
                        transaction,
                        instruction,
                        offset,
                    });
                }
            }
        }
    }
    unsigned_wire(message).map_err(|err| unreadable(err.to_string()))
}

/// [`replacements`], then [`apply_notes`] on each of `wires`, the request's
/// transactions in its order. The new wires, and every address replaced.
pub fn apply_to_request(
    wires: &[Vec<u8>],
    notes: &Notes,
    replaced: &BTreeMap<Pubkey, Pubkey>,
) -> Result<(Vec<Vec<u8>>, Replacements), NoteError> {
    let replacements = replacements(notes, replaced)?;
    let wires = wires
        .iter()
        .enumerate()
        .map(|(transaction, wire)| apply_notes(wire, transaction, notes, &replacements))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((wires, replacements))
}
