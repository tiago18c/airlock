//! What a check pins, by where it runs and who signs it, and what the
//! builder makes for each shape once the program takes it.

use super::*;
use crate::fixtures::{compile, pk, transfer};
use crate::idl::catalog::{IdlStore, MemoryBodies};
use crate::keys::{KeypairSigner, WalletSigner};
use crate::program_code::BPF_LOADER_UPGRADEABLE_ID;
use crate::review::blind::blind_in;
use crate::review::explain::explain_transaction;
use crate::tx::builder::{compile_v1, NonceSpec};
use crate::tx::decode::decode_v1_transaction;
use crate::wallets::programmatic::{programmatic_signer_address, Authorization, ExecutorNonce};
use crate::wallets::relay::{wrap_for_programmatic, wrap_for_vector, VectorWrap};
use crate::wallets::squads::{compile_with_checks, LOADER_UPGRADE};
use crate::wallets::vector::{address, passthrough, SchemeId, PROGRAM_ID};
use solana_hash::Hash;
use solana_instruction::AccountMeta;
use std::collections::BTreeMap;
use wallet_vault::store::MemoryStore;

const HASH: &str = "7ff737ad44ad241dc6cb45dae1010ba61774c6d55b662876282dbc40a9efe573";

fn squad() -> Squad {
    Squad::v4(&pk(50))
}

fn upgrade(program: Pubkey, buffer: Pubkey, authority: Pubkey) -> Instruction {
    Instruction {
        program_id: BPF_LOADER_UPGRADEABLE_ID,
        accounts: vec![
            AccountMeta::new(pk(60), false),
            AccountMeta::new(program, false),
            AccountMeta::new(buffer, false),
            AccountMeta::new(pk(61), false),
            AccountMeta::new_readonly(pk(62), false),
            AccountMeta::new_readonly(pk(63), false),
            AccountMeta::new_readonly(authority, true),
        ],
        data: LOADER_UPGRADE.to_le_bytes().to_vec(),
    }
}

/// What proposal 4 stores: a payment, and an upgrade of program pk(71) from
/// buffer pk(70), which the vault authorizes.
fn stored_instructions() -> Vec<Instruction> {
    let vault = squad().vault(0);
    vec![transfer(vault, pk(2), 5), upgrade(pk(71), pk(70), vault)]
}

/// The check of proposal 4 for `member`'s approval, at the top.
fn check_of(member: Pubkey) -> ProposalCheck {
    ProposalCheck {
        form: CheckForm::TopLevel,
        squad: squad(),
        transaction_index: 4,
        vault_index: 0,
        vault: squad().vault(0),
        ephemeral_signers: 0,
        member,
        action: VoteAction::Approve,
        instructions: stored_instructions(),
    }
}

fn check(member: Pubkey) -> Instruction {
    check_of(member).instruction(&[])
}

/// The same check as `verifyProposalCpi`, for a member that signs through
/// a program.
fn cpi(member: Pubkey) -> Instruction {
    ProposalCheck {
        form: CheckForm::Cpi,
        ..check_of(member)
    }
    .instruction(&[])
}

fn vote(member: Pubkey) -> Instruction {
    squad().approve(4, &member)
}

fn execute(member: Pubkey) -> Instruction {
    squad().execute(4, 0, &stored_instructions(), &member)
}

fn buffer_check(buffer: Pubkey, hash: &str) -> Instruction {
    BufferHashCheck {
        buffer,
        hash: hash.into(),
    }
    .instruction()
}

fn catalog() -> IdlStore<MemoryStore, MemoryBodies> {
    IdlStore::open(MemoryStore::new(), MemoryBodies::new())
}

fn read(wire: &[u8]) -> ExplainedTx {
    explain_transaction(&catalog(), &decode_v1_transaction(wire).unwrap())
}

/// `instructions` paid by pk(1).
fn explained(instructions: Vec<Instruction>) -> ExplainedTx {
    read(&compile(pk(1), instructions).wire)
}

fn checks(instructions: Vec<Instruction>) -> Vec<CheckedApproval> {
    checked_in(&explained(instructions))
}

fn pinned_members(instructions: Vec<Instruction>) -> Vec<Pubkey> {
    checks(instructions)
        .iter()
        .flat_map(|approval| approval.votes.iter().map(|vote| vote.member))
        .collect()
}

/// Nothing pinned, and the vote reads as blind, as it always has.
#[track_caller]
fn still_blind(instructions: Vec<Instruction>) {
    let explained = explained(instructions);
    assert!(checked_in(&explained).is_empty());
    assert_eq!(blind_in(&explained).len(), 1);
}

/// A programmatic signer: its authority, and the address it signs as.
fn programmatic(n: u8) -> (Pubkey, Pubkey) {
    let authority = pk(n);
    (authority, programmatic_signer_address(&authority))
}

