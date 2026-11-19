use super::*;

fn with_votes(
    squad: &Squad,
    status: ProposalStatus,
    approved: &[Pubkey],
    rejected: &[Pubkey],
    cancelled: &[Pubkey],
) -> ProposalEntry {
    let mut entry = entry(squad, status, approved);
    let proposal = entry.proposal.as_mut().unwrap();
    proposal.rejected = rejected.to_vec();
    proposal.cancelled = cancelled.to_vec();
    entry
}

fn voter(key: Pubkey, member: bool) -> Voter {
    Voter { key, member }
}

#[test]
fn the_cutoff_counts_only_the_members_that_may_vote() {
    let all = |count: u8| -> Vec<(Pubkey, u8)> {
        (0..count).map(|at| (pk(10 + at), PERMISSION_ALL)).collect()
    };
    // The programs' own example: seven voters, threshold three, cutoff five.
    assert_eq!(rejection_cutoff(&state(3, 0, &all(7))), 5);
    assert_eq!(rejection_cutoff(&state(2, 0, &all(3))), 2);
    assert_eq!(rejection_cutoff(&state(3, 0, &all(3))), 1);
    assert_eq!(rejection_cutoff(&state(1, 0, &all(1))), 1);
    // A member that may only propose and execute does not vote, so it does
    // not move the cutoff.
    let mut mixed = all(3);
    mixed.push((pk(30), PERMISSION_INITIATE | PERMISSION_EXECUTE));
    let mixed = state(2, 0, &mixed);
    assert_eq!(voters(&mixed).count(), 3);
    assert_eq!(rejection_cutoff(&mixed), 2);
}

#[test]
fn the_counts_follow_the_program() {
    let squad = Squad::v4(&pk(1));
    let (a, b, c, gone) = (pk(2), pk(3), pk(4), pk(5));
    let team = state(
        2,
        0,
        &[
            (a, PERMISSION_ALL),
            (b, PERMISSION_ALL),
            (c, PERMISSION_ALL),
        ],
    );
    let open = with_votes(&squad, ProposalStatus::Active, &[a], &[b], &[]);
    let counts = vote_counts(&team, open.proposal.as_ref().unwrap());
    assert_eq!(
        counts.approved,
        Tally {
            count: 1,
            needed: 2
        }
    );
    assert_eq!(
        counts.rejected,
        Tally {
            count: 1,
            needed: 2
        }
    );
    assert_eq!(counts.rejected.missing(), 1);
    assert!(!counts.rejected.reached());
    assert_eq!(counts.cancelled.count, 0);

    // The program drops a former member's cancel vote before it counts.
    let approved = with_votes(&squad, ProposalStatus::Approved, &[a, b], &[], &[a, gone]);
    let counts = vote_counts(&team, approved.proposal.as_ref().unwrap());
    assert_eq!(
        counts.cancelled,
        Tally {
            count: 1,
            needed: 2
        }
    );
    assert!(counts.approved.reached());
}

#[test]
fn who_stands_where_on_an_open_proposal() {
    let squad = Squad::v4(&pk(1));
    let (a, b, c, d, gone) = (pk(2), pk(3), pk(4), pk(5), pk(6));
    let team = state(
        2,
        0,
        &[
            (a, PERMISSION_ALL),
            (b, PERMISSION_ALL),
            (c, PERMISSION_ALL),
            // May propose but not vote: never waited for.
            (d, PERMISSION_INITIATE),
        ],
    );
    // `gone` voted and has left since.
    let open = with_votes(&squad, ProposalStatus::Active, &[a, gone], &[b], &[]);
    let seen = standing_of(&team, &open);
    assert_eq!(seen.approved, vec![voter(a, true), voter(gone, false)]);
    assert_eq!(seen.rejected, vec![voter(b, true)]);
    assert!(seen.cancelled.is_empty());
    assert_eq!(seen.waiting, vec![c]);
    // The program counts every key in the list.
    assert_eq!(seen.counts.approved.count, 2);

    // Only an open proposal waits for anyone.
    let mut stale = open.clone();
    stale.stale = true;
    assert!(standing_of(&team, &stale).waiting.is_empty());
    for status in [
        ProposalStatus::Draft,
        ProposalStatus::Approved,
        ProposalStatus::Rejected,
        ProposalStatus::Executed,
        ProposalStatus::Cancelled,
    ] {
        let done = with_votes(&squad, status, &[a], &[b], &[]);
        let seen = standing_of(&team, &done);
        assert!(seen.waiting.is_empty(), "{status:?}");
        // The votes cast stay listed.
        assert_eq!(seen.approved, vec![voter(a, true)], "{status:?}");
    }

    let mut none = open;
    none.proposal = None;
    assert_eq!(standing(&team, &none), None);
}

fn standing_of(state: &SquadState, entry: &ProposalEntry) -> Standing {
    standing(state, entry).unwrap()
}

#[test]
fn a_cancel_vote_says_more_than_the_approval_under_it() {
    let squad = Squad::v4(&pk(1));
    let (a, b, c) = (pk(2), pk(3), pk(4));
    let approved = with_votes(&squad, ProposalStatus::Approved, &[a, b], &[], &[a]);
    let proposal = approved.proposal.as_ref().unwrap();
    assert_eq!(vote_of(proposal, &a), Some(Vote::Cancelled));
    assert_eq!(vote_of(proposal, &b), Some(Vote::Approved));
    assert_eq!(vote_of(proposal, &c), None);
    let open = with_votes(&squad, ProposalStatus::Active, &[a], &[b], &[]);
    assert_eq!(
        vote_of(open.proposal.as_ref().unwrap(), &b),
        Some(Vote::Rejected)
    );
}
