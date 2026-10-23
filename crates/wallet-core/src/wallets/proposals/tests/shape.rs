//! Shapes a new proposal can take, from the fullest down to create only.

use super::*;
use crate::ids::WalletId;
use crate::wallets::proposals::{
    keyless_together, proposal_shapes, ChosenMember, MemberKind, ProposalShape,
};

const HOT: MemberKind = MemberKind::HotKey;
const OFFLINE: MemberKind = MemberKind::Offline;
const SIGNER: MemberKind = MemberKind::Programmatic { has_nonce: true };
const VECTOR: MemberKind = MemberKind::Vector;

fn member(n: u8, kind: MemberKind) -> ChosenMember {
    with_permissions(n, kind, PERMISSION_ALL)
}

fn with_permissions(n: u8, kind: MemberKind, permissions: u8) -> ChosenMember {
    ChosenMember {
        wallet: WalletId::new(),
        address: pk(n),
        permissions,
        kind,
    }
}

fn shapes(
    threshold: u16,
    time_lock: u32,
    approved: &[Pubkey],
    chosen: &[ChosenMember],
) -> Vec<ProposalShape> {
    proposal_shapes(&state(threshold, time_lock, &[]), approved, chosen)
}

fn shape(voters: &[u8], executor: Option<u8>) -> ProposalShape {
    ProposalShape {
        voters: voters.iter().map(|n| pk(*n)).collect(),
        executor: executor.map(pk),
    }
}

#[test]
fn thresholds_one_to_three_list_execute_then_fewer_votes() {
    let one = [member(10, HOT)];
    assert_eq!(
        shapes(1, 0, &[], &one),
        vec![shape(&[10], Some(10)), shape(&[10], None), shape(&[], None)]
    );

    let two = [member(10, HOT), member(11, HOT)];
    assert_eq!(
        shapes(2, 0, &[], &two),
        vec![
            shape(&[10, 11], Some(10)),
            shape(&[10, 11], None),
            shape(&[10], None),
            shape(&[], None),
        ]
    );

    let three = [member(10, HOT), member(11, HOT), member(12, HOT)];
    assert_eq!(
        shapes(3, 0, &[], &three),
        vec![
            shape(&[10, 11, 12], Some(10)),
            shape(&[10, 11, 12], None),
            shape(&[10, 11], None),
            shape(&[10], None),
            shape(&[], None),
        ]
    );
}

#[test]
fn a_time_lock_or_missing_execute_permission_drops_execute_first() {
    let one = [member(10, HOT)];
    assert_eq!(
        shapes(1, 60, &[], &one),
        vec![shape(&[10], None), shape(&[], None)]
    );

    let voter = [with_permissions(
        10,
        HOT,
        PERMISSION_INITIATE | PERMISSION_VOTE,
    )];
    assert_eq!(
        shapes(1, 0, &[], &voter),
        vec![shape(&[10], None), shape(&[], None)]
    );
}

#[test]
fn an_acting_member_without_vote_only_creates() {
    let proposer = [with_permissions(10, HOT, PERMISSION_INITIATE)];
    assert_eq!(shapes(1, 0, &[], &proposer), vec![shape(&[], None)]);
}

#[test]
fn a_non_voting_executor_runs_when_no_voter_may() {
    let chosen = [
        with_permissions(10, HOT, PERMISSION_VOTE),
        with_permissions(11, HOT, PERMISSION_EXECUTE),
    ];
    assert_eq!(shapes(1, 0, &[], &chosen)[0], shape(&[10], Some(11)));
}

#[test]
fn an_offline_co_voter_is_skipped_for_a_site_rebuild() {
    let chosen = [member(10, HOT), member(11, OFFLINE)];
    assert_eq!(
        shapes(2, 0, &[], &chosen),
        vec![shape(&[10], None), shape(&[], None)]
    );
}

