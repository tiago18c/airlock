use super::*;

/// Three voters, 2 of 3, and a proposer that may only propose.
fn team() -> (SquadState, [Pubkey; 3], Pubkey) {
    let keys = [pk(2), pk(3), pk(4)];
    let proposer = pk(5);
    let mut members: Vec<(Pubkey, u8)> = keys.iter().map(|key| (*key, PERMISSION_ALL)).collect();
    members.push((proposer, PERMISSION_INITIATE));
    (state(2, 0, &members), keys, proposer)
}

fn cancelled_by(squad: &Squad, approved: &[Pubkey], cancelled: &[Pubkey]) -> ProposalEntry {
    let mut entry = entry(squad, ProposalStatus::Approved, approved);
    entry.proposal.as_mut().unwrap().cancelled = cancelled.to_vec();
    entry
}

#[test]
fn any_member_that_may_propose_opens_a_draft_whoever_made_it() {
    for squad in [Squad::v4(&pk(1)), Squad::smart_account(7)] {
        let (team, [me, _, _], proposer) = team();
        let draft = entry(&squad, ProposalStatus::Draft, &[]);
        for member in [me, proposer] {
            let choices = member_choices(&team, &draft, &member, 0);
            assert_eq!(choices.forward, Ok(ProposalAction::Activate));
            // Neither program rejects or cancels a draft.
            assert_eq!(choices.reject, None);
            assert_eq!(choices.cancel, None);
            assert_eq!(choices.turn_down(), None);
        }
        assert_eq!(
            action_instructions(&squad, &draft, &me, ProposalAction::Activate),
            Some(vec![squad.activate(5, &me)])
        );
        assert!(!ProposalRequest::Single {
            member: me,
            action: ProposalAction::Activate
        }
        .executes());
    }
}

#[test]
fn a_member_that_may_not_propose_is_told_who_can_open_the_draft() {
    let squad = Squad::v4(&pk(1));
    let me = pk(2);
    let team = state(
        1,
        0,
        &[
            (me, PERMISSION_VOTE | PERMISSION_EXECUTE),
            (pk(3), PERMISSION_ALL),
        ],
    );
    let draft = entry(&squad, ProposalStatus::Draft, &[]);
    assert_eq!(
        available_action(&team, &draft, &me, 0),
        Err(NoAction::Draft)
    );
    assert_eq!(
        NoAction::Draft.to_string(),
        "This proposal is a draft, not open for votes yet. Only a member that may propose can open it, and the acting member may not."
    );
    assert_eq!(
        available_action(&team, &draft, &pk(77), 0),
        Err(NoAction::NotAMember)
    );
}

#[test]
fn a_stale_draft_can_no_longer_be_opened() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, _, _], _) = team();
    let mut draft = entry(&squad, ProposalStatus::Draft, &[]);
    draft.stale = true;
    let choices = member_choices(&team, &draft, &me, 0);
    assert_eq!(choices.forward, Err(NoAction::StaleDraft));
    assert_eq!(choices.turn_down(), None);
    assert_eq!(
        NoAction::StaleDraft.to_string(),
        "The multisig changed after this draft was made, so it can no longer be opened for votes."
    );
}

#[test]
fn opening_a_draft_needs_no_reading_and_says_what_it_does() {
    let squad = Squad::smart_account(7);
    let (team, [me, _, _], _) = team();
    let mut draft = entry(&squad, ProposalStatus::Draft, &[]);
    assert_eq!(
        activation_hint(&draft),
        "Opening it lets the members vote on it. It approves nothing. A draft cannot be cancelled: left as it is, no one can vote on it or execute it."
    );
    draft.stored = Stored::Other;
    assert_eq!(
        available_action(&team, &draft, &me, 0),
        Ok(ProposalAction::Activate)
    );
    assert_eq!(
        action_instructions(&squad, &draft, &me, ProposalAction::Activate),
        Some(vec![squad.activate(5, &me)])
    );
    assert!(activation_hint(&draft)
        .contains("This app cannot read what it holds, so once open it can be rejected here but not approved."));
    draft.stored = Stored::Missing;
    assert_eq!(
        available_action(&team, &draft, &me, 0),
        Ok(ProposalAction::Activate)
    );
    assert!(activation_hint(&draft).contains("Its stored transaction is gone"));
}

