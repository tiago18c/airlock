//! The proposals of a Squads multisig, as a member of it sees them.
//!
//! A multisig numbers its transactions. Each has a stored transaction account
//! and, once voting was opened, a proposal account. Both sit at addresses
//! derived from the number, so a page of them is read by address, newest
//! first, without any index of the chain.
//!
//! A stored transaction may read some of its accounts from address lookup
//! tables, as other apps store swaps and other large transactions. Those
//! tables are read with the page ([`page_tables`]), and such a transaction
//! reads like any other once they are (see `squads::TableProblem` for when
//! it cannot be).
//!
//! What a member may do to a proposal follows from its status, the member's
//! permissions, the threshold and the time lock. This module decides that and
//! builds the instructions. Signing happens elsewhere. Who voted how, and how
//! the programs count those votes, is in `votes`.
//!
//! A member may do two things at once to an open proposal: move it forward
//! (approve, and execute when that approval completes it) and reject it.
//! [`member_choices`] gives both. Moving it forward needs the stored
//! transaction read; rejecting does not, since saying no to what one cannot
//! read is the safe answer.

use crate::chain::RawAccount;
use crate::ids::ADDRESS_LOOKUP_TABLE_PROGRAM_ID;
use crate::wallets::squads::{
    ProposalState, ProposalStatus, Squad, SquadState, StoredAccount, TableProblem,
    VaultTransaction, PERMISSION_EXECUTE, PERMISSION_VOTE,
};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use thiserror::Error;

mod combined;
mod shape;
mod votes;

#[cfg(test)]
mod tests;

pub use combined::*;
pub use shape::*;
pub use votes::*;

/// How many transaction numbers one page covers.
pub const PROPOSALS_PER_PAGE: u64 = 10;

/// What is stored under one transaction number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    /// Instructions a vault runs. Those that read accounts from lookup
    /// tables are here once the tables were read.
    Vault(VaultTransaction),
    /// A vault transaction that reads accounts from lookup tables, which
    /// could not all be read, and why.
    LookupTables(TableProblem),
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

/// The address lookup tables the stored transactions in `accounts` read
/// from, each once: what [`read_page`] needs besides. `accounts` is as
/// [`read_page`] takes it.
pub fn page_tables(squad: &Squad, accounts: &[Option<RawAccount>]) -> Vec<Pubkey> {
    let mut tables = Vec::new();
    for account in accounts.iter().skip(1).step_by(2).flatten() {
        if account.owner != squad.program.program_id() {
            continue;
        }
        if let Some(StoredAccount::LookupTables(bound)) = squad.parse_transaction(&account.data) {
            for table in bound.tables() {
                if !tables.contains(&table) {
                    tables.push(table);
                }
            }
        }
    }
    tables
}

/// Read a page. `accounts` holds what [`page_accounts`] asked for, in that
/// order, with `None` for an account that does not exist. `tables` holds
/// the lookup tables [`page_tables`] named, by address, as the node has
/// them: a table not there does not exist.
pub fn read_page(
    squad: &Squad,
    state: &SquadState,
    indexes: &[u64],
    accounts: &[Option<RawAccount>],
    tables: &HashMap<Pubkey, RawAccount>,
) -> Vec<ProposalEntry> {
    let owned = |position: usize| {
        accounts
            .get(position)
            .and_then(Option::as_ref)
            .filter(|account| account.owner == squad.program.program_id())
    };
    // Only what the lookup table program owns is a table.
    let tables: HashMap<Pubkey, Vec<u8>> = tables
        .iter()
        .filter(|(_, account)| account.owner == ADDRESS_LOOKUP_TABLE_PROGRAM_ID)
        .map(|(address, account)| (*address, account.data.clone()))
        .collect();
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
                    Some(StoredAccount::LookupTables(bound)) if bound.index == *index => {
                        match bound.resolve(&tables) {
                            Ok(tx) => Stored::Vault(tx),
                            Err(problem) => Stored::LookupTables(problem),
                        }
                    }
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
    /// A vote against it. [`available_action`] never gives it: whether a
    /// member may reject is [`available_rejection`], beside the action that
    /// moves the proposal forward.
    Reject,
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
    #[error("It is approved. The multisig's time lock holds it for {} more, then it can be executed.", wait_words(.until - .now))]
    TimeLock { until: i64, now: i64 },
    #[error("This proposal was left marked as being executed, a state the program no longer uses and never moves on from, so it cannot be executed.")]
    Executing,
    #[error(
        "This transaction cannot be read here, so it cannot be approved or executed from this app."
    )]
    Unreadable,
    #[error("The stored transaction is gone, so it cannot be executed.")]
    Gone,
    #[error("It was rejected for good. A rejected proposal cannot be reopened: to do what it holds, the transaction would have to be proposed again.")]
    Rejected,
    #[error("Nothing is left to do.")]
    Finished,
}

