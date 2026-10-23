use super::*;
use crate::fixtures::{pk, transfer};
use crate::wallets::squads::{SquadsMember, PERMISSION_ALL, PERMISSION_INITIATE};

mod combined;
mod shape;

fn state(threshold: u16, time_lock: u32, members: &[(Pubkey, u8)]) -> SquadState {
    SquadState {
        threshold,
        time_lock,
        transaction_index: 7,
        stale_transaction_index: 2,
        members: members
            .iter()
            .map(|(key, permissions)| SquadsMember {
                key: *key,
                permissions: *permissions,
            })
            .collect(),
    }
}

fn entry(squad: &Squad, status: ProposalStatus, approved: &[Pubkey]) -> ProposalEntry {
    let vault = squad.vault(0);
    ProposalEntry {
        index: 5,
        proposal: Some(ProposalState {
            index: 5,
            status,
            since: 1_000,
            approved: approved.to_vec(),
            rejected: Vec::new(),
            cancelled: Vec::new(),
        }),
        stored: Stored::Vault(VaultTransaction::for_test(
            squad,
            5,
            0,
            &[transfer(vault, pk(9), 5)],
        )),
        stale: false,
    }
}

#[test]
fn a_page_counts_down_from_the_newest_and_stops_at_one() {
    assert_eq!(page_indexes(12), vec![12, 11, 10, 9, 8, 7, 6, 5, 4, 3]);
    assert_eq!(page_indexes(3), vec![3, 2, 1]);
    assert!(page_indexes(0).is_empty());
    let squad = Squad::v4(&pk(1));
    assert_eq!(
        page_accounts(&squad, &[2, 1]),
        vec![
            squad.proposal(2),
            squad.transaction(2),
            squad.proposal(1),
            squad.transaction(1)
        ]
    );
}

#[test]
fn an_approval_that_is_not_the_last_is_only_an_approval() {
    let squad = Squad::v4(&pk(1));
    let (me, other) = (pk(2), pk(3));
    let two = state(2, 0, &[(me, PERMISSION_ALL), (other, PERMISSION_ALL)]);
    let open = entry(&squad, ProposalStatus::Active, &[]);
    assert_eq!(
        available_action(&two, &open, &me, 0),
        Ok(ProposalAction::Approve)
    );
    // With the other member's approval in, this one completes it.
    let one_in = entry(&squad, ProposalStatus::Active, &[other]);
    assert_eq!(
        available_action(&two, &one_in, &me, 0),
        Ok(ProposalAction::ApproveAndExecute)
    );
    let instructions =
        action_instructions(&squad, &one_in, &me, ProposalAction::ApproveAndExecute).unwrap();
    assert_eq!(
        instructions,
        vec![
            squad.approve(5, &me),
            squad.execute(5, 0, &[transfer(squad.vault(0), pk(9), 5)], &me)
        ]
    );
    // Having approved, the member waits.
    let mine = entry(&squad, ProposalStatus::Active, &[me]);
    assert_eq!(
        available_action(&two, &mine, &me, 0),
        Err(NoAction::WaitingForOthers { missing: 1 })
    );
}

#[test]
fn a_time_lock_or_a_missing_permission_splits_approval_from_execution() {
    let squad = Squad::v4(&pk(1));
    let me = pk(2);
    let one_in = entry(&squad, ProposalStatus::Active, &[pk(3)]);
    let locked = state(2, 60, &[(me, PERMISSION_ALL), (pk(3), PERMISSION_ALL)]);
    assert_eq!(
        available_action(&locked, &one_in, &me, 0),
        Ok(ProposalAction::Approve)
    );
    let approved = entry(&squad, ProposalStatus::Approved, &[me, pk(3)]);
    assert_eq!(
        available_action(&locked, &approved, &me, 1_030),
        Err(NoAction::TimeLock {
            until: 1_060,
            now: 1_030
        })
    );
    assert_eq!(
        available_action(&locked, &approved, &me, 1_060),
        Ok(ProposalAction::Execute)
    );

    let voter = state(2, 0, &[(me, PERMISSION_VOTE), (pk(3), PERMISSION_ALL)]);
    assert_eq!(
        available_action(&voter, &one_in, &me, 0),
        Ok(ProposalAction::Approve)
    );
    assert_eq!(
        available_action(&voter, &approved, &me, 2_000),
        Err(NoAction::MayNotExecute)
    );
    let proposer = state(1, 0, &[(me, PERMISSION_INITIATE)]);
    assert_eq!(
        available_action(&proposer, &one_in, &me, 0),
        Err(NoAction::MayNotVote)
    );
    assert_eq!(
        available_action(&proposer, &one_in, &pk(77), 0),
        Err(NoAction::NotAMember)
    );
}

#[test]
fn what_cannot_be_read_or_is_over_offers_nothing() {
    let squad = Squad::v4(&pk(1));
    let me = pk(2);
    let alone = state(1, 0, &[(me, PERMISSION_ALL)]);
    let mut unread = entry(&squad, ProposalStatus::Active, &[]);
    unread.stored = Stored::Other;
    assert_eq!(
        available_action(&alone, &unread, &me, 0),
        Err(NoAction::Unreadable)
    );
    unread.stored = Stored::LookupTables;
    assert_eq!(
        available_action(&alone, &unread, &me, 0),
        Err(NoAction::Unreadable)
    );
    assert_eq!(
        action_instructions(&squad, &unread, &me, ProposalAction::Execute),
        None
    );

    let mut stale = entry(&squad, ProposalStatus::Active, &[]);
    stale.stale = true;
    assert_eq!(
        available_action(&alone, &stale, &me, 0),
        Err(NoAction::Stale)
    );
    // An approval given before the multisig changed still counts.
    stale.proposal.as_mut().unwrap().status = ProposalStatus::Approved;
    assert_eq!(
        available_action(&alone, &stale, &me, 2_000),
        Ok(ProposalAction::Execute)
    );

    for status in [
        ProposalStatus::Executed,
        ProposalStatus::Rejected,
        ProposalStatus::Cancelled,
    ] {
        assert_eq!(
            available_action(&alone, &entry(&squad, status, &[me]), &me, 0),
            Err(NoAction::Finished)
        );
    }
    let mut none = entry(&squad, ProposalStatus::Active, &[]);
    none.proposal = None;
    assert_eq!(
        available_action(&alone, &none, &me, 0),
        Err(NoAction::NoProposal)
    );
}
