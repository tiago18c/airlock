//! The proposals of a Squads multisig, as a member of it sees them.
//!
//! A multisig numbers its transactions. Each has a stored transaction account
//! and, once voting was opened, a proposal account. Both sit at addresses
//! derived from the number, so a page of them is read by address, newest
//! first, without any index of the chain.
//!
//! What a member may do to a proposal follows from its status, the member's
//! permissions, the threshold and the time lock. This module decides that and
//! builds the instructions. Signing happens elsewhere.

use crate::chain::RawAccount;
use crate::wallets::squads::{
    ProposalState, ProposalStatus, Squad, SquadState, StoredAccount, VaultTransaction,
    PERMISSION_EXECUTE, PERMISSION_VOTE,
};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use thiserror::Error;

mod combined;
mod shape;

#[cfg(test)]
mod tests;

pub use combined::*;
pub use shape::*;

/// How many transaction numbers one page covers.
pub const PROPOSALS_PER_PAGE: u64 = 10;

/// What is stored under one transaction number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    /// Instructions a vault runs.
    Vault(VaultTransaction),
    /// A vault transaction that names accounts through address lookup tables.
    LookupTables,
    /// A change to the multisig itself, a batch, or a policy payload.
    Other,
    /// No account. It was closed, or never created.
    Missing,
}

/// One transaction number of a multisig.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalEntry {
    pub index: u64,
    /// `None` when no proposal was opened for the transaction.
    pub proposal: Option<ProposalState>,
    pub stored: Stored,
    /// The multisig's members, threshold or time lock changed after this was
    /// created. It can no longer be voted on.
    pub stale: bool,
}

/// The transaction numbers of the page that starts at `newest`, newest first.
pub fn page_indexes(newest: u64) -> Vec<u64> {
    let oldest = newest.saturating_sub(PROPOSALS_PER_PAGE - 1).max(1);
    (oldest..=newest).rev().collect()
}

/// The accounts to read for `indexes`: for each, its proposal and then its
/// stored transaction.
pub fn page_accounts(squad: &Squad, indexes: &[u64]) -> Vec<Pubkey> {
    indexes
        .iter()
        .flat_map(|index| [squad.proposal(*index), squad.transaction(*index)])
        .collect()
}

/// Read a page. `accounts` holds what [`page_accounts`] asked for, in that
/// order, with `None` for an account that does not exist.
pub fn read_page(
    squad: &Squad,
    state: &SquadState,
    indexes: &[u64],
    accounts: &[Option<RawAccount>],
) -> Vec<ProposalEntry> {
    let owned = |position: usize| {
        accounts
            .get(position)
            .and_then(Option::as_ref)
            .filter(|account| account.owner == squad.program.program_id())
    };
    indexes
        .iter()
        .enumerate()
        .map(|(at, index)| {
            let proposal = owned(2 * at)
                .and_then(|account| squad.parse_proposal(&account.data))
                .filter(|proposal| proposal.index == *index);
            let stored = match owned(2 * at + 1) {
                None => Stored::Missing,
                Some(account) => match squad.parse_transaction(&account.data) {
                    Some(StoredAccount::Vault(tx)) if tx.index == *index => Stored::Vault(tx),
                    Some(StoredAccount::LookupTables) => Stored::LookupTables,
                    _ => Stored::Other,
                },
            };
            ProposalEntry {
                index: *index,
                proposal,
                stored,
                stale: *index <= state.stale_transaction_index,
            }
        })
        .collect()
}

/// Something a member does to a proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalAction {
    Approve,
    Execute,
    /// This approval is the last one needed and nothing delays execution, so
    /// both go in one transaction.
    ApproveAndExecute,
}

