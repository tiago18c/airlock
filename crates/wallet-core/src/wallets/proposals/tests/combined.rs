//! Which wallets a multisig's row names, and what they can finish together.

use super::*;
use crate::ids::WalletId;
use crate::keys::signer::KeypairSigner;
use crate::wallets::record::{HardwareVendor, WalletKind, WalletRecord};
use std::collections::BTreeSet;

const HOT: MemberKind = MemberKind::HotKey;
const OFFLINE: MemberKind = MemberKind::Offline;
const SIGNER: MemberKind = MemberKind::Programmatic { has_nonce: true };
const SIGNER_WITHOUT_NONCE: MemberKind = MemberKind::Programmatic { has_nonce: false };
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

/// A proposal of a multisig that needs `threshold` approvals and has those
/// of `approved`.
fn open(threshold: u16, approved: &[Pubkey]) -> (Squad, SquadState, ProposalEntry) {
    let squad = Squad::v4(&pk(1));
    let entry = entry(&squad, ProposalStatus::Active, approved);
    (squad, state(threshold, 0, &[]), entry)
}

fn offer(
    threshold: u16,
    approved: &[Pubkey],
    chosen: &[ChosenMember],
    nonce: Option<Pubkey>,
) -> Option<CombinedApproval> {
    let (_, state, entry) = open(threshold, approved);
    combined_approval(&state, &entry, chosen, nonce)
}

fn hot_wallet(label: &str) -> WalletRecord {
    WalletRecord::from_keypair_seed(label, KeypairSigner::new_random().seed_bytes())
}

fn hardware_wallet() -> WalletRecord {
    WalletRecord {
        id: WalletId::new(),
        label: "Ledger".into(),
        tags: BTreeSet::new(),
        kind: WalletKind::Hardware {
            vendor: HardwareVendor::Ledger,
            paths: Vec::new(),
        },
    }
}

/// A multisig wallet acted on by `acting`, with `others` approving together.
fn multisig(acting: Option<WalletId>, others: Vec<WalletId>) -> WalletRecord {
    let mut wallet = WalletRecord::squads("Team", Squad::v4(&pk(1)), Vec::new(), 2);
    wallet.kind.set_member_wallet(acting);
    wallet.kind.set_approves_with(others);
    wallet
}

#[test]
fn the_chosen_members_are_the_acting_one_and_then_the_rows_list_as_the_chain_knows_them() {
    let acting = hot_wallet("Acting");
    let cold = WalletRecord::offline("Cold", pk(20));
    // Its authority key is not a wallet here.
    let mut signer = WalletRecord::programmatic("Signer", pk(21), None);
    let stranger = hot_wallet("Stranger");
    let ledger = hardware_wallet();
    let team = multisig(
        Some(acting.id),
        vec![cold.id, ledger.id, signer.id, stranger.id],
    );
    let team_id = team.id;
    let on_chain = state(
        2,
        0,
        &[
            (acting.address().unwrap(), PERMISSION_ALL),
            (cold.address().unwrap(), PERMISSION_VOTE),
            (signer.address().unwrap(), PERMISSION_EXECUTE),
        ],
    );
    let expected = |wallet: &WalletRecord, permissions: u8, kind: MemberKind| ChosenMember {
        wallet: wallet.id,
        address: wallet.address().unwrap(),
        permissions,
        kind,
    };
    let want = vec![
        expected(&acting, PERMISSION_ALL, HOT),
        expected(&cold, PERMISSION_VOTE, OFFLINE),
        expected(&signer, PERMISSION_EXECUTE, SIGNER_WITHOUT_NONCE),
        // The chain does not list it, so it has no permissions.
        expected(&stranger, 0, HOT),
    ];
    let mut wallets = vec![
        stranger.clone(),
        signer.clone(),
        ledger.clone(),
        cold.clone(),
        acting.clone(),
        team,
    ];
    assert_eq!(chosen_members(&wallets, team_id, &on_chain), want);

    // A nonce account on the signer is seen.
    signer.kind.set_executor_nonce(Some(pk(30)));
    wallets[1] = signer.clone();
    assert_eq!(chosen_members(&wallets, team_id, &on_chain)[2].kind, SIGNER);

    // A Vector wallet takes part. Its key is wherever its record says.
    let vector = WalletRecord::vector(
        "Vector",
        crate::wallets::vector::SchemeId::Ed25519,
        pk(70),
        vec![7; 32],
        crate::wallets::record::Authority::Outside,
    );
    let team = multisig(Some(acting.id), vec![vector.id, cold.id]);
    let id = team.id;
    wallets.extend([vector.clone(), team]);
    let chosen: Vec<(WalletId, MemberKind)> = chosen_members(&wallets, id, &on_chain)
        .iter()
        .map(|member| (member.wallet, member.kind))
        .collect();
    assert_eq!(
        chosen,
        vec![(acting.id, HOT), (vector.id, VECTOR), (cold.id, OFFLINE)]
    );

    // Nobody is chosen without an acting member, or when it is a wallet that
    // cannot take part.
    for acting in [None, Some(ledger.id)] {
        let team = multisig(acting, vec![cold.id]);
        let id = team.id;
        wallets.push(team);
        assert!(chosen_members(&wallets, id, &on_chain).is_empty());
    }
    assert!(chosen_members(&wallets, acting.id, &on_chain).is_empty());
}