#[test]
fn a_member_may_vote_to_cancel_an_approved_proposal_beside_execute() {
    for squad in [Squad::v4(&pk(1)), Squad::smart_account(7)] {
        let (team, [me, other, _], _) = team();
        // Approved by the member and another: it may still vote to cancel.
        let approved = cancelled_by(&squad, &[me, other], &[]);
        let choices = member_choices(&team, &approved, &me, 2_000);
        assert_eq!(choices.forward, Ok(ProposalAction::Execute));
        assert_eq!(choices.reject, None);
        let cancel = choices.cancel.unwrap();
        assert_eq!(cancel, CancelVote { missing_after: 1 });
        assert_eq!(
            choices.turn_down(),
            Some((ProposalAction::Cancel, cancel.hint()))
        );
        assert_eq!(
            cancel.hint(),
            "After this vote, 1 more member's vote to cancel cancels the proposal for good. Until then it stays approved."
        );
        assert_eq!(
            action_instructions(&squad, &approved, &me, ProposalAction::Cancel),
            Some(vec![squad.cancel(5, &me)])
        );
        assert!(!ProposalRequest::Single {
            member: me,
            action: ProposalAction::Cancel
        }
        .executes());
    }
}

#[test]
fn the_cancel_vote_that_reaches_the_threshold_says_it_cancels_for_good() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, other, third], _) = team();
    let one_in = cancelled_by(&squad, &[other, third], &[other]);
    let cancel = available_cancel(&team, &one_in, &me).unwrap();
    assert!(cancel.last());
    assert_eq!(
        cancel.hint(),
        "This vote to cancel is the last one needed: the proposal is cancelled for good and can never be executed."
    );
    // The one that voted to cancel is not offered it again; it may still
    // execute, until the threshold is reached.
    let choices = member_choices(&team, &one_in, &other, 2_000);
    assert_eq!(choices.cancel, None);
    assert_eq!(choices.forward, Ok(ProposalAction::Execute));

    // Five voters, threshold four.
    let five: Vec<(Pubkey, u8)> = (0..5).map(|at| (pk(10 + at), PERMISSION_ALL)).collect();
    let big = state(4, 0, &five);
    let one = cancelled_by(&squad, &[pk(10), pk(11), pk(12), pk(13)], &[pk(11)]);
    let cancel = available_cancel(&big, &one, &pk(10)).unwrap();
    assert_eq!(cancel.missing_after, 2);
    assert_eq!(
        cancel.hint(),
        "After this vote, 2 more members' votes to cancel cancel the proposal for good. Until then it stays approved."
    );
}

#[test]
fn a_former_members_cancel_vote_does_not_count() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, other, _], _) = team();
    // pk(9) voted to cancel and left since: the programs drop its vote.
    let gone = cancelled_by(&squad, &[me, other], &[pk(9)]);
    assert_eq!(
        available_cancel(&team, &gone, &me),
        Some(CancelVote { missing_after: 1 })
    );
}

#[test]
fn cancelling_is_offered_while_the_time_lock_holds_execute() {
    let squad = Squad::smart_account(7);
    let (me, other) = (pk(2), pk(3));
    let locked = state(2, 600, &[(me, PERMISSION_ALL), (other, PERMISSION_ALL)]);
    let approved = cancelled_by(&squad, &[me, other], &[]);
    let choices = member_choices(&locked, &approved, &me, 1_030);
    assert_eq!(
        choices.forward,
        Err(NoAction::TimeLock {
            until: 1_600,
            now: 1_030
        })
    );
    assert_eq!(choices.cancel, Some(CancelVote { missing_after: 1 }));
}

