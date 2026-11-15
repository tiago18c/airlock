//! Checking each note against the transactions it is about, before any is
//! used (the "Checks" of `docs/intents.md`).
//!
//! Check 3's second half, that the chain has never seen a `new` account,
//! needs a node: it is `wallet_flows::intent::new_accounts_unseen`.

use super::error::NoteError;
use super::notes::{AccountNote, DataHolds, Derivation, Notes};
use crate::tx::decode::DecodedTx;
use solana_pubkey::Pubkey;
use std::collections::BTreeSet;

/// Check `notes` against `transactions`, the request's transactions in its
/// order, decoded with the lookup tables a v0 one reads, for a request that
/// acts as `account`. The first note that fails is the error.
///
/// 1. Every address in `accounts`, and every address `data` names, is among
///    the static accounts of a transaction of the request, and no v0
///    transaction reads it from a lookup table.
/// 2. A `derived` note's derivation gives the address it is filed under,
///    and a `bumpOf` names a `pda` note.
/// 3. A `new` account signs a transaction of the request.
/// 4. Each `data` entry is inside its instruction's data and its bytes are
///    that address, or that bump.
/// 5. An address the notes let the wallet replace, or a derived one that
///    rests on such an address, appears in no instruction data that no
///    `data` entry names.
/// 6. An address carries one note, and `account` carries none: who pays for
///    the account acted as is the wallet's to choose, and nothing else about
///    it may change.
pub fn check_notes(
    transactions: &[DecodedTx],
    notes: &Notes,
    account: &Pubkey,
) -> Result<(), NoteError> {
    // 6.
    let mut seen = BTreeSet::new();
    for (address, _) in notes.accounts.iter() {
        if address == account {
            return Err(NoteError::OnAccount { address: *address });
        }
        if !seen.insert(*address) {
            return Err(NoteError::TwoNotes { address: *address });
        }
    }

    // 1.
    let named = notes
        .accounts
        .iter()
        .map(|(address, _)| *address)
        .chain(notes.data.iter().map(|entry| entry.address()));
    for address in named {
        in_transactions(transactions, &address)?;
    }

    // 2.
    for (address, note) in notes.accounts.iter() {
        if let AccountNote::Derived(derivation) = note {
            let (derived, _) =
                derive(derivation, |key| *key).map_err(|reason| NoteError::NotDerivable {
                    address: *address,
                    reason,
                })?;
            if derived != *address {
                return Err(NoteError::WrongDerivation {
                    address: *address,
                    derived,
                });
            }
        }
    }

    // 3.
    for (address, note) in notes.accounts.iter() {
        if matches!(note, AccountNote::New { .. })
            && !transactions
                .iter()
                .any(|tx| tx.signers.iter().any(|slot| slot.pubkey == *address))
        {
            return Err(NoteError::NewDoesNotSign { address: *address });
        }
    }

    // 4, and the second half of 2.
    for (entry, note) in notes.data.iter().enumerate() {
        let data = entry_data(transactions, entry, note)?;
        let end = note.offset.checked_add(note.width());
        let Some(bytes) = end.and_then(|end| data.get(note.offset..end)) else {
            return Err(NoteError::OutOfRange {
                entry,
                offset: note.offset,
                len: note.width(),
                data_len: data.len(),
            });
        };
        match note.holds {
            DataHolds::Account(address) => {
                if bytes != address.as_ref() {
                    return Err(NoteError::NotThatAddress {
                        entry,
                        offset: note.offset,
                        address,
                    });
                }
            }
            DataHolds::BumpOf(address) => {
                let Some(derivation @ Derivation::Pda(_)) = notes.derivation(&address) else {
                    return Err(NoteError::BumpOfNotPda { entry, address });
                };
                let (_, bump) = derive(derivation, |key| *key)
                    .map_err(|reason| NoteError::NotDerivable { address, reason })?;
                let bump = bump.unwrap_or_default();
                if bytes != [bump] {
                    return Err(NoteError::NotThatBump {
                        entry,
                        offset: note.offset,
                        address,
                        bump,
                    });
                }
            }
        }
    }

    // 5.
    for address in may_be_replaced(notes) {
        for (transaction, tx) in transactions.iter().enumerate() {
            for (instruction, ix) in tx.instructions.iter().enumerate() {
                if let Some(offset) =
                    undeclared(notes, transaction, instruction, &ix.data, &address)
                {
                    return Err(NoteError::UndeclaredUse {
                        address,
                        transaction,
                        instruction,
                        offset,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Check 1 for one address.
fn in_transactions(transactions: &[DecodedTx], address: &Pubkey) -> Result<(), NoteError> {
    let mut found = false;
    for (transaction, tx) in transactions.iter().enumerate() {
        for account in tx
            .accounts
            .iter()
            .filter(|account| account.pubkey == *address)
        {
            if account.lookup.is_some() {
                return Err(NoteError::FromLookupTable {
                    transaction,
                    address: *address,
                });
            }
            found = true;
        }
    }
    if found {
        Ok(())
    } else {
        Err(NoteError::NotInTransactions { address: *address })
    }
}

/// The instruction data a `data` entry is about.
fn entry_data<'a>(
    transactions: &'a [DecodedTx],
    entry: usize,
    note: &super::notes::DataNote,
) -> Result<&'a [u8], NoteError> {
    let tx = transactions
        .get(note.transaction)
        .ok_or(NoteError::NoSuchTransaction {
            entry,
            transaction: note.transaction,
            count: transactions.len(),
        })?;
    let ix = tx
        .instructions
        .get(note.instruction)
        .ok_or(NoteError::NoSuchInstruction {
            entry,
            transaction: note.transaction,
            instruction: note.instruction,
            count: tx.instructions.len(),
        })?;
    Ok(&ix.data)
}

/// Addresses the notes let the wallet replace, and every derived one that
/// rests on one of them, at any depth.
pub(crate) fn may_be_replaced(notes: &Notes) -> BTreeSet<Pubkey> {
    let mut found: BTreeSet<Pubkey> = notes
        .accounts
        .iter()
        .filter(|(_, note)| note.replaceable())
        .map(|(address, _)| *address)
        .collect();
    loop {
        let more: Vec<Pubkey> = notes
            .accounts
            .iter()
            .filter(|(address, _)| !found.contains(*address))
            .filter_map(|(address, note)| match note {
                AccountNote::Derived(derivation)
                    if derivation
                        .inputs()
                        .iter()
                        .any(|input| found.contains(input)) =>
                {
                    Some(*address)
                }
                _ => None,
            })
            .collect();
        if more.is_empty() {
            return found;
        }
        found.extend(more);
    }
}

/// Where `data`, instruction `instruction` of transaction `transaction`,
/// holds `address` at an offset no `data` entry names for it.
pub(crate) fn undeclared(
    notes: &Notes,
    transaction: usize,
    instruction: usize,
    data: &[u8],
    address: &Pubkey,
) -> Option<usize> {
    data.windows(32)
        .enumerate()
        .filter(|(_, window)| *window == address.as_ref())
        .map(|(offset, _)| offset)
        .find(|offset| {
            !notes.data.iter().any(|entry| {
                entry.transaction == transaction
                    && entry.instruction == instruction
                    && entry.offset == *offset
                    && entry.holds == DataHolds::Account(*address)
            })
        })
}

/// The address `derivation` gives, with each address it rests on taken
/// from `address`, and for a program-derived address its canonical bump.
pub(crate) fn derive(
    derivation: &Derivation,
    address: impl Fn(&Pubkey) -> Pubkey,
) -> Result<(Pubkey, Option<u8>), String> {
    match derivation {
        Derivation::Pda(pda) => {
            let seeds: Vec<Vec<u8>> = pda
                .seeds
                .iter()
                .map(|seed| seed.bytes_with(&address))
                .collect();
            if let Some(seed) = seeds
                .iter()
                .find(|seed| seed.len() > solana_pubkey::MAX_SEED_LEN)
            {
                return Err(format!(
                    "a seed of {} bytes is longer than the {} a program-derived address takes.",
                    seed.len(),
                    solana_pubkey::MAX_SEED_LEN
                ));
            }
            // One more seed is the bump.
            if seeds.len() >= solana_pubkey::MAX_SEEDS {
                return Err(format!(
                    "{} seeds are more than the {} a program-derived address takes besides its bump.",
                    seeds.len(),
                    solana_pubkey::MAX_SEEDS - 1
                ));
            }
            let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
            Pubkey::try_find_program_address(&refs, &address(&pda.program))
                .map(|(derived, bump)| (derived, Some(bump)))
                .ok_or_else(|| "no bump gives an address off the curve.".to_string())
        }
        Derivation::WithSeed(with_seed) => Pubkey::create_with_seed(
            &address(&with_seed.base),
            &with_seed.seed,
            &address(&with_seed.owner),
        )
        .map(|derived| (derived, None))
        .map_err(|err| format!("{err}.")),
    }
}