#[test]
fn two_members_of_a_two_of_two_approve_and_execute_with_the_acting_member_first() {
    for kind in [HOT, SIGNER, VECTOR] {
        let chosen = [member(10, kind), member(11, kind)];
        assert_eq!(
            offer(2, &[], &chosen, None),
            Some(CombinedApproval {
                voters: vec![pk(10), pk(11)],
                executor: pk(10),
                durable_nonce: None,
            })
        );
    }
    // A durable nonce on the row is not advanced when nobody needs it.
    let hot = [member(10, HOT), member(11, HOT)];
    assert_eq!(
        offer(2, &[], &hot, Some(pk(40))).unwrap().durable_nonce,
        None
    );
    // One programmatic member with a nonce orders the authorization of both.
    let one_nonce = [member(10, SIGNER_WITHOUT_NONCE), member(11, SIGNER)];
    assert!(offer(2, &[], &one_nonce, None).is_some());
    // A hot key and a programmatic signer share a transaction, and so do a
    // hot key and a Vector wallet.
    let mixed = [member(10, HOT), member(11, SIGNER)];
    assert_eq!(offer(2, &[], &mixed, None).unwrap().voters.len(), 2);
    let mixed = [member(10, VECTOR), member(11, HOT)];
    assert_eq!(offer(2, &[], &mixed, None).unwrap().voters.len(), 2);
}

#[test]
fn programmatic_and_vector_members_finish_a_proposal_together() {
    // Both protocols share one transaction: programmatic authorities sign
    // first, then Vector keys. The programmatic member executes, so its vote
    // comes last, next to its execute in the same `Submit`.
    let mixed = [member(10, SIGNER), member(11, VECTOR)];
    assert_eq!(
        offer(2, &[], &mixed, None),
        Some(CombinedApproval {
            voters: vec![pk(11), pk(10)],
            executor: pk(10),
            durable_nonce: None,
        })
    );
    let vector_executes = [
        with_permissions(10, SIGNER, PERMISSION_VOTE),
        member(11, VECTOR),
    ];
    assert_eq!(
        offer(2, &[], &vector_executes, None).unwrap().executor,
        pk(11)
    );
    // The Vector member executes, so the programmatic vote goes first: one
    // `Submit`, then one `Passthrough` for the Vector vote and execute.
    let three = [member(10, VECTOR), member(11, SIGNER), member(12, VECTOR)];
    assert_eq!(
        offer(2, &[], &three, None).unwrap().voters,
        vec![pk(11), pk(10)]
    );
}