/// A `Submit` by `authority` carrying `instructions`, unsigned.
fn submit(authority: Pubkey, instructions: &[Instruction]) -> Instruction {
    let address = programmatic_signer_address(&authority);
    let nonce = ExecutorNonce {
        account: pk(22),
        authority: address,
        value: Hash::new_from_array([3; 32]),
    };
    Authorization::new(address, instructions, &[authority], nonce)
        .unwrap()
        .submit_instruction_unsigned(&BTreeMap::new())
        .unwrap()
}

#[test]
fn a_place_reads_as_the_screen_numbers_it() {
    assert_eq!(IxPlace::top(1).label(), "instruction 2");
    assert_eq!(
        IxPlace(vec![2, 0, 1]).label(),
        "instruction 2 inside instruction 1 inside instruction 3"
    );
}

#[test]
fn a_vote_the_chain_checks_is_not_blind_and_says_what_it_pins() {
    let explained = explained(vec![buffer_check(pk(70), HASH), check(pk(1)), vote(pk(1))]);
    assert!(blind_in(&explained).is_empty());
    assert_eq!(
        checked_in(&explained),
        [CheckedApproval {
            squad: squad(),
            proposal: squad().proposal(4),
            transaction_index: 4,
            vault: squad().vault(0),
            check: IxPlace::top(1),
            votes: vec![PinnedVote {
                member: pk(1),
                at: IxPlace::top(2),
            }],
            execute: None,
            upgrades: vec![CheckedUpgrade {
                program: pk(71),
                buffer: pk(70),
                hash: Some(HASH.into()),
            }],
        }]
    );
    // A lookup table after the check's own accounts changes nothing.
    assert_eq!(
        pinned_members(vec![check_of(pk(1)).instruction(&[pk(80)]), vote(pk(1))]),
        [pk(1)]
    );
}

#[test]
fn a_vote_is_blind_again_when_any_one_thing_does_not_hold() {
    let with = |change: &dyn Fn(&mut ProposalCheck)| {
        let mut check = check_of(pk(1));
        change(&mut check);
        vec![check.instruction(&[]), vote(pk(1))]
    };
    still_blind(with(&|check| check.member = pk(2)));
    still_blind(with(&|check| check.squad = Squad::v4(&pk(51))));
    still_blind(with(&|check| check.action = VoteAction::Reject));
    still_blind(with(&|check| check.transaction_index = 5));

    // The check and the vote agree on a proposal, but not the one derived
    // for the number.
    let mut moved = check(pk(1));
    moved.accounts[0].pubkey = pk(77);
    let mut elsewhere = vote(pk(1));
    elsewhere.accounts[2].pubkey = pk(77);
    still_blind(vec![moved.clone(), elsewhere]);
    // The vote's proposal is derived, the check's is not.
    still_blind(vec![moved, vote(pk(1))]);
    // The transaction it compares is not the one derived.
    let mut other = check(pk(1));
    other.accounts[1].pubkey = pk(78);
    still_blind(vec![other, vote(pk(1))]);

    // No check at all, or one that does not read to its last byte.
    still_blind(vec![vote(pk(1))]);
    let mut longer = check(pk(1));
    longer.data.push(0);
    still_blind(vec![longer, vote(pk(1))]);
    let mut shorter = check(pk(1));
    shorter.data.pop();
    still_blind(vec![shorter, vote(pk(1))]);
}

#[test]
fn what_else_the_transaction_holds_does_not_unpin_a_vote() {
    // A payment, a second identical check, the check after the vote.
    assert_eq!(
        pinned_members(vec![check(pk(1)), vote(pk(1)), transfer(pk(1), pk(2), 5)]),
        [pk(1)]
    );
    assert_eq!(
        pinned_members(vec![check(pk(1)), check(pk(1)), vote(pk(1))]),
        [pk(1)]
    );
    assert_eq!(pinned_members(vec![vote(pk(1)), check(pk(1))]), [pk(1)]);
    // On a durable nonce.
    let nonce = NonceSpec {
        account: pk(30),
        authority: pk(1),
        hash: Hash::new_from_array([3; 32]),
    };
    let wire = compile_v1(pk(1), vec![check(pk(1)), vote(pk(1))], Some(nonce), None)
        .unwrap()
        .wire;
    assert_eq!(checked_in(&read(&wire))[0].votes.len(), 1);
}

#[test]
fn checks_that_carry_different_things_pin_nothing() {
    let mut other = check_of(pk(1));
    other.instructions.pop();
    still_blind(vec![check(pk(1)), other.instruction(&[]), vote(pk(1))]);
    // A rejecting check carrying something else cannot hold either.
    let mut rejecting = check_of(pk(2));
    rejecting.action = VoteAction::Reject;
    rejecting.instructions.pop();
    still_blind(vec![check(pk(1)), rejecting.instruction(&[]), vote(pk(1))]);
}