#[test]
fn a_stale_or_unreadable_approved_proposal_can_still_be_cancelled() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, other, _], _) = team();
    let mut approved = cancelled_by(&squad, &[me, other], &[]);
    approved.stale = true;
    assert!(available_cancel(&team, &approved, &me).is_some());
    approved.stored = Stored::Other;
    let choices = member_choices(&team, &approved, &me, 2_000);
    assert_eq!(choices.forward, Err(NoAction::Unreadable));
    assert!(choices.cancel.is_some());
    approved.stored = Stored::Missing;
    assert!(available_cancel(&team, &approved, &me).is_some());
}

#[test]
fn only_a_member_that_may_vote_cancels_and_only_an_approved_proposal() {
    let squad = Squad::smart_account(7);
    let (team, [me, other, third], proposer) = team();
    let approved = cancelled_by(&squad, &[other, third], &[]);
    assert_eq!(available_cancel(&team, &approved, &proposer), None);
    assert_eq!(available_cancel(&team, &approved, &pk(77)), None);
    // Execute only, no vote.
    let executor = state(
        2,
        0,
        &[
            (me, PERMISSION_EXECUTE),
            (other, PERMISSION_ALL),
            (third, PERMISSION_ALL),
        ],
    );
    assert_eq!(available_cancel(&executor, &approved, &me), None);
    for status in [
        ProposalStatus::Draft,
        ProposalStatus::Active,
        ProposalStatus::Executing,
        ProposalStatus::Executed,
        ProposalStatus::Rejected,
        ProposalStatus::Cancelled,
    ] {
        let mut entry = cancelled_by(&squad, &[other], &[]);
        entry.proposal.as_mut().unwrap().status = status;
        assert_eq!(available_cancel(&team, &entry, &me), None, "{status:?}");
    }
    let mut none = cancelled_by(&squad, &[], &[]);
    none.proposal = None;
    assert_eq!(available_cancel(&team, &none, &me), None);
}

#[test]
fn a_cancelled_proposal_says_it_is_over_for_good() {
    let squad = Squad::v4(&pk(1));
    let (team, [me, other, third], _) = team();
    let mut cancelled = cancelled_by(&squad, &[other, third], &[other, third]);
    cancelled.proposal.as_mut().unwrap().status = ProposalStatus::Cancelled;
    let choices = member_choices(&team, &cancelled, &me, 2_000);
    assert_eq!(choices.forward, Err(NoAction::Cancelled));
    assert_eq!(choices.turn_down(), None);
    assert_eq!(
        NoAction::Cancelled.to_string(),
        "It was cancelled for good. A cancelled proposal cannot be executed or reopened: to do what it holds, the transaction would have to be proposed again."
    );
}

#[test]
fn the_builders_name_the_accounts_each_program_takes() {
    let me = pk(2);
    let v4 = Squad::v4(&pk(1));
    let activate = v4.activate(5, &me);
    assert_eq!(
        activate
            .accounts
            .iter()
            .map(|meta| (meta.pubkey, meta.is_signer, meta.is_writable))
            .collect::<Vec<_>>(),
        vec![
            (v4.address, false, false),
            (me, true, true),
            (v4.proposal(5), false, true)
        ]
    );
    let cancel = v4.cancel(5, &me);
    assert_eq!(cancel.accounts.len(), 4);
    assert_eq!(cancel.accounts[3].pubkey, crate::ids::SYSTEM_PROGRAM_ID);

    let smart = Squad::smart_account(7);
    let activate = smart.activate(5, &me);
    assert_eq!(
        activate
            .accounts
            .iter()
            .map(|meta| meta.pubkey)
            .collect::<Vec<_>>(),
        vec![smart.address, me, smart.proposal(5)]
    );
    let cancel = smart.cancel(5, &me);
    assert_eq!(
        cancel
            .accounts
            .iter()
            .map(|meta| meta.pubkey)
            .collect::<Vec<_>>(),
        vec![
            smart.address,
            me,
            smart.proposal(5),
            crate::ids::SYSTEM_PROGRAM_ID,
            smart.program.program_id()
        ]
    );
}
