//! A change to the multisig itself, as a member sees it in a proposal.

use super::*;
use crate::wallets::squads::{ConfigAction, ConfigTransaction, PolicyVerb, SquadsProgram};

fn change(squad: &Squad, actions: Vec<ConfigAction>) -> Stored {
    Stored::Config(ConfigTransaction {
        program: squad.program,
        creator: pk(2),
        index: 5,
        actions,
    })
}

fn raise() -> Vec<ConfigAction> {
    vec![
        ConfigAction::AddMember(SquadsMember {
            key: pk(9),
            permissions: PERMISSION_ALL,
        }),
        ConfigAction::ChangeThreshold(3),
    ]
}

#[test]
fn a_change_that_was_read_is_approved_and_executed_like_a_vault_transaction() {
    for squad in [Squad::v4(&pk(1)), Squad::smart_account(7)] {
        let (me, other) = (pk(2), pk(3));
        let two = state(2, 0, &[(me, PERMISSION_ALL), (other, PERMISSION_ALL)]);
        let mut open = entry(&squad, ProposalStatus::Active, &[]);
        open.stored = change(&squad, raise());
        assert_eq!(
            available_action(&two, &open, &me, 0),
            Ok(ProposalAction::Approve)
        );
        open.proposal.as_mut().unwrap().approved = vec![other];
        assert_eq!(
            available_action(&two, &open, &me, 0),
            Ok(ProposalAction::ApproveAndExecute)
        );
        let instructions = action_instructions_paid(
            &squad,
            &open,
            &me,
            ProposalAction::ApproveAndExecute,
            &pk(8),
        )
        .unwrap();
        let Stored::Config(stored) = &open.stored else {
            unreachable!()
        };
        assert_eq!(
            instructions,
            vec![
                squad.approve(5, &me),
                squad.execute_change(stored, &me, &pk(8)).unwrap()
            ]
        );
        // Without a payer named, the member pays.
        assert_eq!(
            action_instructions(&squad, &open, &me, ProposalAction::Execute).unwrap(),
            vec![squad.execute_change(stored, &me, &me).unwrap()]
        );

        let mut approved = entry(&squad, ProposalStatus::Approved, &[me, other]);
        approved.stored = change(&squad, raise());
        assert_eq!(
            available_action(&two, &approved, &me, 2_000),
            Ok(ProposalAction::Execute)
        );
    }
}

#[test]
fn an_approved_change_that_went_stale_can_never_be_executed() {
    let squad = Squad::v4(&pk(1));
    let (me, other) = (pk(2), pk(3));
    let two = state(2, 0, &[(me, PERMISSION_ALL), (other, PERMISSION_ALL)]);
    let mut approved = entry(&squad, ProposalStatus::Approved, &[me, other]);
    approved.stored = change(&squad, raise());
    approved.stale = true;
    let verdict = available_action(&two, &approved, &me, 2_000);
    assert_eq!(verdict, Err(NoAction::StaleChange));
    assert!(verdict
        .unwrap_err()
        .to_string()
        .contains("can never be executed"));
    // A vault transaction approved before it went stale still runs.
    let mut vault = entry(&squad, ProposalStatus::Approved, &[me, other]);
    vault.stale = true;
    assert_eq!(
        available_action(&two, &vault, &me, 2_000),
        Ok(ProposalAction::Execute)
    );
    // It can still be voted down: cancelling needs no current multisig.
    assert!(available_cancel(&two, &approved, &me).is_some());
    // Nobody else executes it in its place either.
    assert_eq!(
        executor_instead(
            &two,
            &approved,
            &[ChosenMember {
                wallet: crate::ids::WalletId::new(),
                address: me,
                permissions: PERMISSION_ALL,
                kind: MemberKind::HotKey,
            }],
            2_000
        ),
        None
    );
}

#[test]
fn a_change_the_program_would_refuse_is_never_offered_but_can_be_rejected() {
    let squad = Squad::smart_account(7);
    let (me, other) = (pk(2), pk(3));
    let two = state(2, 0, &[(me, PERMISSION_ALL), (other, PERMISSION_ALL)]);
    let mut open = entry(&squad, ProposalStatus::Active, &[]);
    open.stored = change(&squad, vec![ConfigAction::RemoveMember(pk(9))]);
    let verdict = available_action(&two, &open, &me, 0);
    assert_eq!(
        verdict,
        Err(NoAction::Refused(
            crate::wallets::squads::ChangeRefusal::NotAMember(pk(9))
        ))
    );
    assert!(available_rejection(&two, &open, &me).is_some());
    // Approved before the refusal was known, it is not offered for execution.
    let mut approved = entry(&squad, ProposalStatus::Approved, &[me, other]);
    approved.stored = change(&squad, vec![ConfigAction::SetArchivalAuthority(None)]);
    assert!(matches!(
        available_action(&two, &approved, &me, 2_000),
        Err(NoAction::Refused(_))
    ));
    assert!(!approved.stored.executes_here(&two, 2_000));
}

#[test]
fn a_policy_change_is_approved_but_not_executed_here() {
    let squad = Squad::smart_account(7);
    let (me, other) = (pk(2), pk(3));
    let two = state(2, 0, &[(me, PERMISSION_ALL), (other, PERMISSION_ALL)]);
    let policy = vec![ConfigAction::Policy {
        verb: PolicyVerb::Remove,
        policy: Some(pk(20)),
        fields: Vec::new(),
    }];
    // The last approval does not execute: this app cannot.
    let mut open = entry(&squad, ProposalStatus::Active, &[other]);
    open.stored = change(&squad, policy.clone());
    assert_eq!(
        available_action(&two, &open, &me, 0),
        Ok(ProposalAction::Approve)
    );
    let mut approved = entry(&squad, ProposalStatus::Approved, &[me, other]);
    approved.stored = change(&squad, policy);
    assert!(matches!(
        available_action(&two, &approved, &me, 2_000),
        Err(NoAction::NotExecutedHere(_))
    ));
    assert_eq!(
        action_instructions(&squad, &approved, &me, ProposalAction::Execute),
        None
    );
}

#[test]
fn a_page_reads_a_change_account_as_a_change() {
    let squad = Squad::v4(&pk(1));
    let two = state(2, 0, &[(pk(2), PERMISSION_ALL), (pk(3), PERMISSION_ALL)]);
    let store = squad
        .create_change(8, &raise(), &pk(2), &pk(2))
        .expect("stored");
    // The account as the program writes it: its tag, multisig, creator,
    // number, bump, then the actions as the instruction carried them.
    let mut data = hex::decode("5e080423718b8b70").unwrap();
    data.extend(squad.address.to_bytes());
    data.extend(pk(2).to_bytes());
    data.extend(8u64.to_le_bytes());
    data.push(254);
    data.extend(&store.data[8..store.data.len() - 1]);
    let accounts = vec![
        None,
        Some(RawAccount {
            lamports: 1,
            owner: SquadsProgram::V4.program_id(),
            data,
            executable: false,
            rent_epoch: 0,
        }),
    ];
    let page = read_page(&squad, &two, &[8], &accounts, &HashMap::new());
    let Stored::Config(read) = &page[0].stored else {
        panic!("{:?}", page[0].stored);
    };
    assert_eq!(read.actions, raise());
    assert!(page[0].stored.is_read());
}
