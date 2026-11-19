//! Changes to a multisig: read from the account the programs write, built
//! back, worded, and what they leave.

use super::*;
use crate::fixtures::pk;

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

/// An account's discriminator, from the IDL the app ships.
fn account_tag(program: SquadsProgram, name: &str) -> Vec<u8> {
    let text = match program {
        SquadsProgram::V4 => include_str!("../../../idls/squads-multisig.json"),
        SquadsProgram::SmartAccount => include_str!("../../../idls/squads-smart-account.json"),
    };
    let idl: serde_json::Value = serde_json::from_str(text).unwrap();
    let account = idl["program"]["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["name"] == name)
        .unwrap()
        .clone();
    hex::decode(
        account["data"]["fields"][0]["defaultValue"]["data"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}

/// The account a program writes for change `index` of `squad` holding the
/// actions encoded as `actions` (Borsh, with their count first), as both
/// programs lay it out.
pub(crate) fn change_account(
    squad: &Squad,
    index: u64,
    creator: Pubkey,
    actions: &[u8],
) -> Vec<u8> {
    let mut data = Vec::new();
    match squad.program {
        SquadsProgram::V4 => {
            data.extend(account_tag(squad.program, "configTransaction"));
            data.extend(squad.address.to_bytes());
            data.extend(creator.to_bytes());
        }
        SquadsProgram::SmartAccount => {
            data.extend(account_tag(squad.program, "settingsTransaction"));
            data.extend(squad.address.to_bytes());
            data.extend(creator.to_bytes());
            // The rent collector: whoever paid for it.
            data.extend(creator.to_bytes());
        }
    }
    data.extend(index.to_le_bytes());
    data.push(255);
    data.extend(actions);
    data
}

/// The actions as `create_change` encodes them: its arguments are the
/// actions, then an empty memo.
pub(crate) fn encoded(squad: &Squad, actions: &[ConfigAction]) -> Vec<u8> {
    let store = squad.create_change(1, actions, &pk(2), &pk(3)).unwrap();
    let data = &store.data[8..];
    assert_eq!(data.last(), Some(&0), "no memo");
    data[..data.len() - 1].to_vec()
}

fn every_action(program: SquadsProgram) -> Vec<ConfigAction> {
    let mut actions = vec![
        ConfigAction::AddMember(SquadsMember {
            key: pk(10),
            permissions: PERMISSION_VOTE,
        }),
        ConfigAction::RemoveMember(pk(11)),
        ConfigAction::ChangeThreshold(3),
        ConfigAction::SetTimeLock(86_400),
        ConfigAction::AddSpendingLimit(SpendingLimit {
            seed: pk(12),
            vault_index: 1,
            mint: Pubkey::default(),
            amount: 2_500_000_000,
            period: Period::Week,
            members: vec![pk(10), pk(13)],
            destinations: vec![pk(14)],
            expiration: match program {
                SquadsProgram::V4 => None,
                SquadsProgram::SmartAccount => Some(1_900_000_000),
            },
        }),
        ConfigAction::RemoveSpendingLimit(pk(15)),
    ];
    match program {
        SquadsProgram::V4 => {
            actions.push(ConfigAction::SetRentCollector(Some(pk(16))));
            actions.push(ConfigAction::SetRentCollector(None));
        }
        SquadsProgram::SmartAccount => {
            actions.push(ConfigAction::SetArchivalAuthority(Some(pk(17))));
            actions.push(ConfigAction::SetArchivalAuthority(None));
        }
    }
    actions
}

#[test]
fn every_typed_action_reads_back_as_it_was_stored_on_either_program() {
    for program in BOTH {
        let squad = match program {
            SquadsProgram::V4 => Squad::v4(&pk(1)),
            SquadsProgram::SmartAccount => Squad::smart_account(7),
        };
        let actions = every_action(program);
        let data = change_account(&squad, 4, pk(2), &encoded(&squad, &actions));
        let read = squad.parse_change(&data).unwrap();
        assert_eq!(read.program, program);
        assert_eq!((read.creator, read.index), (pk(2), 4));
        assert_eq!(read.actions, actions, "{program:?}");
        assert_eq!(
            squad.parse_transaction(&data),
            Some(StoredAccount::Config(read)),
            "{program:?}"
        );

        // Of another multisig: nothing. Cut short: nothing either.
        let elsewhere = Squad {
            program,
            address: pk(1),
        };
        assert_eq!(elsewhere.parse_change(&data), None);
        assert_eq!(elsewhere.parse_transaction(&data), None);
        assert_eq!(squad.parse_change(&data[..data.len() - 3]), None);
        // The other program's actions do not belong here.
        let other = match program {
            SquadsProgram::V4 => ConfigAction::SetArchivalAuthority(None),
            SquadsProgram::SmartAccount => ConfigAction::SetRentCollector(None),
        };
        assert_eq!(squad.create_change(1, &[other], &pk(2), &pk(3)), None);
    }
}

#[test]
fn the_instruction_that_stores_a_change_reads_back_as_its_actions_on_either_program() {
    for program in BOTH {
        let squad = match program {
            SquadsProgram::V4 => Squad::v4(&pk(1)),
            SquadsProgram::SmartAccount => Squad::smart_account(7),
        };
        let actions = every_action(program);
        let store = squad.create_change(4, &actions, &pk(2), &pk(3)).unwrap();
        assert_eq!(
            stored_change_actions(&store.program_id, &store.data),
            Some(actions),
            "{program:?}"
        );
        // Any other instruction of the program, or of another, is none.
        let open = squad.create_proposal(4, &pk(2), &pk(3));
        assert_eq!(stored_change_actions(&open.program_id, &open.data), None);
        assert_eq!(stored_change_actions(&pk(9), &store.data), None);
        assert_eq!(
            stored_change_actions(&store.program_id, &store.data[..12]),
            None
        );
    }
}

#[test]
fn smart_account_policy_actions_are_read_by_name_and_fields() {
    let squad = Squad::smart_account(7);
    let mut actions = vec![3u8, 0, 0, 0];
    // policyCreate: seed 5, a settings change policy that allows changing
    // the threshold, one signer that may do everything, 1 of 1, no time
    // lock, no start, expiring once the settings change.
    actions.push(7);
    actions.extend(5u64.to_le_bytes());
    actions.push(2);
    actions.extend(1u32.to_le_bytes());
    actions.push(2);
    actions.extend(1u32.to_le_bytes());
    actions.extend(pk(20).to_bytes());
    actions.push(7);
    actions.extend(1u16.to_le_bytes());
    actions.extend(0u32.to_le_bytes());
    actions.push(0);
    actions.extend([1, 1]);
    // policyUpdate of policy pk(21): no signer, 1 of ..., the same payload.
    actions.push(8);
    actions.extend(pk(21).to_bytes());
    actions.extend(0u32.to_le_bytes());
    actions.extend(1u16.to_le_bytes());
    actions.extend(60u32.to_le_bytes());
    actions.push(2);
    actions.extend(1u32.to_le_bytes());
    actions.push(2);
    actions.push(0);
    // policyRemove of policy pk(22).
    actions.push(9);
    actions.extend(pk(22).to_bytes());

    let data = change_account(&squad, 3, pk(2), &actions);
    let read = squad.parse_change(&data).expect("read");
    let [create, update, remove] = read.actions.as_slice() else {
        panic!("three actions: {:?}", read.actions);
    };
    let ConfigAction::Policy {
        verb: PolicyVerb::Create,
        policy: None,
        fields,
    } = create
    else {
        panic!("{create:?}");
    };
    let names: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "seed",
            "policyCreationPayload",
            "signers",
            "threshold",
            "timeLock",
            "startTimestamp",
            "expirationArgs"
        ]
    );
    assert_eq!(fields[0].1, "5");
    assert_eq!(fields[1].1, "SettingsChange({actions: [ChangeThreshold]})");
    assert_eq!(fields[6].1, "SettingsState");
    assert!(matches!(
        update,
        ConfigAction::Policy { verb: PolicyVerb::Update, policy: Some(policy), .. } if *policy == pk(21)
    ));
    assert_eq!(
        *remove,
        ConfigAction::Policy {
            verb: PolicyVerb::Remove,
            policy: Some(pk(22)),
            fields: Vec::new(),
        }
    );
    // Read, but neither stored nor executed by this app.
    assert!(read.not_executed_here().is_some());
    assert_eq!(squad.execute_change(&read, &pk(2), &pk(3)), None);
    assert_eq!(squad.create_change(4, &read.actions, &pk(2), &pk(3)), None);

    let lines = change_lines(None, &read.actions);
    let name = |key: &Pubkey| format!("<{}>", key.to_bytes()[0]);
    assert!(lines[0]
        .text(&name)
        .starts_with("Create a policy: seed 5, policyCreationPayload SettingsChange("));
    assert!(lines[1]
        .text(&name)
        .starts_with("Update policy <21>: signers [], "));
    assert_eq!(lines[2].text(&name), "Remove policy <22>");
}

#[test]
fn an_execution_lists_the_spending_limits_it_creates_or_removes() {
    for program in BOTH {
        let squad = match program {
            SquadsProgram::V4 => Squad::v4(&pk(1)),
            SquadsProgram::SmartAccount => Squad::smart_account(7),
        };
        let change = ConfigTransaction {
            program,
            creator: pk(2),
            index: 6,
            actions: every_action(program),
        };
        let execute = squad.execute_change(&change, &pk(3), &pk(4)).unwrap();
        let names: Vec<(Pubkey, bool, bool)> = execute
            .accounts
            .iter()
            .map(|meta| (meta.pubkey, meta.is_signer, meta.is_writable))
            .collect();
        let mut want = vec![
            (squad.address, false, true),
            (pk(3), true, false),
            (squad.proposal(6), false, true),
            (squad.transaction(6), false, false),
            (pk(4), true, true),
            (SYSTEM_PROGRAM_ID, false, false),
        ];
        if program == SquadsProgram::SmartAccount {
            want.push((program.program_id(), false, false));
        }
        want.push((squad.spending_limit(&pk(12)), false, true));
        want.push((pk(15), false, true));
        assert_eq!(names, want, "{program:?}");
        assert_ne!(squad.spending_limit(&pk(12)), squad.spending_limit(&pk(13)));
    }
}

fn team(threshold: u16, members: &[(u8, u8)]) -> SquadState {
    SquadState {
        threshold,
        time_lock: 0,
        transaction_index: 5,
        stale_transaction_index: 0,
        members: members
            .iter()
            .map(|(n, permissions)| SquadsMember {
                key: pk(*n),
                permissions: *permissions,
            })
            .collect(),
        config_authority: None,
        rent_collector: None,
    }
}

fn name(key: &Pubkey) -> String {
    match key.to_bytes()[0] {
        1 => "Alice".into(),
        2 => "Bob".into(),
        3 => "Carol".into(),
        4 => "Dave".into(),
        n => format!("<{n}>"),
    }
}

fn texts(lines: &[Said]) -> Vec<String> {
    lines.iter().map(|said| said.text(&name)).collect()
}

#[test]
fn each_action_reads_as_a_person_would_say_it_in_order() {
    let state = team(
        2,
        &[
            (1, PERMISSION_ALL),
            (2, PERMISSION_ALL),
            (3, PERMISSION_ALL),
        ],
    );
    let actions = [
        ConfigAction::AddMember(SquadsMember {
            key: pk(4),
            permissions: PERMISSION_ALL,
        }),
        ConfigAction::AddMember(SquadsMember {
            key: pk(5),
            permissions: PERMISSION_VOTE,
        }),
        ConfigAction::AddMember(SquadsMember {
            key: pk(6),
            permissions: 0,
        }),
        ConfigAction::RemoveMember(pk(2)),
        ConfigAction::ChangeThreshold(3),
        ConfigAction::ChangeThreshold(3),
        ConfigAction::SetTimeLock(86_400),
        ConfigAction::SetTimeLock(3_600),
        ConfigAction::SetTimeLock(0),
        ConfigAction::AddSpendingLimit(SpendingLimit {
            seed: pk(12),
            vault_index: 0,
            mint: Pubkey::default(),
            amount: 1_500_000_000,
            period: Period::Day,
            members: vec![pk(1), pk(3)],
            destinations: Vec::new(),
            expiration: None,
        }),
        ConfigAction::AddSpendingLimit(SpendingLimit {
            seed: pk(12),
            vault_index: 1,
            mint: pk(30),
            amount: 500,
            period: Period::OneTime,
            members: vec![pk(1)],
            destinations: vec![pk(4), pk(5)],
            expiration: Some(1_798_675_200),
        }),
        ConfigAction::RemoveSpendingLimit(pk(15)),
        ConfigAction::SetRentCollector(Some(pk(1))),
        ConfigAction::SetRentCollector(None),
        ConfigAction::SetArchivalAuthority(Some(pk(3))),
    ];
    assert_eq!(
        texts(&change_lines(Some(&state), &actions)),
        [
            "Add Dave as a member that may propose, vote and execute",
            "Add <5> as a member that may vote",
            "Add <6> as a member that may do nothing",
            "Remove Bob from the members",
            "Change the threshold from 2 to 3 out of 4 voting members",
            "Set the threshold to 3, as it is already",
            "Set a time lock of 1 day: an approved proposal waits that long before it can be executed",
            "Change the time lock from 1 day to 1 hour",
            "Remove the time lock of 1 hour",
            "Let Alice and Carol move up to 1.5 SOL a day out of vault 0 without a vote, to any address",
            "Let Alice move up to 500 of the smallest units of token <30> in all out of vault 1 without a vote, only to Dave and <5>, until 2026-12-31 UTC",
            "Remove spending limit <15>",
            "Let the rent of executed, rejected and cancelled transactions' accounts be collected to Alice",
            "Stop the rent of finished transactions' accounts being collected: they can no longer be closed",
            "Set the archival authority to Carol",
        ]
    );
    // Without the multisig, nothing is said about what it was.
    assert_eq!(
        texts(&change_lines(
            None,
            &[
                ConfigAction::ChangeThreshold(3),
                ConfigAction::SetTimeLock(60)
            ]
        )),
        ["Set the threshold to 3", "Set the time lock to 1 minute"]
    );
    assert_eq!(change_summary(&actions[..1]), "adds a member".to_string());
    assert_eq!(
        change_summary(&[actions[0].clone(), actions[4].clone()]),
        "adds a member and changes the threshold"
    );
    assert_eq!(change_summary(&actions[..3]), "3 changes to the multisig");
}

#[test]
fn the_outcome_is_the_multisig_after_every_action_in_order() {
    let state = team(
        2,
        &[
            (1, PERMISSION_ALL),
            (2, PERMISSION_ALL),
            (3, PERMISSION_ALL),
        ],
    );
    let add_dave = ConfigAction::AddMember(SquadsMember {
        key: pk(4),
        permissions: PERMISSION_ALL,
    });
    let outcome = change_outcome(
        SquadsProgram::V4,
        &state,
        &[add_dave.clone(), ConfigAction::ChangeThreshold(3)],
        0,
    )
    .unwrap();
    assert_eq!(outcome.members.len(), 4);
    assert_eq!(outcome.threshold, 3);
    assert!(outcome.stales);
    assert_eq!(
        outcome_line(&state, &outcome).unwrap().text(&name),
        "After this: 3 of 4 approvals needed"
    );

    // A removal, then a lower threshold, with a time lock.
    let outcome = change_outcome(
        SquadsProgram::SmartAccount,
        &state,
        &[
            ConfigAction::RemoveMember(pk(3)),
            ConfigAction::SetTimeLock(7_200),
        ],
        0,
    )
    .unwrap();
    assert_eq!(
        outcome.members.iter().map(|m| m.key).collect::<Vec<_>>(),
        [pk(1), pk(2)]
    );
    assert_eq!(
        outcome_line(&state, &outcome).unwrap().text(&name),
        "After this: 2 of 2 approvals needed · 2 hours time lock"
    );

    // Spending limits and the rent collector leave the members alone, and
    // nothing goes stale.
    let outcome = change_outcome(
        SquadsProgram::V4,
        &state,
        &[ConfigAction::SetRentCollector(Some(pk(1)))],
        0,
    )
    .unwrap();
    assert!(!outcome.stales);
    assert_eq!(outcome.rent_collector, Some(pk(1)));
    assert_eq!(outcome_line(&state, &outcome), None);

    // Order matters: the threshold first would need more voters than there
    // are.
    assert_eq!(
        change_outcome(
            SquadsProgram::V4,
            &state,
            &[ConfigAction::ChangeThreshold(4), add_dave.clone()],
            0
        )
        .map(|outcome| outcome.threshold),
        Ok(4)
    );
}

#[test]
fn what_the_program_refuses_is_said_as_never_executable() {
    let state = team(
        2,
        &[
            (1, PERMISSION_ALL),
            (2, PERMISSION_ALL),
            (3, PERMISSION_VOTE),
        ],
    );
    let refused =
        |actions: &[ConfigAction]| change_outcome(SquadsProgram::V4, &state, actions, 100);
    assert_eq!(
        refused(&[ConfigAction::RemoveMember(pk(9))]),
        Err(ChangeRefusal::NotAMember(pk(9)))
    );
    assert_eq!(
        refused(&[ConfigAction::AddMember(SquadsMember {
            key: pk(2),
            permissions: PERMISSION_VOTE
        })]),
        Err(ChangeRefusal::AlreadyAMember(pk(2)))
    );
    assert_eq!(
        refused(&[ConfigAction::ChangeThreshold(4)]),
        Err(ChangeRefusal::ThresholdOverVoters {
            threshold: 4,
            voters: 3
        })
    );
    assert_eq!(
        refused(&[ConfigAction::ChangeThreshold(0)]),
        Err(ChangeRefusal::ZeroThreshold)
    );
    assert_eq!(
        refused(&[
            ConfigAction::RemoveMember(pk(1)),
            ConfigAction::RemoveMember(pk(2))
        ]),
        Err(ChangeRefusal::NoProposers)
    );
    assert_eq!(
        refused(&[ConfigAction::SetTimeLock(MAX_TIME_LOCK + 1)]),
        Err(ChangeRefusal::TimeLockTooLong(MAX_TIME_LOCK + 1))
    );
    assert_eq!(refused(&[]), Err(ChangeRefusal::NoActions));
    // Removing a voter so the threshold can no longer be met.
    let two_voters = team(2, &[(1, PERMISSION_ALL), (2, PERMISSION_ALL)]);
    assert_eq!(
        change_outcome(
            SquadsProgram::V4,
            &two_voters,
            &[ConfigAction::RemoveMember(pk(2))],
            0
        ),
        Err(ChangeRefusal::ThresholdOverVoters {
            threshold: 2,
            voters: 1
        })
    );
    let limit = |amount, members: Vec<Pubkey>, expiration| {
        ConfigAction::AddSpendingLimit(SpendingLimit {
            seed: pk(12),
            vault_index: 0,
            mint: Pubkey::default(),
            amount,
            period: Period::Day,
            members,
            destinations: Vec::new(),
            expiration,
        })
    };
    assert_eq!(
        refused(&[limit(0, vec![pk(1)], None)]),
        Err(ChangeRefusal::EmptySpendingLimit)
    );
    assert_eq!(
        refused(&[limit(5, Vec::new(), None)]),
        Err(ChangeRefusal::SpendingLimitForNobody)
    );
    assert_eq!(
        refused(&[limit(5, vec![pk(1), pk(1)], None)]),
        Err(ChangeRefusal::SpendingLimitTwice)
    );
    // Only the smart account program's limits expire.
    let expired = [limit(5, vec![pk(1)], Some(100))];
    assert_eq!(
        change_outcome(SquadsProgram::SmartAccount, &state, &expired, 100),
        Err(ChangeRefusal::SpendingLimitExpired)
    );
    assert!(change_outcome(SquadsProgram::SmartAccount, &state, &expired, 99).is_ok());
    assert_eq!(
        change_outcome(
            SquadsProgram::SmartAccount,
            &state,
            &[ConfigAction::SetArchivalAuthority(None)],
            0
        ),
        Err(ChangeRefusal::ArchivalNotImplemented)
    );
    assert_eq!(
        ChangeRefusal::NotAMember(pk(9)).said().text(&name),
        "The program refuses to execute this change: it removes <9>, which is not a member. It can never be executed, so it is not offered for approval."
    );
    assert_eq!(
        ChangeRefusal::NoExecutors.said().text(&name),
        "The program refuses to execute this change: it would leave no member that may execute. It can never be executed, so it is not offered for approval."
    );
}

#[test]
fn a_threshold_is_said_against_the_members_that_vote_after_the_whole_change() {
    let state = team(
        2,
        &[
            (1, PERMISSION_ALL),
            (2, PERMISSION_ALL),
            (3, PERMISSION_ALL),
        ],
    );
    let add = |n: u8, permissions: u8| {
        ConfigAction::AddMember(SquadsMember {
            key: pk(n),
            permissions,
        })
    };
    let said = |actions: &[ConfigAction]| texts(&change_lines(Some(&state), actions)).join("; ");
    assert_eq!(
        said(&[ConfigAction::ChangeThreshold(3)]),
        "Change the threshold from 2 to 3 out of 3 members"
    );
    // The members the change itself adds count, wherever they come in it.
    assert!(said(&[
        ConfigAction::ChangeThreshold(4),
        add(4, PERMISSION_ALL),
        add(5, PERMISSION_ALL)
    ])
    .starts_with("Change the threshold from 2 to 4 out of 5 members"));
    // A member that may not vote is not counted, and the words say so.
    assert!(said(&[add(4, 0), ConfigAction::ChangeThreshold(3)])
        .ends_with("Change the threshold from 2 to 3 out of 3 voting members"));
    assert_eq!(
        said(&[
            ConfigAction::RemoveMember(pk(2)),
            ConfigAction::RemoveMember(pk(3)),
            ConfigAction::ChangeThreshold(1)
        ])
        .rsplit("; ")
        .next(),
        Some("Change the threshold from 2 to 1 out of 1 member")
    );
    // Without the multisig as it is, nothing is known to count.
    assert_eq!(
        texts(&change_lines(None, &[ConfigAction::ChangeThreshold(3)])),
        ["Set the threshold to 3"]
    );
}
