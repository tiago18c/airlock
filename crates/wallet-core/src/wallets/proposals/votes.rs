//! Who voted how on a proposal, and how far each vote has come.
//!
//! Both Squads programs keep three lists of keys on a proposal: who approved,
//! who rejected and who voted to cancel. They decide the same way:
//!
//! - It is approved once the approvals reach the threshold.
//! - It is rejected once the rejections reach the cutoff: the number of
//!   members that may vote, less the threshold, plus one. With that many
//!   against it, the threshold can no longer be reached.
//! - An approved proposal is cancelled once cancel votes reach the threshold.
//!   Before counting, the program drops the cancel votes of keys that are no
//!   longer members, so only members' cancel votes count.
//!
//! A member's approval and rejection exclude each other: approving takes
//! back a rejection and rejecting takes back an approval. A cancel vote sits
//! beside the approval it overturns.
//!
//! The lists keep a key that has since left the multisig. Leaving changes
//! the members, which makes every earlier proposal stale, so such a vote is
//! shown as cast, by someone who is no longer a member.

use crate::wallets::squads::{ProposalState, ProposalStatus, SquadState, PERMISSION_VOTE};
use solana_pubkey::Pubkey;

use super::ProposalEntry;

/// How many members cast one kind of vote, and how many it takes to decide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tally {
    pub count: usize,
    pub needed: usize,
}

impl Tally {
    /// How many more votes of this kind decide the proposal.
    pub fn missing(&self) -> usize {
        self.needed.saturating_sub(self.count)
    }

    /// The votes cast are enough.
    pub fn reached(&self) -> bool {
        self.count >= self.needed
    }
}

/// Where the three votes on a proposal stand, as the program counts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoteCounts {
    /// Approvals against the threshold.
    pub approved: Tally,
    /// Rejections against the cutoff, [`rejection_cutoff`].
    pub rejected: Tally,
    /// Cancel votes of members against the threshold.
    pub cancelled: Tally,
}

/// The members that may vote.
pub fn voters(state: &SquadState) -> impl Iterator<Item = &Pubkey> {
    state
        .members
        .iter()
        .filter(|member| member.permissions & PERMISSION_VOTE != 0)
        .map(|member| &member.key)
}

/// How many rejections reject a proposal: enough that the members who have
/// not rejected can no longer reach the threshold. Both programs compute it
/// as the voters less the threshold, plus one.
pub fn rejection_cutoff(state: &SquadState) -> usize {
    (voters(state).count() + 1).saturating_sub(usize::from(state.threshold))
}

fn is_member(state: &SquadState, key: &Pubkey) -> bool {
    state.members.iter().any(|member| member.key == *key)
}

/// Count the votes on `proposal` as the program would, with the multisig as
/// it is now.
pub fn vote_counts(state: &SquadState, proposal: &ProposalState) -> VoteCounts {
    let threshold = usize::from(state.threshold);
    VoteCounts {
        approved: Tally {
            count: proposal.approved.len(),
            needed: threshold,
        },
        rejected: Tally {
            count: proposal.rejected.len(),
            needed: rejection_cutoff(state),
        },
        cancelled: Tally {
            count: proposal
                .cancelled
                .iter()
                .filter(|key| is_member(state, key))
                .count(),
            needed: threshold,
        },
    }
}

/// A vote a member cast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vote {
    Approved,
    Rejected,
    /// A vote to cancel an approved proposal.
    Cancelled,
}

/// The vote of `key` that says most about where it stands: a cancel vote
/// overturns its approval, so it comes first.
pub fn vote_of(proposal: &ProposalState, key: &Pubkey) -> Option<Vote> {
    if proposal.cancelled.contains(key) {
        Some(Vote::Cancelled)
    } else if proposal.approved.contains(key) {
        Some(Vote::Approved)
    } else if proposal.rejected.contains(key) {
        Some(Vote::Rejected)
    } else {
        None
    }
}

/// A key in one of a proposal's vote lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Voter {
    pub key: Pubkey,
    /// Still a member of the multisig. A key that voted and left since is
    /// still listed: its vote was cast.
    pub member: bool,
}

/// Who stands where on one proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Standing {
    pub approved: Vec<Voter>,
    pub rejected: Vec<Voter>,
    pub cancelled: Vec<Voter>,
    /// The members that may vote and have neither approved nor rejected.
    /// Empty unless the proposal is open for votes and not stale.
    pub waiting: Vec<Pubkey>,
    pub counts: VoteCounts,
}

/// Who stands where on `entry`. `None` when no proposal was opened for it.
pub fn standing(state: &SquadState, entry: &ProposalEntry) -> Option<Standing> {
    let proposal = entry.proposal.as_ref()?;
    let listed = |keys: &[Pubkey]| {
        keys.iter()
            .map(|key| Voter {
                key: *key,
                member: is_member(state, key),
            })
            .collect()
    };
    let open = proposal.status == ProposalStatus::Active && !entry.stale;
    let waiting = if open {
        voters(state)
            .filter(|key| !proposal.approved.contains(key) && !proposal.rejected.contains(key))
            .copied()
            .collect()
    } else {
        Vec::new()
    };
    Some(Standing {
        approved: listed(&proposal.approved),
        rejected: listed(&proposal.rejected),
        cancelled: listed(&proposal.cancelled),
        waiting,
        counts: vote_counts(state, proposal),
    })
}
