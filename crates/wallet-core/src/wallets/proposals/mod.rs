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
//! A member may do two things at once to a proposal: move it forward and
//! turn it down. [`member_choices`] gives both.
//!
//! - A draft is moved forward by opening it for votes (activating it), which
//!   any member that may propose can do while it is not stale. Activating
//!   approves nothing, so it needs no reading. A draft cannot be turned down:
//!   neither program rejects or cancels one. Left as it is, no one can vote
//!   on it or execute it.
//! - An open proposal is moved forward by approving it, and executing it
//!   when that approval completes it, and turned down by rejecting it.
//! - An approved proposal is moved forward by executing it, and turned down
//!   by voting to cancel it, also while its time lock holds it, and also once
//!   it is stale. It is cancelled for good once the cancel votes reach the
//!   threshold.
//!
//! Approving and executing need the stored transaction read. Activating,
//! rejecting and cancelling do not: opening a vote approves nothing, and
//! saying no to what one cannot read is the safe answer.
//!
//! What is stored may also be a change to the multisig itself (its members,
//! threshold, time lock, spending limits): read, it is approved and executed
//! like a vault transaction, with two differences the programs make. A
//! change whose result the program would refuse is never offered for
//! approval, since it can never be executed. And an approved change that
//! went stale can never be executed, where an approved vault transaction
//! still can.

use crate::chain::RawAccount;
use crate::ids::ADDRESS_LOOKUP_TABLE_PROGRAM_ID;
use crate::wallets::squads::{
    change_outcome, ChangeRefusal, ConfigTransaction, ProposalState, ProposalStatus, Squad,
    SquadState, StoredAccount, TableProblem, VaultTransaction, PERMISSION_EXECUTE,
    PERMISSION_INITIATE, PERMISSION_VOTE,
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
    /// A change to the multisig itself.
    Config(ConfigTransaction),
    /// A batch, a policy payload, or a change to the multisig holding an
    /// action this app cannot read.
    Other,
    /// No account. It was closed, or never created.
    Missing,
}

impl Stored {
    /// It was read: what it does is known.
    pub fn is_read(&self) -> bool {
        matches!(self, Self::Vault(_) | Self::Config(_))
    }

    /// This app builds its execution, and the program would take it as far
    /// as can be told at time `now`: a vault transaction, or a change to
    /// `state` this app executes whose result the program keeps.
    pub fn executes_here(&self, state: &SquadState, now: i64) -> bool {
        match self {
            Self::Vault(_) => true,
            Self::Config(change) => {
                change.not_executed_here().is_none()
                    && change_outcome(change.program, state, &change.actions, now).is_ok()
            }
            _ => false,
        }
    }
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
                    Some(StoredAccount::Config(change)) if change.index == *index => {
                        Stored::Config(change)
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
    /// Open a draft for votes. It approves nothing, so it needs no reading.
    Activate,
    /// A vote against it. [`available_action`] never gives it: whether a
    /// member may reject is [`available_rejection`], beside the action that
    /// moves the proposal forward.
    Reject,
    /// A vote to cancel an approved proposal. [`available_action`] never
    /// gives it: whether a member may cast it is [`available_cancel`],
    /// beside Execute.
    Cancel,
}

/// Why a member can do nothing to a proposal right now.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NoAction {
    #[error("The acting wallet is not a member of this multisig on this cluster.")]
    NotAMember,
    #[error("No proposal was opened for this transaction, so there is nothing to vote on.")]
    NoProposal,
    /// A draft the acting member may not open: only a member that may
    /// propose can.
    #[error("This proposal is a draft, not open for votes yet. Only a member that may propose can open it, and the acting member may not.")]
    Draft,
    #[error(
        "The multisig changed after this draft was made, so it can no longer be opened for votes."
    )]
    StaleDraft,
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
    /// A change to the multisig the program would refuse to execute, so a
    /// vote for it could never come to anything.
    #[error("The program refuses to execute this change, so it can never be executed and is not offered for approval or execution. {0}")]
    Refused(ChangeRefusal),
    /// An approved change to the multisig made stale since: both programs
    /// refuse to execute it.
    #[error("The multisig changed after this change to it was made, so it can never be executed: both programs refuse a stale change to the multisig, even an approved one.")]
    StaleChange,
    /// A change this app reads but does not execute.
    #[error("{0}")]
    NotExecutedHere(&'static str),
    #[error("The stored transaction is gone, so it cannot be executed.")]
    Gone,
    #[error("It was rejected for good. A rejected proposal cannot be reopened: to do what it holds, the transaction would have to be proposed again.")]
    Rejected,
    #[error("It was cancelled for good. A cancelled proposal cannot be executed or reopened: to do what it holds, the transaction would have to be proposed again.")]
    Cancelled,
    #[error("Nothing is left to do.")]
    Finished,
}