#[test]
fn each_member_is_pinned_by_its_own_check_and_one_without_stays_blind() {
    let instructions = vec![
        check(pk(1)),
        check(pk(2)),
        vote(pk(1)),
        vote(pk(2)),
        vote(pk(3)),
    ];
    let explained = explained(instructions);
    let found = checked_in(&explained);
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].votes,
        [
            PinnedVote {
                member: pk(1),
                at: IxPlace::top(2),
            },
            PinnedVote {
                member: pk(2),
                at: IxPlace::top(3),
            },
        ]
    );
    assert_eq!(found[0].check, IxPlace::top(0));
    // The third member's vote is still blind, and reads as such.
    assert_eq!(blind_in(&explained).len(), 1);
    assert!(!found[0].pins_vote_of(&pk(3)));
    match approval_shape(&explained) {
        ApprovalShape::Open(open) => assert_eq!(open.votes.len(), 3),
        other => panic!("{other:?}"),
    }
}

#[test]
fn executing_the_same_proposal_is_pinned_too() {
    let explained = explained(vec![check(pk(1)), vote(pk(1)), execute(pk(1))]);
    let found = checked_in(&explained);
    assert_eq!(found[0].execute, Some(IxPlace::top(2)));
    assert_eq!(found[0].votes.len(), 1);
    assert!(blind_in(&explained).is_empty());
    assert_eq!(approval_shape(&explained), ApprovalShape::Checked);

    // Executing it by another member, with no vote here, is pinned the same.
    let found = checks(vec![check(pk(1)), execute(pk(2))]);
    assert_eq!(found[0].execute, Some(IxPlace::top(1)));
    assert!(found[0].votes.is_empty());

    // Without a check it runs blind, and an execution of another proposal
    // stays blind beside a pinned vote.
    let explained_alone = explained_of(vec![execute(pk(1))]);
    assert!(checked_in(&explained_alone).is_empty());
    assert_eq!(blind_in(&explained_alone).len(), 1);
    let elsewhere = squad().execute(5, 0, &stored_instructions(), &pk(1));
    let explained = explained_of(vec![check(pk(1)), vote(pk(1)), elsewhere]);
    assert_eq!(checked_in(&explained)[0].execute, None);
    assert_eq!(blind_in(&explained).len(), 1);
}

fn explained_of(instructions: Vec<Instruction>) -> ExplainedTx {
    explained(instructions)
}

#[test]
fn a_check_beside_a_submit_pins_no_vote_inside_it() {
    // The authority signs only what the `Submit` carries: whoever sends it
    // could drop a check beside it after the authority signed. The program
    // does not run either form there anyway.
    let (authority, member) = programmatic(20);
    for beside in [check(member), cpi(member)] {
        still_blind(vec![beside.clone(), submit(authority, &[vote(member)])]);
        still_blind(vec![submit(authority, &[vote(member)]), beside]);
    }
    // Nor does a check inside another signer's `Submit`.
    let (other, _) = programmatic(21);
    still_blind(vec![
        submit(other, &[cpi(member)]),
        submit(authority, &[vote(member)]),
    ]);
    // Nor a check that only a stored list carries inside the same `Submit`.
    let stored = Squad::v4(&pk(51))
        .create_transaction(1, 0, &[cpi(member)], &member, &member)
        .unwrap();
    still_blind(vec![submit(authority, &[stored, vote(member)])]);
}

#[test]
fn a_check_by_cpi_inside_the_same_submit_pins_the_vote() {
    let (authority, member) = programmatic(20);
    let explained = explained(vec![submit(authority, &[cpi(member), vote(member)])]);
    let found = checked_in(&explained);
    assert_eq!(found.len(), 1);
    // The `Submit`, the executor's `Execute` in it, then what that runs.
    assert_eq!(found[0].check, IxPlace(vec![0, 0, 0]));
    assert_eq!(
        found[0].votes,
        [PinnedVote {
            member,
            at: IxPlace(vec![0, 0, 1]),
        }]
    );
    assert!(blind_in(&explained).is_empty());
    assert_eq!(
        found[0].check.label(),
        "instruction 1 inside instruction 1 inside instruction 1"
    );
    // A `verifyProposal` there never runs: the program wants it at the top,
    // and would refuse the `Submit` beside it.
    still_blind(vec![submit(authority, &[check(member), vote(member)])]);
    // A check by CPI naming another member, or with another account where
    // the member goes, is not this vote's.
    let (_, stranger) = programmatic(21);
    still_blind(vec![submit(authority, &[cpi(stranger), vote(member)])]);
    let mut moved = cpi(member);
    moved.accounts[3].pubkey = stranger;
    still_blind(vec![submit(authority, &[moved, vote(member)])]);
}

