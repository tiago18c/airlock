//! Vetting a change to the multisig before it is proposed, and how far one
//! transaction takes it.

use super::*;
use crate::ids::WalletId;
use crate::wallets::squads::{ConfigAction, SquadsProgram, PERMISSION_EXECUTE, PERMISSION_VOTE};

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

/// Three members that may do everything, 2 of 3.
fn team() -> SquadState {
    state(
        2,
        0,
        &[
            (pk(2), PERMISSION_ALL),
            (pk(3), PERMISSION_ALL),
            (pk(4), PERMISSION_ALL),
        ],
    )
}

fn add(n: u8, permissions: u8) -> ConfigAction {
    ConfigAction::AddMember(SquadsMember {
        key: pk(n),
        permissions,
    })
}

fn vet(
    program: SquadsProgram,
    state: &SquadState,
    actions: &[ConfigAction],
) -> Result<(u16, usize), CannotPropose> {
    vet_change(program, state, actions, 0).map(|outcome| (outcome.threshold, outcome.members.len()))
}

fn chosen(n: u8, kind: MemberKind, permissions: u8) -> ChosenMember {
    ChosenMember {
        wallet: WalletId::new(),
        address: pk(n),
        permissions,
        kind,
    }
}

#[test]
fn adding_a_member_and_raising_the_threshold_go_in_one_change_in_that_order() {
    let draft = ChangeDraft {
        add: Some(SquadsMember {
            key: pk(9),
            permissions: PERMISSION_ALL,
        }),
        remove: None,
        threshold: Some(3),
        rent_collector: None,
    };
    let actions = draft.actions(&team());
    assert_eq!(
        actions,
        vec![add(9, PERMISSION_ALL), ConfigAction::ChangeThreshold(3)]
    );
    for program in BOTH {
        assert_eq!(vet(program, &team(), &actions), Ok((3, 4)), "{program:?}");
    }
}

#[test]
fn a_field_left_as_it_is_asks_for_nothing() {
    let mut collected = team();
    collected.rent_collector = Some(pk(7));
    let draft = ChangeDraft {
        add: None,
        remove: None,
        threshold: Some(2),
        rent_collector: Some(Some(pk(7))),
    };
    assert!(draft.actions(&collected).is_empty());
    let clear = ChangeDraft {
        rent_collector: Some(None),
        ..ChangeDraft::default()
    };
    assert_eq!(
        clear.actions(&collected),
        vec![ConfigAction::SetRentCollector(None)]
    );
    assert!(clear.actions(&team()).is_empty());
}

#[test]
fn what_the_program_would_refuse_is_not_proposable() {
    for program in BOTH {
        let refused = |actions: &[ConfigAction]| match vet(program, &team(), actions) {
            Err(CannotPropose::Refused(refusal)) => refusal,
            other => panic!("{program:?}: {other:?}"),
        };
        // A threshold above the voters left.
        assert_eq!(
            refused(&[ConfigAction::ChangeThreshold(4)]),
            ChangeRefusal::ThresholdOverVoters {
                threshold: 4,
                voters: 3
            }
        );
        assert_eq!(
            refused(&[
                ConfigAction::RemoveMember(pk(4)),
                ConfigAction::ChangeThreshold(3)
            ]),
            ChangeRefusal::ThresholdOverVoters {
                threshold: 3,
                voters: 2
            }
        );
        // A new member that may not vote does not count.
        assert_eq!(
            refused(&[add(9, PERMISSION_EXECUTE), ConfigAction::ChangeThreshold(4)]),
            ChangeRefusal::ThresholdOverVoters {
                threshold: 4,
                voters: 3
            }
        );
        assert_eq!(
            refused(&[ConfigAction::ChangeThreshold(0)]),
            ChangeRefusal::ZeroThreshold
        );
        // Removing one that is not there, adding one that is, or twice.
        assert_eq!(
            refused(&[ConfigAction::RemoveMember(pk(9))]),
            ChangeRefusal::NotAMember(pk(9))
        );
        assert_eq!(
            refused(&[add(3, PERMISSION_VOTE)]),
            ChangeRefusal::AlreadyAMember(pk(3))
        );
        assert_eq!(
            refused(&[add(9, PERMISSION_VOTE), add(9, PERMISSION_ALL)]),
            ChangeRefusal::AlreadyAMember(pk(9))
        );
    }
    // Nobody left who may propose, or execute.
    let lone = state(1, 0, &[(pk(2), PERMISSION_ALL), (pk(3), PERMISSION_VOTE)]);
    assert_eq!(
        vet(
            SquadsProgram::V4,
            &lone,
            &[ConfigAction::RemoveMember(pk(2))]
        ),
        Err(CannotPropose::Refused(ChangeRefusal::NoProposers))
    );
    let executor = state(
        1,
        0,
        &[
            (pk(2), PERMISSION_INITIATE | PERMISSION_VOTE),
            (pk(3), PERMISSION_EXECUTE),
        ],
    );
    assert_eq!(
        vet(
            SquadsProgram::SmartAccount,
            &executor,
            &[ConfigAction::RemoveMember(pk(3))]
        ),
        Err(CannotPropose::Refused(ChangeRefusal::NoExecutors))
    );
}