/// What `member` can do to move `entry` forward at time `now` (Unix
/// seconds): approve it, execute it, or both.
///
/// A transaction this app cannot read is never offered for approval: a vote
/// for something that was not read is not a vote. A member that rejected may
/// still approve while the proposal is open: the programs take its rejection
/// back.
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
        Stored::LookupTables(_) | Stored::Other => Err(NoAction::Unreadable),
    };
    let may_execute = permissions & PERMISSION_EXECUTE != 0;
    match proposal.status {
        ProposalStatus::Draft => Err(NoAction::Draft),
        ProposalStatus::Active => {
            if entry.stale {
                return Err(NoAction::Stale);
            }
            // Before reading: a member that may not vote could not approve
            // what it read either.
            if permissions & PERMISSION_VOTE == 0 {
                return Err(NoAction::MayNotVote);
            }
            readable?;
            let threshold = usize::from(state.threshold);
            if proposal.approved.contains(member) {
                return Err(NoAction::WaitingForOthers {
                    missing: threshold.saturating_sub(proposal.approved.len()),
                });
            }
            let completes = proposal.approved.len() + 1 >= threshold;
            Ok(if completes && state.time_lock == 0 && may_execute {
                ProposalAction::ApproveAndExecute
            } else {
                ProposalAction::Approve
            })
        }
        // Both programs execute an approved vault transaction that became
        // stale since: it was approved under the rules of its day.
        ProposalStatus::Approved => {
            readable?;
            if !may_execute {
                return Err(NoAction::MayNotExecute);
            }
            let until = time_lock_ends(state, proposal);
            if now < until {
                return Err(NoAction::TimeLock { until, now });
            }
            Ok(ProposalAction::Execute)
        }
        ProposalStatus::Executing => Err(NoAction::Executing),
        ProposalStatus::Rejected => Err(NoAction::Rejected),
        ProposalStatus::Executed | ProposalStatus::Cancelled => Err(NoAction::Finished),
    }
}

/// A rejection a member may cast, and what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejection {
    /// How many more rejections reject the proposal once this one is in.
    /// Zero when this one does: the proposal is then rejected for good.
    pub missing_after: usize,
    /// The member had approved. The programs take that approval back.
    pub replaces_approval: bool,
}

impl Rejection {
    /// This rejection reaches the cutoff, and the proposal is rejected for
    /// good.
    pub fn last(&self) -> bool {
        self.missing_after == 0
    }

    /// What it does, said before it is cast.
    pub fn hint(&self) -> String {
        let mut hint = String::new();
        if self.replaces_approval {
            hint.push_str("Rejecting takes back the acting member's approval. ");
        }
        hint.push_str(&match self.missing_after {
            0 => "This rejection is the last one needed: the proposal is rejected for good, and its transaction would have to be proposed again.".to_string(),
            1 => "After this rejection, 1 more rejects the proposal for good.".to_string(),
            more => format!("After this rejection, {more} more reject the proposal for good."),
        });
        hint
    }
}