#[test]
fn a_check_by_cpi_at_the_top_pins_nothing() {
    // The program refuses it there (6048), whoever signs.
    still_blind(vec![cpi(pk(1)), vote(pk(1))]);
    let (authority, member) = programmatic(20);
    still_blind(vec![cpi(member), submit(authority, &[vote(member)])]);
}

#[test]
fn a_key_that_signs_the_transaction_is_pinned_only_at_the_top() {
    let (authority, _) = programmatic(20);
    // pk(1) pays, so it signs the whole transaction, `Submit` and all, but a
    // check in the `Submit` does not look at the top.
    still_blind(vec![submit(authority, &[check(pk(1))]), vote(pk(1))]);
    // A hot member's vote inside the `Submit`, as a forwarded signer: a
    // `verifyProposal` does not look there, and a `verifyProposalCpi` there
    // is for an address the `Submit` signs as, which pk(1) is not.
    still_blind(vec![check(pk(1)), submit(authority, &[vote(pk(1))])]);
    still_blind(vec![submit(authority, &[cpi(pk(1)), vote(pk(1))])]);
    // At the top, with a `Submit` beside it.
    assert_eq!(
        pinned_members(vec![
            check(pk(1)),
            vote(pk(1)),
            submit(authority, &[transfer(pk(2), pk(3), 1)])
        ]),
        [pk(1)]
    );
}

#[test]
fn each_vote_in_a_mixed_transaction_is_pinned_by_its_own_form() {
    let (authority, member) = programmatic(20);
    let explained = explained(vec![
        check(pk(1)),
        vote(pk(1)),
        submit(authority, &[cpi(member), vote(member)]),
    ]);
    let found = checked_in(&explained);
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].votes,
        [
            PinnedVote {
                member: pk(1),
                at: IxPlace::top(1),
            },
            PinnedVote {
                member,
                at: IxPlace(vec![2, 0, 1]),
            },
        ]
    );
    assert_eq!(found[0].check, IxPlace::top(0));
    assert!(blind_in(&explained).is_empty());
    // Each check in the other one's place pins neither: the proposal reads
    // as blind.
    still_blind(vec![
        cpi(pk(1)),
        vote(pk(1)),
        submit(authority, &[check(member), vote(member)]),
    ]);
}

#[test]
fn an_execution_inside_a_submit_needs_a_check_inside_it() {
    let (authority, member) = programmatic(20);
    let explained = explained(vec![
        check(pk(1)),
        vote(pk(1)),
        submit(authority, &[execute(member)]),
    ]);
    let found = checked_in(&explained);
    assert_eq!(found[0].votes.len(), 1);
    assert_eq!(found[0].execute, None);
    assert_eq!(blind_in(&explained).len(), 1);
    let inside = checks(vec![
        check(pk(1)),
        vote(pk(1)),
        submit(authority, &[cpi(member), execute(member)]),
    ]);
    assert_eq!(inside[0].execute, Some(IxPlace(vec![2, 0, 1])));
}

#[test]
fn a_buffer_hash_counts_only_where_every_pinned_signature_covers_it() {
    let (authority, member) = programmatic(20);
    let hash_of = |instructions: Vec<Instruction>| {
        let found = checks(instructions);
        assert_eq!(found[0].votes.len(), 1);
        found[0].upgrades[0].hash.clone()
    };
    // Beside the `Submit`, the hash could be dropped after the authority
    // signed.
    assert_eq!(
        hash_of(vec![
            buffer_check(pk(70), HASH),
            submit(authority, &[cpi(member), vote(member)]),
        ]),
        None
    );
    assert_eq!(
        hash_of(vec![submit(
            authority,
            &[buffer_check(pk(70), HASH), cpi(member), vote(member)]
        )]),
        Some(HASH.into())
    );
    // At the top, for a key that signs the transaction.
    assert_eq!(
        hash_of(vec![buffer_check(pk(70), HASH), check(pk(1)), vote(pk(1))]),
        Some(HASH.into())
    );
    assert_eq!(hash_of(vec![check(pk(1)), vote(pk(1))]), None);
    assert_eq!(
        hash_of(vec![buffer_check(pk(72), HASH), check(pk(1)), vote(pk(1))]),
        None
    );
    // Checked against another account than the one it names.
    let mut elsewhere = buffer_check(pk(70), HASH);
    elsewhere.accounts[0].pubkey = pk(72);
    assert_eq!(hash_of(vec![elsewhere, check(pk(1)), vote(pk(1))]), None);
    // Two hashes for one buffer cannot both hold.
    let other = HASH.replace('7', "8");
    assert_eq!(
        hash_of(vec![
            buffer_check(pk(70), HASH),
            buffer_check(pk(70), &other),
            check(pk(1)),
            vote(pk(1)),
        ]),
        None
    );
}