#[test]
fn an_offline_member_votes_only_on_the_rows_durable_nonce() {
    let chosen = [member(10, OFFLINE), member(11, HOT)];
    assert_eq!(
        offer(2, &[], &chosen, Some(pk(40))),
        Some(CombinedApproval {
            voters: vec![pk(10), pk(11)],
            executor: pk(10),
            durable_nonce: Some(pk(40)),
        })
    );
    assert_eq!(offer(2, &[], &chosen, None), None);

    // With a third member chosen, the offline one is passed over and the
    // other two vote on a blockhash.
    let three = [member(10, OFFLINE), member(11, HOT), member(12, HOT)];
    assert_eq!(
        offer(2, &[], &three, None),
        Some(CombinedApproval {
            voters: vec![pk(11), pk(12)],
            executor: pk(11),
            durable_nonce: None,
        })
    );

    // An offline key and a member without a key cannot share a transaction.
    for keyless in [SIGNER, VECTOR] {
        let apart = [member(10, OFFLINE), member(11, keyless)];
        assert_eq!(offer(2, &[], &apart, Some(pk(40))), None, "{keyless:?}");
    }
    // Nor when the offline key would only execute.
    let executes = [
        with_permissions(10, SIGNER, PERMISSION_VOTE),
        with_permissions(11, SIGNER, PERMISSION_VOTE),
        member(12, OFFLINE),
    ];
    assert_eq!(offer(2, &[], &executes, Some(pk(40))), None);
}

#[test]
fn only_the_votes_still_needed_are_cast_in_the_rows_order() {
    // Two of three: the third chosen member would vote past the threshold.
    let chosen = [member(10, HOT), member(11, HOT), member(12, HOT)];
    assert_eq!(
        offer(2, &[], &chosen, None).unwrap().voters,
        vec![pk(10), pk(11)]
    );
    // Three of three with one approval in: the two others.
    assert_eq!(
        offer(3, &[pk(11)], &chosen, None).unwrap().voters,
        vec![pk(10), pk(12)]
    );
    // A member that may not vote is passed over.
    let silent = [
        member(10, HOT),
        with_permissions(11, HOT, PERMISSION_EXECUTE),
        member(12, HOT),
    ];
    assert_eq!(
        offer(2, &[], &silent, None).unwrap().voters,
        vec![pk(10), pk(12)]
    );
}

#[test]
fn a_member_that_does_not_vote_executes_when_no_voter_may() {
    let chosen = [
        with_permissions(10, HOT, PERMISSION_VOTE),
        with_permissions(11, HOT, PERMISSION_VOTE),
        with_permissions(12, HOT, PERMISSION_EXECUTE),
    ];
    let combined = offer(2, &[], &chosen, None).unwrap();
    assert_eq!(combined.voters, vec![pk(10), pk(11)]);
    assert_eq!(combined.executor, pk(12));
    assert_eq!(combined.signers(), vec![pk(10), pk(11), pk(12)]);

    // The first voter that may execute does, and signs once.
    let second = [
        with_permissions(10, HOT, PERMISSION_VOTE),
        member(11, HOT),
        member(12, HOT),
    ];
    let combined = offer(2, &[], &second, None).unwrap();
    assert_eq!(combined.executor, pk(11));
    assert_eq!(combined.signers(), vec![pk(10), pk(11)]);
}

