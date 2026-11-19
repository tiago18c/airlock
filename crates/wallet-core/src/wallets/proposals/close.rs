//! Closing a proposal's accounts, which returns their rent.
//!
//! Both programs close a stored transaction and its proposal together, and
//! take it from anyone who pays the fee: no member signs, so it works
//! whatever kind of wallet the acting member is, and it needs no reading.
//! What it costs is the record: what was proposed and who voted leave the
//! chain.
//!
//! When the programs allow it, from their source and proven against their
//! binaries:
//!
//! - executed, rejected and cancelled proposals, always;
//! - drafts and open proposals once stale (a change to the multisig made
//!   them so), since no one can open or vote on them any more;
//! - an approved vault transaction never, since it can still be executed,
//!   stale or not; an approved change to the multisig once stale, since it
//!   can then never be executed;
//! - a stored transaction with no proposal, once stale;
//! - on v4, only when the multisig has a rent collector, which gets the rent
//!   of both accounts. The smart account program returns each account's
//!   rent to whoever paid it, as the account records.
//!
//! A batch is closed one stored transaction at a time before the batch
//! itself, which this app does not do, and a proposal whose stored
//! transaction is gone cannot be closed: the programs close a proposal only
//! together with its transaction.

use super::*;
use crate::wallets::squads::{CloseKind, SquadsProgram};

/// How many closes go in one transaction. Each names two accounts of its own
/// and the rest are shared: twenty take under half of a transaction's bytes
/// and about a third of the compute one may use (the flow tests measure
/// both, on both programs).
pub const CLOSES_PER_TRANSACTION: usize = 20;

/// What a page read of one transaction number's two accounts that closing
/// them needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryAccounts {
    /// What the proposal account holds. Zero when there is none.
    pub proposal_lamports: u64,
    /// What the stored transaction account holds. Zero when there is none.
    pub transaction_lamports: u64,
    /// How the stored transaction is closed. `None` when it is gone, or is
    /// something this app does not close (a batch).
    pub close_kind: Option<CloseKind>,
    /// Smart account: who paid the proposal's rent.
    pub proposal_rent_collector: Option<Pubkey>,
    /// Smart account: who paid the stored transaction's rent.
    pub transaction_rent_collector: Option<Pubkey>,
}

/// A close the program takes: everything its instruction needs, and where
/// the rent goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closing {
    pub index: u64,
    pub kind: CloseKind,
    /// Where the proposal's rent goes. The same as `transaction_collector`
    /// on v4, and when there is no proposal.
    pub proposal_collector: Pubkey,
    pub transaction_collector: Pubkey,
    /// Lamports returned, by collector, each once.
    pub returns: Vec<(Pubkey, u64)>,
}

impl Closing {
    /// The rent returned in all.
    pub fn rent(&self) -> u64 {
        self.returns.iter().map(|(_, lamports)| lamports).sum()
    }

    pub fn instruction(&self, squad: &Squad) -> Instruction {
        squad.close(
            self.index,
            self.kind,
            &self.proposal_collector,
            &self.transaction_collector,
        )
    }
}

/// Why a transaction's accounts cannot be closed now.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum NoClose {
    /// v4, on a multisig that would otherwise allow it.
    #[error("This multisig has no rent collector, so the program does not let its transactions' accounts be closed. Setting one is a change to the multisig that its members can propose.")]
    NoRentCollector,
    #[error("An approved proposal cannot be closed while it can still be executed: it can be executed, or its members can vote to cancel it.")]
    Approved,
    #[error(
        "An open proposal can be closed only once a change to the multisig has made it stale."
    )]
    Open,
    #[error("A draft can be closed only once a change to the multisig has made it stale.")]
    Draft,
    #[error("With no proposal opened for it, a stored transaction can be closed only once a change to the multisig has made it stale.")]
    NoProposal,
    #[error("This proposal was left marked as being executed, and the program does not close such a proposal.")]
    Executing,
    #[error("Its stored transaction is gone, and the program closes a proposal only together with it, so the proposal's account stays.")]
    Gone,
    #[error("This app does not close a batch: the program closes its stored transactions one by one first.")]
    NotClosedHere,
    /// Nothing is left: its accounts were closed.
    #[error("Its accounts were closed and their rent returned.")]
    Closed,
}