/// What `member` can do to move `entry` forward at time `now` (Unix
/// seconds): open a draft for votes, approve it, execute it, or both.
///
/// A transaction this app cannot read is never offered for approval: a vote
/// for something that was not read is not a vote. Opening a draft for votes
/// approves nothing, so it is offered whatever the draft holds. A member that
/// rejected may still approve while the proposal is open: the programs take
/// its rejection back.
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
        Stored::Vault(_) | Stored::Config(_) => Ok(()),
        Stored::Missing => Err(NoAction::Gone),
        Stored::LookupTables(_) | Stored::Other => Err(NoAction::Unreadable),
    };
    let change = match &entry.stored {
        Stored::Config(change) => Some(change),
        _ => None,
    };
    // A change the program would refuse can never be executed.
    let refused = |change: &ConfigTransaction| {
        change_outcome(change.program, state, &change.actions, now)
            .err()
            .map(NoAction::Refused)
    };
    let may_execute = permissions & PERMISSION_EXECUTE != 0;
    match proposal.status {
        // Both programs: a member with the initiate permission, whoever made
        // the draft, while it is not stale.
        ProposalStatus::Draft => {
            if entry.stale {
                Err(NoAction::StaleDraft)
            } else if permissions & PERMISSION_INITIATE == 0 {
                Err(NoAction::Draft)
            } else {
                Ok(ProposalAction::Activate)
            }
        }
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
            if let Some(refusal) = change.and_then(refused) {
                return Err(refusal);
            }
            let threshold = usize::from(state.threshold);
            if proposal.approved.contains(member) {
                return Err(NoAction::WaitingForOthers {
                    missing: threshold.saturating_sub(proposal.approved.len()),
                });
            }
            let completes = proposal.approved.len() + 1 >= threshold;
            let executes_here = change.is_none_or(|change| change.not_executed_here().is_none());
            Ok(
                if completes && state.time_lock == 0 && may_execute && executes_here {
                    ProposalAction::ApproveAndExecute
                } else {
                    ProposalAction::Approve
                },
            )
        }
        // Both programs execute an approved vault transaction that became
        // stale since: it was approved under the rules of its day. Not an
        // approved change to the multisig: that one has to be current.
        ProposalStatus::Approved => {
            readable?;
            if let Some(change) = change {
                if entry.stale {
                    return Err(NoAction::StaleChange);
                }
                if let Some(refusal) = refused(change) {
                    return Err(refusal);
                }
                if let Some(why) = change.not_executed_here() {
                    return Err(NoAction::NotExecutedHere(why));
                }
            }
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
        ProposalStatus::Cancelled => Err(NoAction::Cancelled),
        ProposalStatus::Executed => Err(NoAction::Finished),
    }
}

/// What opening draft `entry` for votes does, said before it is pressed, and
/// what else can be done to a draft.
pub fn activation_hint(entry: &ProposalEntry) -> String {
    let mut hint = "Opening it lets the members vote on it. It approves nothing.".to_string();
    match &entry.stored {
        Stored::Vault(_) | Stored::Config(_) => {}
        Stored::Missing => hint.push_str(
            " Its stored transaction is gone, so once open it can be rejected but never executed.",
        ),
        Stored::LookupTables(_) | Stored::Other => hint.push_str(
            " This app cannot read what it holds, so once open it can be rejected here but not approved.",
        ),
    }
    hint.push(' ');
    hint.push_str(DRAFT_CANNOT_BE_CANCELLED);
    hint
}

/// What a person looking for a way to cancel a draft is told: neither
/// program rejects or cancels a draft.
pub const DRAFT_CANNOT_BE_CANCELLED: &str =
    "A draft cannot be cancelled: left as it is, no one can vote on it or execute it.";

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

/// A vote to cancel an approved proposal that a member may cast, and what it
/// does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CancelVote {
    /// How many more members' cancel votes cancel the proposal once this one
    /// is in. Zero when this one does: the proposal is then cancelled for
    /// good.
    pub missing_after: usize,
}

impl CancelVote {
    /// This vote reaches the threshold, and the proposal is cancelled for
    /// good.
    pub fn last(&self) -> bool {
        self.missing_after == 0
    }

    /// What it does, said before it is cast.
    pub fn hint(&self) -> String {
        match self.missing_after {
            0 => "This vote to cancel is the last one needed: the proposal is cancelled for good and can never be executed.".to_string(),
            1 => "After this vote, 1 more member's vote to cancel cancels the proposal for good. Until then it stays approved.".to_string(),
            more => format!("After this vote, {more} more members' votes to cancel cancel the proposal for good. Until then it stays approved."),
        }
    }
}