#[test]
fn an_empty_change_or_one_that_changes_nothing_is_not_proposable() {
    for program in BOTH {
        assert_eq!(vet(program, &team(), &[]), Err(CannotPropose::NoActions));
        assert_eq!(
            vet(program, &team(), &[ConfigAction::ChangeThreshold(2)]),
            Err(CannotPropose::ChangesNothing)
        );
        // Out and back in as it was.
        assert_eq!(
            vet(
                program,
                &team(),
                &[ConfigAction::RemoveMember(pk(3)), add(3, PERMISSION_ALL)]
            ),
            Err(CannotPropose::ChangesNothing)
        );
        // Back in with less is a change.
        assert_eq!(
            vet(
                program,
                &team(),
                &[ConfigAction::RemoveMember(pk(3)), add(3, PERMISSION_VOTE)]
            ),
            Ok((2, 3))
        );
    }
    let mut collected = team();
    collected.rent_collector = Some(pk(7));
    assert_eq!(
        vet(
            SquadsProgram::V4,
            &collected,
            &[ConfigAction::SetRentCollector(Some(pk(7)))]
        ),
        Err(CannotPropose::ChangesNothing)
    );
}

#[test]
fn a_controlled_multisig_takes_no_change_by_proposal() {
    let mut controlled = team();
    controlled.config_authority = Some(pk(8));
    for program in BOTH {
        assert_eq!(
            vet(program, &controlled, &[ConfigAction::ChangeThreshold(3)]),
            Err(CannotPropose::Controlled)
        );
    }
    assert_eq!(
        change_proposal(
            &controlled,
            &[chosen(2, MemberKind::HotKey, PERMISSION_ALL)],
            None
        ),
        Err(CannotPropose::Controlled)
    );
}

#[test]
fn the_smart_account_program_has_no_rent_collector() {
    let set = [ConfigAction::SetRentCollector(Some(pk(7)))];
    assert_eq!(
        vet(SquadsProgram::SmartAccount, &team(), &set),
        Err(CannotPropose::NotOnThisProgram)
    );
    assert_eq!(vet(SquadsProgram::V4, &team(), &set), Ok((2, 3)));
    assert_eq!(
        vet(
            SquadsProgram::V4,
            &team(),
            &[ConfigAction::SetArchivalAuthority(Some(pk(7)))]
        ),
        Err(CannotPropose::NotOnThisProgram)
    );
}

#[test]
fn the_acting_member_has_to_be_a_member_that_may_propose() {
    let s = team();
    assert_eq!(change_proposal(&s, &[], None), Err(CannotPropose::NoMember));
    assert_eq!(
        change_proposal(&s, &[chosen(9, MemberKind::HotKey, 0)], None),
        Err(CannotPropose::NotAMember(pk(9)))
    );
    assert_eq!(
        change_proposal(
            &s,
            &[chosen(
                2,
                MemberKind::HotKey,
                PERMISSION_VOTE | PERMISSION_EXECUTE
            )],
            None
        ),
        Err(CannotPropose::MayNotPropose(pk(2)))
    );
    assert_eq!(
        change_proposal(
            &s,
            &[chosen(
                2,
                MemberKind::Programmatic { has_nonce: false },
                PERMISSION_ALL
            )],
            None
        ),
        Err(CannotPropose::NoNonce(pk(2)))
    );
}

#[test]
fn the_members_here_approve_and_execute_as_far_as_they_reach() {
    let s = team();
    let hot = |n| chosen(n, MemberKind::HotKey, PERMISSION_ALL);
    // One vote of two: stored, opened and approved.
    let alone = change_proposal(&s, &[hot(2)], None).unwrap();
    assert_eq!(
        (
            alone.index,
            alone.creator,
            alone.voters.clone(),
            alone.executor
        ),
        (8, pk(2), vec![pk(2)], None)
    );
    // Two together reach it: executed too.
    let two = change_proposal(&s, &[hot(2), hot(3)], None).unwrap();
    assert_eq!(
        (two.voters.clone(), two.executor),
        (vec![pk(2), pk(3)], Some(pk(2)))
    );
    assert_eq!(two.signers(), vec![pk(2), pk(3)]);
    // A time lock holds execution.
    let mut locked = s.clone();
    locked.time_lock = 60;
    let held = change_proposal(&locked, &[hot(2), hot(3)], None).unwrap();
    assert_eq!(held.executor, None);
    // A creator that may only propose stores and opens it, and votes not.
    let proposer = change_proposal(
        &s,
        &[chosen(2, MemberKind::HotKey, PERMISSION_INITIATE)],
        None,
    )
    .unwrap();
    assert_eq!((proposer.voters, proposer.executor), (Vec::new(), None));
}