#[test]
fn nothing_is_offered_when_the_chosen_members_cannot_finish_the_proposal() {
    let two = [member(10, HOT), member(11, HOT)];
    assert!(offer(2, &[], &two, None).is_some());

    // Nobody was chosen, or only the acting member.
    assert_eq!(offer(2, &[], &[], None), None);
    assert_eq!(offer(2, &[], &two[..1], None), None);
    // One vote is missing: approve and execute already does it.
    assert_eq!(offer(2, &[pk(50)], &two, None), None);
    assert_eq!(offer(1, &[], &two, None), None);
    // Too few of the chosen can still vote.
    assert_eq!(offer(3, &[], &two, None), None);
    assert_eq!(offer(2, &[pk(10)], &two, None), None);
    let one_voter = [
        member(10, HOT),
        with_permissions(11, HOT, PERMISSION_EXECUTE),
    ];
    assert_eq!(offer(2, &[], &one_voter, None), None);
    // The chain does not know one of them.
    let stranger = [member(10, HOT), with_permissions(11, HOT, 0)];
    assert_eq!(offer(2, &[], &stranger, None), None);
    // Nobody may execute.
    let voters = [
        with_permissions(10, HOT, PERMISSION_VOTE),
        with_permissions(11, HOT, PERMISSION_VOTE | PERMISSION_INITIATE),
    ];
    assert_eq!(offer(2, &[], &voters, None), None);
    // No programmatic signer in it has a nonce to order the authorization.
    let unordered = [
        member(10, SIGNER_WITHOUT_NONCE),
        member(11, SIGNER_WITHOUT_NONCE),
    ];
    assert_eq!(offer(2, &[], &unordered, None), None);
    let beside_a_key = [member(10, HOT), member(11, SIGNER_WITHOUT_NONCE)];
    assert_eq!(offer(2, &[], &beside_a_key, None), None);

    let squad = Squad::v4(&pk(1));
    let on_chain = state(2, 0, &[]);
    // A time lock splits approval from execution.
    let active = entry(&squad, ProposalStatus::Active, &[]);
    assert_eq!(
        combined_approval(&state(2, 60, &[]), &active, &two, None),
        None
    );
    // Not open for votes.
    for status in [
        ProposalStatus::Draft,
        ProposalStatus::Approved,
        ProposalStatus::Executed,
        ProposalStatus::Rejected,
        ProposalStatus::Cancelled,
    ] {
        let closed = entry(&squad, status, &[]);
        assert_eq!(
            combined_approval(&on_chain, &closed, &two, None),
            None,
            "{status:?}"
        );
    }
    let mut stale = active.clone();
    stale.stale = true;
    assert_eq!(combined_approval(&on_chain, &stale, &two, None), None);
    let mut unopened = active.clone();
    unopened.proposal = None;
    assert_eq!(combined_approval(&on_chain, &unopened, &two, None), None);
    // What was not read is not voted on.
    for stored in [Stored::LookupTables, Stored::Other, Stored::Missing] {
        let mut unreadable = active.clone();
        unreadable.stored = stored;
        assert_eq!(combined_approval(&on_chain, &unreadable, &two, None), None);
    }
}

#[test]
fn the_instructions_are_one_approval_for_each_voter_and_then_the_execution() {
    let (squad, _, entry) = open(2, &[]);
    let combined = CombinedApproval {
        voters: vec![pk(10), pk(11)],
        executor: pk(12),
        durable_nonce: None,
    };
    let Stored::Vault(stored) = &entry.stored else {
        panic!("a vault transaction");
    };
    let expected = vec![
        squad.approve(entry.index, &pk(10)),
        squad.approve(entry.index, &pk(11)),
        squad.execute_stored(stored, &pk(12)),
    ];
    assert_eq!(
        combined_instructions(&squad, &entry, &combined),
        Some(expected.clone())
    );
    assert_eq!(
        ProposalRequest::Combined(combined.clone()).instructions(&squad, &entry),
        Some(expected)
    );

    // One member acting alone is the action it always was.
    let single = ProposalRequest::Single {
        member: pk(10),
        action: ProposalAction::ApproveAndExecute,
    };
    assert_eq!(
        single.instructions(&squad, &entry),
        action_instructions(&squad, &entry, &pk(10), ProposalAction::ApproveAndExecute)
    );

    let mut gone = entry.clone();
    gone.stored = Stored::Missing;
    assert_eq!(combined_instructions(&squad, &gone, &combined), None);
}