/// Whether `member` may vote to cancel `entry`, and what its vote would do.
/// `None` when it may not.
///
/// Both programs take a cancel vote from a member with the vote permission
/// on an approved proposal, whether or not it is stale, whether or not its
/// time lock still holds it, and whether or not the member approved it:
/// they read only the proposal. A member that already voted to cancel cannot
/// vote again (`AlreadyCancelled`). The cancel votes of keys that left the
/// multisig are dropped before counting, as [`vote_counts`] counts them.
pub fn available_cancel(
    state: &SquadState,
    entry: &ProposalEntry,
    member: &Pubkey,
) -> Option<CancelVote> {
    let permissions = state
        .members
        .iter()
        .find(|known| known.key == *member)?
        .permissions;
    let proposal = entry.proposal.as_ref()?;
    if proposal.status != ProposalStatus::Approved
        || permissions & PERMISSION_VOTE == 0
        || proposal.cancelled.contains(member)
    {
        return None;
    }
    let cancelled = vote_counts(state, proposal).cancelled;
    Some(CancelVote {
        missing_after: cancelled.missing().saturating_sub(1),
    })
}

/// Everything `member` may do to `entry` at time `now`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberChoices {
    /// What moves it forward, or why nothing does: [`available_action`].
    pub forward: Result<ProposalAction, NoAction>,
    /// Its rejection, when it may cast one: [`available_rejection`].
    pub reject: Option<Rejection>,
    /// Its vote to cancel, when it may cast one: [`available_cancel`]. Never
    /// beside a rejection: one needs an open proposal, the other an
    /// approved one.
    pub cancel: Option<CancelVote>,
}

impl MemberChoices {
    /// The careful choice beside the one that moves the proposal forward:
    /// Reject on an open proposal, Cancel on an approved one, with what it
    /// does in words.
    pub fn turn_down(&self) -> Option<(ProposalAction, String)> {
        match (self.reject, self.cancel) {
            (Some(rejection), _) => Some((ProposalAction::Reject, rejection.hint())),
            (None, Some(cancel)) => Some((ProposalAction::Cancel, cancel.hint())),
            (None, None) => None,
        }
    }
}

/// What `member` may do to `entry` at time `now` (Unix seconds): the action
/// that moves it forward, and the rejection or the cancel vote, each when
/// the programs take it.
pub fn member_choices(
    state: &SquadState,
    entry: &ProposalEntry,
    member: &Pubkey,
    now: i64,
) -> MemberChoices {
    MemberChoices {
        forward: available_action(state, entry, member, now),
        reject: available_rejection(state, entry, member),
        cancel: available_cancel(state, entry, member),
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
/// the action needs the stored transaction and it could not be read.
/// Activating, rejecting and cancelling never do. Executing a change to the
/// multisig names `member` as the one that pays when an account grows:
/// [`action_instructions_paid`] names another.
pub fn action_instructions(
    squad: &Squad,
    entry: &ProposalEntry,
    member: &Pubkey,
    action: ProposalAction,
) -> Option<Vec<Instruction>> {
    action_instructions_paid(squad, entry, member, action, member)
}

/// The execution by `member` of what `entry` stores, with `rent_payer`
/// signing for what a change to the multisig costs (see
/// [`Squad::execute_change`]). `None` when it was not read, or is a change
/// this app does not execute.
pub fn execute_instruction(
    squad: &Squad,
    entry: &ProposalEntry,
    member: &Pubkey,
    rent_payer: &Pubkey,
) -> Option<Instruction> {
    match &entry.stored {
        Stored::Vault(tx) => Some(squad.execute_stored(tx, member)),
        Stored::Config(change) => squad.execute_change(change, member, rent_payer),
        _ => None,
    }
}

/// [`action_instructions`], with `rent_payer` paying for what executing a
/// change to the multisig costs. It signs too, so the fee payer is the one
/// to name: it signs anyway.
pub fn action_instructions_paid(
    squad: &Squad,
    entry: &ProposalEntry,
    member: &Pubkey,
    action: ProposalAction,
    rent_payer: &Pubkey,
) -> Option<Vec<Instruction>> {
    let approve = || squad.approve(entry.index, member);
    let execute = || execute_instruction(squad, entry, member, rent_payer);
    Some(match action {
        ProposalAction::Approve => vec![approve()],
        ProposalAction::Execute => vec![execute()?],
        ProposalAction::ApproveAndExecute => vec![approve(), execute()?],
        ProposalAction::Activate => vec![squad.activate(entry.index, member)],
        ProposalAction::Reject => vec![squad.reject(entry.index, member)],
        ProposalAction::Cancel => vec![squad.cancel(entry.index, member)],
    })
}