impl NoClose {
    /// Worth a line on the row of `entry`: someone looking at it would
    /// wonder why it cannot be closed. An open proposal has Reject beside
    /// it, and a fresh approved one Execute, so neither says it.
    pub fn worth_saying(&self, entry: &ProposalEntry) -> bool {
        match self {
            Self::Open | Self::Closed => false,
            Self::Approved => entry.stale,
            _ => true,
        }
    }
}

/// Whether the accounts of `entry`, in the multisig of `program` whose state
/// is `state`, can be closed now, and where the rent would go; or why not.
/// No member is needed: whoever pays the fee may send it.
pub fn available_close(
    program: SquadsProgram,
    state: &SquadState,
    entry: &ProposalEntry,
) -> Result<Closing, NoClose> {
    if entry.stored == Stored::Missing {
        return Err(if entry.proposal.is_some() {
            NoClose::Gone
        } else {
            NoClose::Closed
        });
    }
    let kind = entry.accounts.close_kind.ok_or(NoClose::NotClosedHere)?;
    let stale = entry.stale;
    let stale_or = |reason| if stale { Ok(()) } else { Err(reason) };
    match entry.proposal.as_ref().map(|proposal| proposal.status) {
        None => stale_or(NoClose::NoProposal),
        Some(ProposalStatus::Draft) => stale_or(NoClose::Draft),
        Some(ProposalStatus::Active) => stale_or(NoClose::Open),
        // A stale vault transaction can still be executed; a stale change
        // to the multisig never can.
        Some(ProposalStatus::Approved) => match kind {
            CloseKind::Vault => Err(NoClose::Approved),
            CloseKind::Config => stale_or(NoClose::Approved),
        },
        Some(ProposalStatus::Executing) => Err(NoClose::Executing),
        Some(ProposalStatus::Rejected | ProposalStatus::Executed | ProposalStatus::Cancelled) => {
            Ok(())
        }
    }?;
    let has_proposal = entry.accounts.proposal_lamports > 0;
    let (proposal_collector, transaction_collector) = match program {
        SquadsProgram::V4 => {
            let collector = state.rent_collector.ok_or(NoClose::NoRentCollector)?;
            (collector, collector)
        }
        SquadsProgram::SmartAccount => {
            // Read with the account, so always there for a transaction
            // this app closes.
            let transaction = entry
                .accounts
                .transaction_rent_collector
                .ok_or(NoClose::NotClosedHere)?;
            // Unchecked when there is no proposal.
            let proposal = match (has_proposal, entry.accounts.proposal_rent_collector) {
                (true, Some(collector)) => collector,
                (true, None) => return Err(NoClose::NotClosedHere),
                (false, _) => transaction,
            };
            (proposal, transaction)
        }
    };
    let mut returns: Vec<(Pubkey, u64)> = Vec::new();
    for (collector, lamports) in [
        (proposal_collector, entry.accounts.proposal_lamports),
        (transaction_collector, entry.accounts.transaction_lamports),
    ] {
        if lamports == 0 {
            continue;
        }
        match returns.iter_mut().find(|(known, _)| *known == collector) {
            Some((_, sum)) => *sum += lamports,
            None => returns.push((collector, lamports)),
        }
    }
    Ok(Closing {
        index: entry.index,
        kind,
        proposal_collector,
        transaction_collector,
        returns,
    })
}

/// Every close of `entries` the program would take now, newest first.
pub fn closable(
    program: SquadsProgram,
    state: &SquadState,
    entries: &[ProposalEntry],
) -> Vec<Closing> {
    entries
        .iter()
        .filter_map(|entry| available_close(program, state, entry).ok())
        .collect()
}

/// The rent `closings` return, by collector, each once, in the order first
/// met.
pub fn rent_returned(closings: &[Closing]) -> Vec<(Pubkey, u64)> {
    let mut all: Vec<(Pubkey, u64)> = Vec::new();
    for (collector, lamports) in closings.iter().flat_map(|closing| closing.returns.iter()) {
        match all.iter_mut().find(|(known, _)| known == collector) {
            Some((_, sum)) => *sum += lamports,
            None => all.push((*collector, *lamports)),
        }
    }
    all
}