#[test]
fn offline_and_keyless_wallets_vote_only_where_they_can() {
    let s = state(1, 0, &[(pk(2), PERMISSION_ALL), (pk(3), PERMISSION_ALL)]);
    let one = state(2, 0, &[(pk(2), PERMISSION_ALL), (pk(3), PERMISSION_ALL)]);
    // An offline acting member does it all alone, on another device.
    let offline =
        change_proposal(&s, &[chosen(2, MemberKind::Offline, PERMISSION_ALL)], None).unwrap();
    assert_eq!((offline.executor, offline.offline), (Some(pk(2)), true));
    // An offline wallet beside it votes only on the row's durable nonce.
    let beside = [
        chosen(2, MemberKind::HotKey, PERMISSION_ALL),
        chosen(3, MemberKind::Offline, PERMISSION_ALL),
    ];
    let without = change_proposal(&one, &beside, None).unwrap();
    assert_eq!(
        (without.voters.clone(), without.offline),
        (vec![pk(2)], false)
    );
    let with = change_proposal(&one, &beside, Some(pk(8))).unwrap();
    assert_eq!(
        (with.voters.clone(), with.executor, with.offline),
        (vec![pk(2), pk(3)], Some(pk(2)), true)
    );
    // Never beside a member without a key.
    let mixed = [
        chosen(2, MemberKind::Vector, PERMISSION_ALL),
        chosen(3, MemberKind::Offline, PERMISSION_ALL),
    ];
    let vector = change_proposal(&one, &mixed, Some(pk(8))).unwrap();
    assert_eq!(
        (vector.voters.clone(), vector.offline),
        (vec![pk(2)], false)
    );
    // A programmatic signer without its own nonce does not vote beside.
    let signer = [
        chosen(2, MemberKind::HotKey, PERMISSION_ALL),
        chosen(
            3,
            MemberKind::Programmatic { has_nonce: false },
            PERMISSION_ALL,
        ),
    ];
    assert_eq!(
        change_proposal(&one, &signer, None).unwrap().voters,
        vec![pk(2)]
    );
    // A programmatic creator executes with its own instructions together.
    let programmatic = change_proposal(
        &s,
        &[chosen(
            2,
            MemberKind::Programmatic { has_nonce: true },
            PERMISSION_ALL,
        )],
        None,
    )
    .unwrap();
    assert_eq!(
        (programmatic.voters, programmatic.executor),
        (vec![pk(2)], Some(pk(2)))
    );
}

#[test]
fn the_instructions_store_open_approve_and_execute_with_the_payer_paying() {
    for squad in [Squad::v4(&pk(1)), Squad::smart_account(7)] {
        let actions = [add(9, PERMISSION_ALL), ConfigAction::ChangeThreshold(3)];
        let proposal = ChangeProposal {
            index: 8,
            creator: pk(2),
            voters: vec![pk(2), pk(3)],
            executor: Some(pk(2)),
            offline: false,
        };
        let change = ConfigTransaction {
            program: squad.program,
            creator: pk(2),
            index: 8,
            actions: actions.to_vec(),
        };
        assert_eq!(
            proposal.instructions(&squad, &actions, &pk(5)).unwrap(),
            vec![
                squad.create_change(8, &actions, &pk(2), &pk(5)).unwrap(),
                squad.create_proposal(8, &pk(2), &pk(5)),
                squad.approve(8, &pk(2)),
                squad.approve(8, &pk(3)),
                squad.execute_change(&change, &pk(2), &pk(5)).unwrap(),
            ]
        );
    }
    // A rent collector cannot be stored on the smart account program.
    let proposal = ChangeProposal {
        index: 1,
        creator: pk(2),
        voters: Vec::new(),
        executor: None,
        offline: false,
    };
    assert_eq!(
        proposal.instructions(
            &Squad::smart_account(7),
            &[ConfigAction::SetRentCollector(None)],
            &pk(5)
        ),
        None
    );
}

#[test]
fn a_refusal_is_one_short_sentence() {
    let name = |key: &Pubkey| {
        if *key == pk(3) {
            "Bob".to_string()
        } else {
            key.to_string()
        }
    };
    let said = |reason: CannotPropose| reason.said().text(&name);
    assert_eq!(
        said(CannotPropose::Refused(ChangeRefusal::ThresholdOverVoters {
            threshold: 4,
            voters: 3
        })),
        "It would need 4 approvals with only 3 members that may vote."
    );
    assert_eq!(
        said(CannotPropose::Refused(ChangeRefusal::AlreadyAMember(pk(3)))),
        "Bob is a member already."
    );
    assert_eq!(
        said(CannotPropose::MayNotPropose(pk(3))),
        "Bob may not propose."
    );
}