/// Whether `member` may reject `entry`, and what its rejection would do.
/// `None` when it may not.
///
/// Both programs take a rejection from a member with the vote permission on
/// an open proposal that is not stale, whatever the stored transaction is
/// and whether or not it still exists: they read only the proposal. A
/// member's rejection takes back its approval, and a member that already
/// rejected cannot reject again.
pub fn available_rejection(
    state: &SquadState,
    entry: &ProposalEntry,
    member: &Pubkey,
) -> Option<Rejection> {
    let permissions = state
        .members
        .iter()
        .find(|known| known.key == *member)?
        .permissions;
    let proposal = entry.proposal.as_ref()?;
    let open = proposal.status == ProposalStatus::Active && !entry.stale;
    if !open || permissions & PERMISSION_VOTE == 0 || proposal.rejected.contains(member) {
        return None;
    }
    let rejected = vote_counts(state, proposal).rejected;
    Some(Rejection {
        missing_after: rejected.missing().saturating_sub(1),
        replaces_approval: proposal.approved.contains(member),
    })
}

/// Everything `member` may do to `entry` at time `now`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberChoices {
    /// What moves it forward, or why nothing does: [`available_action`].
    pub forward: Result<ProposalAction, NoAction>,
    /// Its rejection, when it may cast one: [`available_rejection`].
    pub reject: Option<Rejection>,
}

/// What `member` may do to `entry` at time `now` (Unix seconds): the action
/// that moves it forward and the rejection, each when the programs take it.
pub fn member_choices(
    state: &SquadState,
    entry: &ProposalEntry,
    member: &Pubkey,
    now: i64,
) -> MemberChoices {
    MemberChoices {
        forward: available_action(state, entry, member, now),
        reject: available_rejection(state, entry, member),
    }
}

/// When the time lock lets an approved `proposal` be executed, in Unix
/// seconds: the programs count from the moment it was approved.
pub fn time_lock_ends(state: &SquadState, proposal: &ProposalState) -> i64 {
    proposal.since.saturating_add(i64::from(state.time_lock))
}

/// A wait of `seconds`, in words, rounded up: "45 seconds", "3 minutes",
/// "2 hours", "4 days".
pub fn wait_words(seconds: i64) -> String {
    let seconds = seconds.max(1);
    let unit = |count: i64, one: &str| {
        if count == 1 {
            format!("1 {one}")
        } else {
            format!("{count} {one}s")
        }
    };
    let up = |of: i64| (seconds + of - 1) / of;
    match seconds {
        s if s < 120 => unit(s, "second"),
        s if s < 2 * 3_600 => unit(up(60), "minute"),
        s if s < 2 * 86_400 => unit(up(3_600), "hour"),
        _ => unit(up(86_400), "day"),
    }
}

/// Another wallet of this device that can execute `entry` when the acting
/// member cannot: the first of `chosen` (the acting member first, then
/// those it approves with) that is a member with the execute permission and
/// signs in time, which an offline key does not. `None` when the acting
/// member can execute it itself, or when it is not ready to be executed.
pub fn executor_instead<'a>(
    state: &SquadState,
    entry: &ProposalEntry,
    chosen: &'a [ChosenMember],
    now: i64,
) -> Option<&'a ChosenMember> {
    let acting = chosen.first()?;
    if available_action(state, entry, &acting.address, now) != Err(NoAction::MayNotExecute) {
        return None;
    }
    chosen.iter().skip(1).find(|member| {
        member.kind != MemberKind::Offline
            && member.permissions & PERMISSION_EXECUTE != 0
            && available_action(state, entry, &member.address, now) == Ok(ProposalAction::Execute)
    })
}

/// The instructions of `action` on `entry`, which `member` signs. `None` when
/// the action needs the stored transaction and it could not be read. A
/// rejection never does.
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
        ProposalAction::Reject => vec![squad.reject(entry.index, member)],
    })
}