/// A Vector account of an Ed25519 key, and that key.
fn vector_account() -> (Pubkey, Vec<u8>) {
    let key = KeypairSigner::new_random().pubkey().to_bytes().to_vec();
    (address(&PROGRAM_ID, SchemeId::Ed25519, &key).unwrap(), key)
}

/// `instructions` in a `Passthrough` of the account of `key` that signs as
/// its system PDA.
fn as_system(key: &[u8], instructions: &[Instruction]) -> Instruction {
    let mut flagged = passthrough(SchemeId::Ed25519, key, &[]).unwrap();
    // Data: tag, signer count, then each signer's flag.
    flagged.data[2] = 1;
    crate::wallets::vector::rewrite_passthrough(&flagged, instructions).unwrap()
}

#[test]
fn a_vote_in_a_passthrough_is_pinned_only_by_a_check_by_cpi_inside_it() {
    let (account, key) = vector_account();
    let inside = passthrough(SchemeId::Ed25519, &key, &[cpi(account), vote(account)]).unwrap();
    let explained = explained(vec![inside.clone()]);
    let found = checked_in(&explained);
    assert_eq!(found[0].check, IxPlace(vec![0, 0]));
    assert_eq!(
        found[0].votes,
        [PinnedVote {
            member: account,
            at: IxPlace(vec![0, 1]),
        }]
    );
    assert!(blind_in(&explained).is_empty());
    // A Vector wallet's key signs every instruction of the transaction, but
    // the program runs no `verifyProposal` beside a Vector instruction, nor
    // one inside a `Passthrough`, nor a `verifyProposalCpi` outside it.
    let carried = passthrough(SchemeId::Ed25519, &key, &[vote(account)]).unwrap();
    still_blind(vec![check(account), carried.clone()]);
    still_blind(vec![cpi(account), carried.clone()]);
    still_blind(vec![passthrough(
        SchemeId::Ed25519,
        &key,
        &[check(account), vote(account)],
    )
    .unwrap()]);
    // Without one, it is blind.
    still_blind(vec![carried.clone()]);
    // In one `Passthrough`, for a vote in another, it pins nothing either.
    let lone_check = passthrough(SchemeId::Ed25519, &key, &[cpi(account)]).unwrap();
    still_blind(vec![lone_check, carried]);
    // A second `Passthrough` in the transaction could use the account's
    // signature outside the checked list, so the program runs no check.
    let other = passthrough(SchemeId::Ed25519, &key, &[transfer(pk(2), pk(3), 1)]).unwrap();
    still_blind(vec![inside.clone(), other]);
}

#[test]
fn a_vote_of_a_system_pda_is_pinned_by_a_check_by_cpi_for_it() {
    let (account, key) = vector_account();
    let pda = crate::wallets::vector::system_pda(&account);
    let inside = as_system(&key, &[cpi(pda), vote(pda)]);
    let explained = explained(vec![inside]);
    let found = checked_in(&explained);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].votes[0].member, pda);
    assert_eq!(found[0].votes[0].at, IxPlace(vec![0, 1]));
    assert!(blind_in(&explained).is_empty());
    // A `Passthrough` that signs as the account itself does not sign as the
    // PDA, so the program could not run the check.
    let as_account = passthrough(SchemeId::Ed25519, &key, &[cpi(pda), vote(pda)]).unwrap();
    still_blind(vec![as_account]);
    // And one that signs as the PDA does not sign as the account.
    still_blind(vec![as_system(&key, &[cpi(account), vote(account)])]);
}

#[test]
fn only_a_list_that_runs_pins_a_vote() {
    // A check and a vote stored for a vault to run some other time.
    let other = Squad::v4(&pk(51));
    let pair = [check(pk(1)), vote(pk(1))];
    let carrying = other
        .create_transaction(1, 0, &pair, &pk(1), &pk(1))
        .unwrap();
    let explained = explained(vec![carrying.clone()]);
    assert!(checked_in(&explained).is_empty());
    assert_eq!(blind_in(&explained).len(), 1);
    // A stored check beside a vote that runs pins nothing either.
    let stored_check = other
        .create_transaction(1, 0, &[check(pk(1))], &pk(1), &pk(1))
        .unwrap();
    still_blind(vec![stored_check, vote(pk(1))]);
    // Nor does a list carried for another transaction.
    let carried = crate::review::explain::explain_instructions(&catalog(), &pair);
    assert_eq!(crate::review::blind::blind_in_carried(&carried).len(), 1);
}

