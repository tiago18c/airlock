use super::*;

/// Three voters, 2 of 3: two rejections reject it.
fn three() -> (SquadState, [Pubkey; 3]) {
    let keys = [pk(2), pk(3), pk(4)];
    let members: Vec<(Pubkey, u8)> = keys.iter().map(|key| (*key, PERMISSION_ALL)).collect();
    (state(2, 0, &members), keys)
}

fn voted(
    squad: &Squad,
    status: ProposalStatus,
    approved: &[Pubkey],
    rejected: &[Pubkey],
) -> ProposalEntry {
    let mut entry = entry(squad, status, approved);
    entry.proposal.as_mut().unwrap().rejected = rejected.to_vec();
    entry
}

#[test]
fn a_member_in_neither_list_may_approve_or_reject() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, _, _]) = three();
    let open = voted(&squad, ProposalStatus::Active, &[], &[]);
    let choices = member_choices(&team, &open, &me, 0);
    assert_eq!(choices.forward, Ok(ProposalAction::Approve));
    assert_eq!(
        choices.reject,
        Some(Rejection {
            missing_after: 1,
            replaces_approval: false
        })
    );
    assert_eq!(
        choices.reject.unwrap().hint(),
        "After this rejection, 1 more rejects the proposal for good."
    );
    assert_eq!(
        action_instructions(&squad, &open, &me, ProposalAction::Reject),
        Some(vec![squad.reject(5, &me)])
    );
}

#[test]
fn the_rejection_that_reaches_the_cutoff_says_it_rejects_for_good() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, other, _]) = three();
    let one_against = voted(&squad, ProposalStatus::Active, &[], &[other]);
    let reject = available_rejection(&team, &one_against, &me).unwrap();
    assert!(reject.last());
    assert_eq!(
        reject.hint(),
        "This rejection is the last one needed: the proposal is rejected for good, and its transaction would have to be proposed again."
    );
    // Approving it is still on offer beside.
    assert_eq!(
        available_action(&team, &one_against, &me, 0),
        Ok(ProposalAction::Approve)
    );

    // Seven voters, threshold three: the cutoff is five.
    let seven: Vec<(Pubkey, u8)> = (0..7).map(|at| (pk(10 + at), PERMISSION_ALL)).collect();
    let big = state(3, 0, &seven);
    let two_against = voted(&squad, ProposalStatus::Active, &[], &[pk(11), pk(12)]);
    let reject = available_rejection(&big, &two_against, &pk(10)).unwrap();
    assert_eq!(reject.missing_after, 2);
    assert_eq!(
        reject.hint(),
        "After this rejection, 2 more reject the proposal for good."
    );
}

#[test]
fn a_member_that_approved_may_change_to_a_rejection() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, other, _]) = three();
    let mine = voted(&squad, ProposalStatus::Active, &[me], &[]);
    let choices = member_choices(&team, &mine, &me, 0);
    assert_eq!(
        choices.forward,
        Err(NoAction::WaitingForOthers { missing: 1 })
    );
    let reject = choices.reject.unwrap();
    assert!(reject.replaces_approval);
    assert_eq!(
        reject.hint(),
        "Rejecting takes back the acting member's approval. After this rejection, 1 more rejects the proposal for good."
    );
    // With another rejection in, this one rejects it.
    let against = voted(&squad, ProposalStatus::Active, &[me], &[other]);
    let reject = available_rejection(&team, &against, &me).unwrap();
    assert!(reject.replaces_approval && reject.last());
}

#[test]
fn a_member_that_rejected_may_change_to_an_approval_but_not_reject_again() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, other, _]) = three();
    let mine = voted(&squad, ProposalStatus::Active, &[], &[me]);
    let choices = member_choices(&team, &mine, &me, 0);
    assert_eq!(choices.forward, Ok(ProposalAction::Approve));
    assert_eq!(choices.reject, None);
    // Its approval completes it when another member approved.
    let other_in = voted(&squad, ProposalStatus::Active, &[other], &[me]);
    assert_eq!(
        available_action(&team, &other_in, &me, 0),
        Ok(ProposalAction::ApproveAndExecute)
    );
}