/// Why a member can do nothing to a proposal right now.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NoAction {
    #[error("The acting wallet is not a member of this multisig on this cluster.")]
    NotAMember,
    #[error("No proposal was opened for this transaction, so there is nothing to vote on.")]
    NoProposal,
    #[error("This proposal is a draft. Its creator has to open it for votes.")]
    Draft,
    #[error(
        "The multisig changed after this proposal was created, so it can no longer be voted on."
    )]
    Stale,
    #[error("The acting member may not vote.")]
    MayNotVote,
    #[error("The acting member has approved. {missing} more approval(s) are needed.")]
    WaitingForOthers { missing: usize },
    #[error("It is approved. The acting member may not execute.")]
    MayNotExecute,
    /// `until` is in Unix seconds.
    #[error("It is approved. The multisig's time lock holds it for {} more second(s).", .until - .now)]
    TimeLock { until: i64, now: i64 },
    #[error(
        "This transaction cannot be read here, so it cannot be approved or executed from this app."
    )]
    Unreadable,
    #[error("The stored transaction is gone, so it cannot be executed.")]
    Gone,
    #[error("Nothing is left to do.")]
    Finished,
}

/// What `member` can do to `entry` at time `now` (Unix seconds).
///
/// A transaction this app cannot read is never offered for approval: a vote
/// for something that was not read is not a vote.
pub fn available_action(
    state: &SquadState,
    entry: &ProposalEntry,
    member: &Pubkey,
    now: i64,
) -> Result<ProposalAction, NoAction> {
    let permissions = state
        .members
        .iter()
        .find(|known| known.key == *member)
        .map(|known| known.permissions)
        .ok_or(NoAction::NotAMember)?;
    let proposal = entry.proposal.as_ref().ok_or(NoAction::NoProposal)?;
    let readable = match &entry.stored {
        Stored::Vault(_) => Ok(()),
        Stored::Missing => Err(NoAction::Gone),
        Stored::LookupTables | Stored::Other => Err(NoAction::Unreadable),
    };
    let may_execute = permissions & PERMISSION_EXECUTE != 0;
    match proposal.status {
        ProposalStatus::Draft => Err(NoAction::Draft),
        ProposalStatus::Active => {
            if entry.stale {
                return Err(NoAction::Stale);
            }
            readable?;
            let threshold = usize::from(state.threshold);
            if proposal.approved.contains(member) {
                return Err(NoAction::WaitingForOthers {
                    missing: threshold.saturating_sub(proposal.approved.len()),
                });
            }
            if permissions & PERMISSION_VOTE == 0 {
                return Err(NoAction::MayNotVote);
            }
            let completes = proposal.approved.len() + 1 >= threshold;
            Ok(if completes && state.time_lock == 0 && may_execute {
                ProposalAction::ApproveAndExecute
            } else {
                ProposalAction::Approve
            })
        }
        ProposalStatus::Approved => {
            readable?;
            if !may_execute {
                return Err(NoAction::MayNotExecute);
            }
            let until = proposal.since.saturating_add(i64::from(state.time_lock));
            if now < until {
                return Err(NoAction::TimeLock { until, now });
            }
            Ok(ProposalAction::Execute)
        }
        ProposalStatus::Rejected
        | ProposalStatus::Executing
        | ProposalStatus::Executed
        | ProposalStatus::Cancelled => Err(NoAction::Finished),
    }
}

/// The instructions of `action` on `entry`, which `member` signs. `None` when
/// the action needs the stored transaction and it could not be read.
pub fn action_instructions(
    squad: &Squad,
    entry: &ProposalEntry,
    member: &Pubkey,
    action: ProposalAction,
) -> Option<Vec<Instruction>> {
    let approve = || squad.approve(entry.index, member);
    let execute = || match &entry.stored {
        Stored::Vault(tx) => Some(squad.execute_stored(tx, member)),
        _ => None,
    };
    Some(match action {
        ProposalAction::Approve => vec![approve()],
        ProposalAction::Execute => vec![execute()?],
        ProposalAction::ApproveAndExecute => vec![approve(), execute()?],
    })
}