#[test]
fn a_smart_account_approval_is_never_pinned() {
    let smart = Squad::smart_account(7);
    let check = ProposalCheck {
        squad: Squad {
            program: SquadsProgram::V4,
            address: smart.address,
        },
        ..check_of(pk(1))
    };
    let mut instruction = check.instruction(&[]);
    instruction.accounts[0].pubkey = smart.proposal(4);
    instruction.accounts[1].pubkey = smart.transaction(4);
    still_blind(vec![instruction, smart.approve(4, &pk(1))]);
}

fn shape(instructions: Vec<Instruction>) -> ApprovalShape {
    approval_shape(&explained(instructions))
}

fn open(votes: &[Pubkey]) -> OpenApproval {
    OpenApproval {
        squad: squad(),
        proposal: squad().proposal(4),
        votes: votes
            .iter()
            .map(|member| V4Vote {
                squad: squad(),
                proposal: squad().proposal(4),
                member: *member,
                carried: false,
            })
            .collect(),
        execute: false,
        nonce: false,
        in_submit: false,
        in_passthrough: false,
    }
}

/// [`open`], each vote carried by its signer's `Submit` or `Passthrough`.
fn carried(votes: &[Pubkey]) -> OpenApproval {
    let mut open = open(votes);
    for vote in &mut open.votes {
        vote.carried = true;
    }
    open
}

#[test]
fn the_shape_says_what_checks_would_join() {
    assert_eq!(
        shape(vec![vote(pk(1))]),
        ApprovalShape::Open(open(&[pk(1)]))
    );
    assert_eq!(
        shape(vec![check(pk(1)), vote(pk(1))]),
        ApprovalShape::Checked
    );
    assert_eq!(
        shape(vec![vote(pk(1)), vote(pk(2)), execute(pk(1))]),
        ApprovalShape::Open(OpenApproval {
            execute: true,
            ..open(&[pk(1), pk(2)])
        })
    );
    // On a durable nonce.
    let nonce = NonceSpec {
        account: pk(30),
        authority: pk(1),
        hash: Hash::new_from_array([3; 32]),
    };
    let wire = compile_v1(pk(1), vec![vote(pk(1))], Some(nonce), None)
        .unwrap()
        .wire;
    assert_eq!(
        approval_shape(&read(&wire)),
        ApprovalShape::Open(OpenApproval {
            nonce: true,
            ..open(&[pk(1)])
        })
    );
    // Anything else, another proposal's execution, the same member twice,
    // a smart account's vote, an execution alone, a vote only stored.
    let elsewhere = squad().execute(5, 0, &stored_instructions(), &pk(1));
    for other in [
        vec![vote(pk(1)), transfer(pk(1), pk(2), 5)],
        vec![vote(pk(1)), elsewhere],
        vec![vote(pk(1)), vote(pk(1))],
        vec![Squad::smart_account(7).approve(4, &pk(1))],
        vec![execute(pk(1))],
        vec![Squad::v4(&pk(51))
            .create_transaction(1, 0, &[vote(pk(1))], &pk(1), &pk(1))
            .unwrap()],
    ] {
        assert_eq!(shape(other.clone()), ApprovalShape::Other, "{other:?}");
    }
    // Wrapped for a keyless member.
    let (authority, member) = programmatic(20);
    assert_eq!(
        shape(vec![submit(authority, &[vote(member)])]),
        ApprovalShape::Open(OpenApproval {
            in_submit: true,
            ..carried(&[member])
        })
    );
    assert_eq!(
        shape(vec![submit(authority, &[cpi(member), vote(member)])]),
        ApprovalShape::Checked
    );
    let (account, key) = vector_account();
    let in_passthrough = passthrough(SchemeId::Ed25519, &key, &[vote(account)]).unwrap();
    assert_eq!(
        shape(vec![in_passthrough]),
        ApprovalShape::Open(OpenApproval {
            in_passthrough: true,
            ..carried(&[account])
        })
    );
    let pda = crate::wallets::vector::system_pda(&account);
    assert_eq!(
        shape(vec![as_system(&key, &[vote(pda)])]),
        ApprovalShape::Open(OpenApproval {
            in_passthrough: true,
            ..carried(&[pda])
        })
    );
    // A vote no check could reach: in a `Submit` that does not sign as its
    // member, or beside a Vector `Advance` but in no `Passthrough`.
    assert_eq!(
        shape(vec![submit(authority, &[vote(pk(1))])]),
        ApprovalShape::Other
    );
    let signature = vec![0; SchemeId::Ed25519.signature_len()];
    let advance = crate::wallets::vector::advance(&[crate::wallets::vector::AdvanceEntry {
        scheme: SchemeId::Ed25519,
        key: &key,
        next_key: &[],
        signature: &signature,
    }])
    .unwrap();
    assert_eq!(shape(vec![advance, vote(pk(1))]), ApprovalShape::Other);
}