#[test]
fn rejecting_needs_no_reading() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, _, _]) = three();
    for stored in [
        Stored::Other,
        Stored::LookupTables(TableProblem::Missing { table: pk(40) }),
        Stored::LookupTables(TableProblem::Short {
            table: pk(40),
            index: 9,
            holds: 2,
        }),
        Stored::Missing,
    ] {
        let mut open = voted(&squad, ProposalStatus::Active, &[], &[]);
        open.stored = stored.clone();
        let choices = member_choices(&team, &open, &me, 0);
        assert!(choices.forward.is_err(), "{stored:?}");
        assert!(choices.reject.is_some(), "{stored:?}");
        assert_eq!(
            action_instructions(&squad, &open, &me, ProposalAction::Reject),
            Some(vec![squad.reject(5, &me)]),
            "{stored:?}"
        );
    }
}

#[test]
fn only_an_open_proposal_takes_a_rejection() {
    let squad = Squad::smart_account(7);
    let (team, [me, other, _]) = three();
    for status in [
        ProposalStatus::Draft,
        ProposalStatus::Approved,
        ProposalStatus::Executing,
        ProposalStatus::Executed,
        ProposalStatus::Rejected,
        ProposalStatus::Cancelled,
    ] {
        let entry = voted(&squad, status, &[other], &[]);
        assert_eq!(available_rejection(&team, &entry, &me), None, "{status:?}");
    }
    let mut stale = voted(&squad, ProposalStatus::Active, &[], &[]);
    stale.stale = true;
    assert_eq!(available_rejection(&team, &stale, &me), None);
    let mut none = voted(&squad, ProposalStatus::Active, &[], &[]);
    none.proposal = None;
    assert_eq!(available_rejection(&team, &none, &me), None);
}

#[test]
fn a_rejected_proposal_says_it_is_over_for_good() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, other, third]) = three();
    let rejected = voted(&squad, ProposalStatus::Rejected, &[], &[other, third]);
    let choices = member_choices(&team, &rejected, &me, 0);
    assert_eq!(choices.forward, Err(NoAction::Rejected));
    assert_eq!(choices.reject, None);
    assert_eq!(
        NoAction::Rejected.to_string(),
        "It was rejected for good. A rejected proposal cannot be reopened: to do what it holds, the transaction would have to be proposed again."
    );
}

#[test]
fn a_member_that_may_not_vote_or_is_no_member_may_not_reject() {
    let squad = Squad::v4(&pk(1));
    let me = pk(2);
    let team = state(
        1,
        0,
        &[
            (me, PERMISSION_INITIATE | PERMISSION_EXECUTE),
            (pk(3), PERMISSION_ALL),
        ],
    );
    let mut open = voted(&squad, ProposalStatus::Active, &[], &[]);
    assert_eq!(available_rejection(&team, &open, &me), None);
    assert_eq!(available_rejection(&team, &open, &pk(77)), None);
    // Not voting is the reason given, even for what cannot be read.
    open.stored = Stored::Other;
    assert_eq!(
        available_action(&team, &open, &me, 0),
        Err(NoAction::MayNotVote)
    );
    assert_eq!(
        available_action(&team, &open, &pk(77), 0),
        Err(NoAction::NotAMember)
    );
}

#[test]
fn a_rejection_runs_nothing_so_it_asks_for_no_heap() {
    let request = ProposalRequest::Single {
        member: pk(2),
        action: ProposalAction::Reject,
    };
    assert!(!request.executes());
    for action in [ProposalAction::Execute, ProposalAction::ApproveAndExecute] {
        assert!(ProposalRequest::Single {
            member: pk(2),
            action
        }
        .executes());
    }
    assert!(!ProposalRequest::Single {
        member: pk(2),
        action: ProposalAction::Approve
    }
    .executes());
}
