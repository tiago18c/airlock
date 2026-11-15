//! What a transaction the wallet composed is to the request it resolves,
//! read from the transaction itself.

use crate::edit::{Carrier, CarrierKind};
use crate::ids::COMPUTE_BUDGET_PROGRAM_ID;
use crate::review::blind::{squads_call, SquadsCall};
use crate::review::StoredTarget;
use crate::tx::decode::DecodedTx;
use crate::wallets::squads::{Squad, SquadsProgram, StoredTransaction};
use serde::{Deserialize, Serialize};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

/// `role` of a resolution's transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TxRole {
    /// The account signs it, and its instructions run at the top.
    Direct,
    /// A program runs its instructions as the account: a programmatic
    /// signer's `Submit`, a Vector `Passthrough`, a smart account's
    /// `executeTransactionSync`, or a Squads proposal created, approved and
    /// executed in one transaction.
    Carrier,
    /// It stores the instructions in a Squads proposal and does not execute
    /// them.
    Proposal,
    /// It approves a Squads proposal, and nothing more.
    Vote,
    /// It executes a Squads proposal stored earlier.
    Execution,
}

/// What a Squads instruction found in a transaction does, with what it
/// acts on.
enum Found {
    Stores { squad: Squad, vault: Pubkey },
    Opens(StoredTarget),
    Approves,
    Runs { proposal: Option<Pubkey> },
    Sync { vault: Pubkey },
}

/// The role of `tx`, a transaction the wallet composed, for `account`, the
/// account the request acts as. Read through a programmatic signer's or a
/// Vector account's carrier, since a multisig's member may be one.
///
/// - `proposal` when it stores a Squads transaction for `account`'s vault
///   and does not execute that proposal;
/// - `carrier` when it does execute it, when a smart account runs the
///   instructions at once as `account`, or when a `Submit` or a
///   `Passthrough` carries them;
/// - `execution` when it executes a proposal stored earlier, `vote` when it
///   only approves one;
/// - `direct` when `account` signs it and no carrier wraps the
///   instructions.
///
/// `None` when it is none of these.
pub fn tx_role(tx: &DecodedTx, account: &Pubkey) -> Option<TxRole> {
    let mut found = Vec::new();
    let mut wrapped = false;
    walk(&own_instructions(tx), &mut found, &mut wrapped);

    let stored = found
        .iter()
        .any(|item| matches!(item, Found::Stores { vault, .. } if vault == account));
    let runs: Vec<Option<Pubkey>> = found
        .iter()
        .filter_map(|item| match item {
            Found::Runs { proposal } => Some(*proposal),
            _ => None,
        })
        .collect();
    if stored {
        // Opened and run in the same transaction: the instructions ran.
        let opened: Vec<Pubkey> = found
            .iter()
            .filter_map(|item| match item {
                Found::Opens(target) => Some(target.proposal),
                _ => None,
            })
            .collect();
        let ran = runs
            .iter()
            .any(|proposal| proposal.is_some_and(|proposal| opened.contains(&proposal)));
        return Some(if ran {
            TxRole::Carrier
        } else {
            TxRole::Proposal
        });
    }
    if found
        .iter()
        .any(|item| matches!(item, Found::Sync { vault } if vault == account))
    {
        return Some(TxRole::Carrier);
    }
    if !runs.is_empty() {
        return Some(TxRole::Execution);
    }
    if found.iter().any(|item| matches!(item, Found::Approves)) {
        return Some(TxRole::Vote);
    }
    if wrapped {
        return Some(TxRole::Carrier);
    }
    tx.signers
        .iter()
        .any(|slot| slot.pubkey == *account)
        .then_some(TxRole::Direct)
}

/// The proposal `tx` opens on a stored transaction it also stores, when it
/// does: what [`tx_role`] calls a `proposal`, and what to follow until it
/// runs.
pub fn proposal_opened(tx: &DecodedTx) -> Option<StoredTarget> {
    let mut found = Vec::new();
    walk(&own_instructions(tx), &mut found, &mut false);
    let stored: Vec<Squad> = found
        .iter()
        .filter_map(|item| match item {
            Found::Stores { squad, .. } => Some(*squad),
            _ => None,
        })
        .collect();
    found.into_iter().find_map(|item| match item {
        Found::Opens(target) if stored.contains(&target.squad) => Some(target),
        _ => None,
    })
}

fn own_instructions(tx: &DecodedTx) -> Vec<Instruction> {
    tx.native_instructions()
        .into_iter()
        .filter(|ix| ix.program_id != COMPUTE_BUDGET_PROGRAM_ID)
        .collect()
}

/// Every Squads instruction in `list`, opening a `Submit` or a
/// `Passthrough`. What a Squads proposal stores runs some other time, so it
/// is not opened. `wrapped` says a `Submit` or a `Passthrough` was found.
fn walk(list: &[Instruction], found: &mut Vec<Found>, wrapped: &mut bool) {
    for ix in list {
        if let Some(program) = SquadsProgram::from_program_id(&ix.program_id) {
            if let Some(item) = squads_item(program, ix) {
                found.push(item);
            }
            continue;
        }
        if let Some((carrier, inner)) = Carrier::open(ix) {
            if matches!(
                carrier.kind,
                CarrierKind::Programmatic { .. } | CarrierKind::Passthrough { .. }
            ) {
                *wrapped = true;
                walk(&inner, found, wrapped);
            }
        }
    }
}

fn squads_item(program: SquadsProgram, ix: &Instruction) -> Option<Found> {
    let account = |at: usize| ix.accounts.get(at).map(|meta| meta.pubkey);
    if let Some((
        Carrier {
            kind: CarrierKind::SquadsSync { vault, .. },
            ..
        },
        _,
    )) = Carrier::open(ix)
    {
        return Some(Found::Sync { vault });
    }
    let squad = Squad {
        program,
        address: account(0)?,
    };
    Some(match squads_call(program, &ix.data)? {
        SquadsCall::Stores => Found::Stores {
            squad,
            // A stored config change runs as no vault.
            vault: StoredTransaction::parse(ix).map_or(Pubkey::default(), |stored| stored.vault),
        },
        SquadsCall::Opens => {
            let number = u64::from_le_bytes(ix.data.get(8..16)?.try_into().ok()?);
            let proposal = account(1)?;
            if squad.proposal(number) != proposal {
                return None;
            }
            Found::Opens(StoredTarget {
                squad,
                proposal,
                transaction: Some(squad.transaction(number)),
            })
        }
        SquadsCall::Approves { .. } => Found::Approves,
        SquadsCall::Runs { proposal, .. } => Found::Runs {
            proposal: account(proposal),
        },
    })
}