fn stored() -> VaultTransaction {
    VaultTransaction::for_test(&squad(), 4, 0, &stored_instructions())
}

/// `wire`, read for adding checks, built with every shape allowed and a
/// buffer hash, compiled again as the flow compiles it, paid by `payer`.
fn built(wire: &[u8], payer: Pubkey) -> (PlainApproval, Vec<u8>) {
    let tx = decode_v1_transaction(wire).unwrap();
    let plain = plain_approval(&tx).expect("an approval");
    let buffer = BufferHashCheck {
        buffer: pk(70),
        hash: HASH.into(),
    };
    let instructions = checked_instructions(
        &plain,
        &stored(),
        std::slice::from_ref(&buffer),
        ClearSigningAccepts::EVERYTHING,
    )
    .unwrap();
    let wire = compile_with_checks(payer, &tx.budget, &instructions, tx.lifetime).unwrap();
    (plain, wire)
}

/// Every vote in `wire` and its execution, if any, are pinned, every
/// upgrade's buffer hashed.
#[track_caller]
fn all_pinned(wire: &[u8], members: &[Pubkey], executes: bool) -> CheckedApproval {
    let explained = read(wire);
    let found = checked_in(&explained);
    assert_eq!(found.len(), 1);
    let approval = found[0].clone();
    for member in members {
        assert!(approval.pins_vote_of(member), "{member}");
    }
    assert_eq!(approval.execute.is_some(), executes);
    assert!(approval
        .upgrades
        .iter()
        .all(|upgrade| upgrade.hash.is_some()));
    assert!(blind_in(&explained).is_empty());
    assert_eq!(approval_shape(&explained), ApprovalShape::Checked);
    approval
}

#[test]
fn a_lone_approval_is_built_as_the_deployed_program_takes_it() {
    let wire = compile(pk(8), vec![vote(pk(9))]).wire;
    let (plain, with) = built(&wire, pk(8));
    assert_eq!(plain.shape, open(&[pk(9)]));
    let native = decode_v1_transaction(&with).unwrap().native_instructions();
    assert!(BufferHashCheck::parse(&native[0]).is_some());
    assert_eq!(ProposalCheck::parse(&native[1]).unwrap().member, pk(9));
    assert_eq!(native[2], vote(pk(9)));
    all_pinned(&with, &[pk(9)], false);
    // The deployed program takes this one.
    let deployed = checked_instructions(&plain, &stored(), &[], ClearSigningAccepts::DEPLOYED);
    assert!(deployed.is_ok());
}

#[test]
fn approving_together_and_executing_is_built_with_a_check_per_member() {
    let wire = compile(pk(8), vec![vote(pk(9)), vote(pk(10)), execute(pk(9))]).wire;
    let (plain, with) = built(&wire, pk(8));
    assert_eq!(plain.shape.facts().votes, 2);
    let native = decode_v1_transaction(&with).unwrap().native_instructions();
    let members: Vec<Pubkey> = native
        .iter()
        .filter_map(ProposalCheck::parse)
        .map(|check| check.member)
        .collect();
    assert_eq!(members, [pk(9), pk(10)]);
    all_pinned(&with, &[pk(9), pk(10)], true);
    // What the deployed program refuses, the builder refuses.
    assert_eq!(
        checked_instructions(&plain, &stored(), &[], ClearSigningAccepts::DEPLOYED),
        Err(ShapeRefused::Execute)
    );
    let together = compile(pk(8), vec![vote(pk(9)), vote(pk(10))]).wire;
    let plain = plain_approval(&decode_v1_transaction(&together).unwrap()).unwrap();
    assert_eq!(
        checked_instructions(&plain, &stored(), &[], ClearSigningAccepts::DEPLOYED),
        Err(ShapeRefused::SeveralVotes)
    );
}

#[test]
fn a_nonce_advance_stays_first() {
    let nonce = NonceSpec {
        account: pk(30),
        authority: pk(8),
        hash: Hash::new_from_array([3; 32]),
    };
    let wire = compile_v1(pk(8), vec![vote(pk(9)), execute(pk(9))], Some(nonce), None)
        .unwrap()
        .wire;
    let (plain, with) = built(&wire, pk(8));
    assert!(plain.shape.nonce && plain.nonce.is_some());
    let tx = decode_v1_transaction(&with).unwrap();
    assert!(tx.uses_durable_nonce);
    assert_eq!(tx.lifetime, Hash::new_from_array([3; 32]));
    assert_eq!(tx.native_instructions()[0], plain.nonce.clone().unwrap());
    all_pinned(&with, &[pk(9)], true);
    assert_eq!(
        checked_instructions(&plain, &stored(), &[], ClearSigningAccepts::DEPLOYED),
        Err(ShapeRefused::DurableNonce)
    );
}