#[test]
fn programmatic_and_vector_members_vote_together() {
    let apart = [member(10, SIGNER), member(11, VECTOR)];
    assert_eq!(
        shapes(2, 0, &[], &apart),
        vec![
            shape(&[10, 11], Some(10)),
            shape(&[10, 11], None),
            shape(&[10], None),
            shape(&[], None),
        ]
    );
    let vector_first = [member(10, VECTOR), member(11, SIGNER)];
    assert_eq!(
        shapes(2, 0, &[], &vector_first),
        vec![
            shape(&[10, 11], Some(10)),
            shape(&[10, 11], None),
            shape(&[10], None),
            shape(&[], None),
        ]
    );
}

#[test]
fn one_approval_already_cast_is_not_repeated() {
    let two = [member(10, HOT), member(11, HOT)];
    assert_eq!(
        shapes(2, 0, &[pk(10)], &two),
        vec![shape(&[11], Some(11)), shape(&[11], None), shape(&[], None),]
    );
}

#[test]
fn duplicate_shapes_are_removed() {
    // Threshold 1 with two voters: the second vote is never taken, so the
    // one-vote shapes are not repeated after trimming.
    let two = [member(10, HOT), member(11, HOT)];
    let listed = shapes(1, 0, &[], &two);
    assert_eq!(
        listed,
        vec![shape(&[10], Some(10)), shape(&[10], None), shape(&[], None)]
    );
}

/// Programmatic members share one `Submit`, so their instructions have to
/// sit together: create and propose, the approvals, then execute.
#[test]
fn programmatic_instructions_are_kept_together() {
    let (p, q, h) = (pk(1), pk(2), pk(3));
    let advances = |address: &Pubkey| (*address == p || *address == q).then_some(false);
    let order = |creator, voters: &[Pubkey], executor| {
        keyless_together(creator, voters, executor, advances, false)
    };

    // The row's order stays when it already works.
    assert_eq!(order(None, &[h, p], Some(h)), Some(vec![h, p]));
    // Together, no create: the programmatic votes go next to its execute.
    assert_eq!(order(None, &[p, h], Some(p)), Some(vec![h, p]));
    assert_eq!(order(None, &[p, h, q], Some(q)), Some(vec![h, p, q]));
    // A programmatic creator: its votes follow create and propose.
    assert_eq!(order(Some(p), &[h, p], Some(h)), Some(vec![p, h]));
    assert_eq!(order(Some(p), &[h, p], None), Some(vec![p, h]));
    // Another member creates and a programmatic member executes.
    assert_eq!(order(Some(h), &[p, h], Some(p)), Some(vec![h, p]));
    // A programmatic creator and executor with another vote: no order.
    assert_eq!(order(Some(p), &[p, h], Some(p)), None);
    // No programmatic member: the order is kept.
    assert_eq!(order(Some(h), &[pk(4), h], Some(h)), Some(vec![pk(4), h]));
}

/// Vector members share one `Passthrough` when their instructions sit
/// together, and the programmatic `Submit` must not split them.
#[test]
fn vector_instructions_are_kept_together_when_they_can_be() {
    let (p, q, v, w, h) = (pk(1), pk(2), pk(3), pk(4), pk(5));
    let advances = |address: &Pubkey| match *address {
        a if a == p || a == q => Some(false),
        a if a == v || a == w => Some(true),
        _ => None,
    };
    let order = |creator, voters: &[Pubkey], executor, vector_too| {
        keyless_together(creator, voters, executor, advances, vector_too)
    };

    // A programmatic creator, a programmatic and a Vector co-voter, the
    // Vector one executing: one Submit, then one Passthrough.
    assert_eq!(
        order(Some(p), &[p, v, q], Some(v), true),
        Some(vec![p, q, v])
    );
    // Two Vector voters split by a programmatic one come together.
    assert_eq!(
        order(None, &[v, p, w, h], Some(h), true),
        Some(vec![p, v, w, h])
    );
    // A Vector creator and executor around a programmatic vote: the
    // programmatic vote still fits, the Vector instructions cannot meet.
    assert_eq!(order(Some(v), &[v, p], Some(v), true), None);
    assert_eq!(order(Some(v), &[v, p], Some(v), false), Some(vec![v, p]));
}