#[test]
fn a_programmatic_members_check_ends_up_inside_its_submit() {
    let (authority, member) = programmatic(20);
    let hot = pk(8);
    let nonce = ExecutorNonce {
        account: pk(22),
        authority: member,
        value: Hash::new_from_array([3; 32]),
    };
    let wrap = |wire: &[u8]| {
        wrap_for_programmatic(
            &decode_v1_transaction(wire).unwrap(),
            &[authority],
            nonce,
            hot,
            Hash::new_from_array([9; 32]),
        )
        .unwrap()
    };
    // As the app has it on Sign: rebuilt for the member already.
    let wrapped = wrap(&compile(hot, vec![vote(member)]).wire);
    let (plain, with) = built(&wrapped, hot);
    assert!(plain.shape.in_submit && plain.shape.wrapped());
    assert!(plain.shape.votes[0].carried);
    // The plain list is the vote as first written, and the checks join it.
    assert_eq!(plain.instructions, [vote(member)]);
    // The keyless rebuild again, around the checks.
    let again = wrap(&with);
    let approval = all_pinned(&again, &[member], false);
    assert_eq!(approval.check.0.len(), 3);
    // The transaction is the `Submit` alone, and what it runs is the buffer
    // hash, the check by CPI for the member, and the vote: nothing else.
    let tx = decode_v1_transaction(&again).unwrap();
    assert_eq!(tx.native_instructions().len(), 1);
    let explained = read(&again);
    let InnerKind::Authorization { .. } = &explained.instructions[0].inner.as_ref().unwrap().kind
    else {
        panic!("a Submit");
    };
    let execute = &explained.instructions[0]
        .inner
        .as_ref()
        .unwrap()
        .instructions;
    assert_eq!(execute.len(), 1);
    let runs = &execute[0].inner.as_ref().unwrap().instructions;
    assert_eq!(runs.len(), 3);
    assert!(BufferHashCheck::from_data(&runs[0].data).is_some());
    let check = ProposalCheck::from_data(&runs[1].data).unwrap();
    assert_eq!((check.form, check.member), (CheckForm::Cpi, member));
    assert_eq!(runs[1].accounts[3], member);
    assert_eq!(runs[2].data, vote(member).data);
    // The deployed program takes it.
    assert!(checked_instructions(&plain, &stored(), &[], ClearSigningAccepts::DEPLOYED).is_ok());
    // A program that took no check in a `Submit` would be told apart.
    let top_only = ClearSigningAccepts {
        in_submit: false,
        ..ClearSigningAccepts::DEPLOYED
    };
    assert_eq!(
        checked_instructions(&plain, &stored(), &[], top_only),
        Err(ShapeRefused::InSubmit)
    );
}

#[test]
fn a_vector_members_checks_go_inside_its_passthrough() {
    let (account, key) = vector_account();
    let hot = pk(8);
    let wrap = |wire: &[u8]| {
        wrap_for_vector(
            &decode_v1_transaction(wire).unwrap(),
            &[VectorWrap {
                account,
                scheme: SchemeId::Ed25519,
                key: key.clone(),
                next_key: None,
            }],
            hot,
            Hash::new_from_array([9; 32]),
        )
        .unwrap()
    };
    let wrapped = wrap(&compile(hot, vec![vote(account)]).wire);
    let (plain, with) = built(&wrapped, hot);
    assert!(plain.shape.in_passthrough && plain.shape.votes[0].carried);
    assert_eq!(plain.instructions, [vote(account)]);
    let again = wrap(&with);
    let approval = all_pinned(&again, &[account], false);
    assert_eq!(approval.check.0.len(), 2);
    // The `Advance` and one `Passthrough`, which runs the buffer hash, the
    // check by CPI for the account, and the vote: nothing else.
    let explained = read(&again);
    assert_eq!(explained.instructions.len(), 2);
    assert_eq!(explained.instructions[0].data[0], ADVANCE_DISCRIMINATOR);
    let runs = &explained.instructions[1]
        .inner
        .as_ref()
        .unwrap()
        .instructions;
    assert_eq!(runs.len(), 3);
    assert!(BufferHashCheck::from_data(&runs[0].data).is_some());
    let check = ProposalCheck::from_data(&runs[1].data).unwrap();
    assert_eq!((check.form, check.member), (CheckForm::Cpi, account));
    assert_eq!(runs[2].data, vote(account).data);
    assert!(checked_instructions(&plain, &stored(), &[], ClearSigningAccepts::DEPLOYED).is_ok());
    let top_only = ClearSigningAccepts {
        in_passthrough: false,
        ..ClearSigningAccepts::DEPLOYED
    };
    assert_eq!(
        checked_instructions(&plain, &stored(), &[], top_only),
        Err(ShapeRefused::InPassthrough)
    );
}
