//! A smart account whose members on this device can carry every vote and
//! execute runs the site's instructions at once, in one
//! `executeTransactionSync`.

use super::squads::{add_hot, add_pair};
use super::*;
use crate::wallets::squads::SyncExecution;

const SMART: SquadsProgram = SquadsProgram::SmartAccount;

#[test]
fn a_member_that_may_do_everything_runs_it_at_once() {
    let mut d = desk();
    let (member, member_id) = add_hot(&mut d, "Member");
    let squad = d.add_squad(SMART, member_id);
    let vault = squad.vault(0);

    let tx = site_tx(vault);
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let chain = [squad_account(
        &squad,
        1,
        0,
        4,
        &[(member.pubkey(), PERMISSION_ALL), (pk(8), PERMISSION_ALL)],
    )];
    let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
    assert_eq!(
        adapted.steps,
        vec![AdaptedStep::SquadsSync {
            wallet: "Treasury".into(),
            vault,
            member: member.pubkey(),
            threshold: 1,
            signers: vec![member.pubkey()],
        }]
    );

    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    // The hot wallet pays, the member signs, the vault does neither.
    assert_eq!(
        signer_keys(&rebuilt),
        vec![d.payer.pubkey(), member.pubkey()]
    );
    assert!(is_plain_account(&rebuilt, &vault));
    // One instruction, and nothing stored: no transaction or proposal account.
    let native = rebuilt.native_instructions();
    assert_eq!(native.len(), 1);
    let sync = SyncExecution::parse(&native[0]).unwrap();
    assert_eq!(
        (sync.squad, sync.vault, sync.vault_index),
        (squad, vault, 0)
    );
    assert_eq!(sync.signers, vec![member.pubkey()]);
    assert_eq!(sync.instructions, tx.native_instructions());
    for address in [squad.transaction(5), squad.proposal(5)] {
        assert!(!rebuilt
            .accounts
            .iter()
            .any(|account| account.pubkey == address));
    }
    assert_eq!(
        rebuilt.budget.compute_unit_limit,
        Some(DEFAULT_CU_PER_INSTRUCTION + SQUADS_OVERHEAD_CU)
    );

    let both: [&dyn WalletSigner; 2] = [&d.payer, &member];
    let signed = apply_local_signatures(&adapted.wire, &both).unwrap();
    assert!(decode_v1_transaction(&signed).unwrap().fully_signed());
}

/// Issue #64, at once: the member pays and receives. Inside, it only
/// receives.
#[test]
fn a_send_to_the_fee_payer_member_runs_at_once() {
    let mut d = desk();
    let (member, member_id) = add_hot(&mut d, "Member");
    let squad = d.add_squad(SMART, member_id);
    d.wallets
        .last_mut()
        .unwrap()
        .kind
        .set_fee_payer(Some(member_id));
    let vault = squad.vault(0);
    let member_pk = member.pubkey();
    let wire = compile_v1(
        member_pk,
        vec![transfer(vault, member_pk, 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    let tx = decode_v1_transaction(&wire).unwrap();
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let chain = [squad_account(
        &squad,
        1,
        0,
        0,
        &[(member_pk, PERMISSION_ALL)],
    )];
    let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
    assert!(matches!(
        &adapted.steps[..],
        [AdaptedStep::SquadsSync { signers, .. }] if *signers == vec![member_pk]
    ));
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(signer_keys(&rebuilt), vec![member_pk]);
    let sync = SyncExecution::parse(&rebuilt.native_instructions()[0]).unwrap();
    let to = &sync.instructions[0].accounts[1];
    assert!(to.pubkey == member_pk && !to.is_signer, "{to:?}");
    let keys: [&dyn WalletSigner; 1] = [&member];
    let signed = apply_local_signatures(&adapted.wire, &keys).unwrap();
    assert!(decode_v1_transaction(&signed).unwrap().fully_signed());
}

#[test]
fn a_programmatic_member_runs_it_at_once_through_its_authorization() {
    let mut d = desk();
    let squad = d.add_squad(SMART, d.signer_id);
    let vault = squad.vault(0);
    let tx = site_tx(vault);
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let chain = [
        squad_account(&squad, 1, 0, 0, &[(d.address, PERMISSION_ALL)]),
        d.nonce_account(),
    ];
    let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
    assert!(matches!(
        &adapted.steps[..],
        [AdaptedStep::SquadsSync { signers, .. }, AdaptedStep::Programmatic { .. }]
            if *signers == vec![d.address]
    ));

    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(signer_keys(&rebuilt), vec![d.payer.pubkey()]);
    assert!(is_plain_account(&rebuilt, &vault));
    assert!(is_plain_account(&rebuilt, &d.address));
    // Submit carries Execute, which carries the one execution.
    let submission = Submission::parse(&rebuilt.instructions[0].data).unwrap();
    let run = Execution::parse(&submission.execute().unwrap())
        .unwrap()
        .instructions();
    assert_eq!(run.len(), 1);
    let sync = SyncExecution::parse(&run[0]).unwrap();
    assert_eq!(sync.instructions, tx.native_instructions());
    assert!(run[0]
        .accounts
        .iter()
        .any(|meta| meta.pubkey == d.address && meta.is_signer));

    let signed = d.sign_both(&adapted.wire);
    assert!(decode_v1_transaction(&signed.wire).unwrap().fully_signed());
}

/// Where a proposal would have to hand execute to the other member, to keep
/// the programmatic member's instructions together, one execution has no
/// order to keep.
#[test]
fn a_programmatic_creator_and_a_hot_member_run_it_at_once() {
    let mut d = desk();
    let (cosigner, cosigner_id) = add_hot(&mut d, "Cosigner");
    let signer_id = d.signer_id;
    let squad = add_pair(&mut d, SMART, signer_id, cosigner_id);
    let tx = site_tx(squad.vault(0));
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let members = [
        (d.address, PERMISSION_ALL),
        (cosigner.pubkey(), PERMISSION_ALL),
    ];
    let chain = [squad_account(&squad, 2, 0, 0, &members), d.nonce_account()];
    let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
    assert!(
        matches!(
            &adapted.steps[..],
            [AdaptedStep::SquadsSync { signers, .. }, AdaptedStep::Programmatic { .. }]
                if *signers == vec![d.address, cosigner.pubkey()]
        ),
        "{:?}",
        adapted.steps
    );
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert!(signer_keys(&rebuilt).contains(&cosigner.pubkey()));
    let submission = Submission::parse(&rebuilt.instructions[0].data).unwrap();
    let run = Execution::parse(&submission.execute().unwrap())
        .unwrap()
        .instructions();
    let [execute] = &run[..] else {
        panic!("{run:?}");
    };
    let sync = SyncExecution::parse(execute).unwrap();
    assert_eq!(sync.signers, vec![d.address, cosigner.pubkey()]);
}

/// The program adds up what the members that sign may do: one that may only
/// create and vote, and one that may only vote and execute, are enough
/// between them.
#[test]
fn permissions_add_up_across_the_members_that_sign() {
    let mut d = desk();
    let (acting, acting_id) = add_hot(&mut d, "Acting");
    let (other, other_id) = add_hot(&mut d, "Other");
    let squad = add_pair(&mut d, SMART, acting_id, other_id);
    let tx = site_tx(squad.vault(0));
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let members = [
        (acting.pubkey(), PERMISSION_INITIATE | PERMISSION_VOTE),
        (other.pubkey(), PERMISSION_VOTE | PERMISSION_EXECUTE),
    ];
    let chain = [squad_account(&squad, 2, 0, 0, &members)];
    let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
    assert!(
        matches!(
            &adapted.steps[..],
            [AdaptedStep::SquadsSync { signers, .. }]
                if *signers == vec![acting.pubkey(), other.pubkey()]
        ),
        "{:?}",
        adapted.steps
    );
    // The payer first, then both members, in whatever order the message
    // compiles them.
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    let mut keys = signer_keys(&rebuilt);
    assert_eq!(keys.remove(0), d.payer.pubkey());
    keys.sort();
    let mut members = vec![acting.pubkey(), other.pubkey()];
    members.sort();
    assert_eq!(keys, members);
}

/// Without enough votes, a time lock, or anyone that may execute, the
/// program would refuse: the proposal is built as it always was.
#[test]
fn what_cannot_run_at_once_is_a_proposal() {
    let mut d = desk();
    let (acting, acting_id) = add_hot(&mut d, "Acting");
    let (other, other_id) = add_hot(&mut d, "Other");
    let squad = add_pair(&mut d, SMART, acting_id, other_id);
    let tx = site_tx(squad.vault(0));
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let all = [
        (acting.pubkey(), PERMISSION_ALL),
        (other.pubkey(), PERMISSION_ALL),
    ];
    let vote_only = [
        (acting.pubkey(), PERMISSION_INITIATE | PERMISSION_VOTE),
        (other.pubkey(), PERMISSION_VOTE),
    ];
    let cases = [
        // threshold, time lock, members, why not at once
        (3, 0, &all, HeldBack::Votes { missing: 1 }),
        (2, 60, &all, HeldBack::TimeLock),
        (2, 0, &vote_only, HeldBack::NoExecutor),
    ];
    for (threshold, time_lock, members, held) in cases {
        let chain = [squad_account(&squad, threshold, time_lock, 0, members)];
        let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
        assert!(
            matches!(
                &adapted.steps[..],
                [AdaptedStep::Squads { executor: None, held_back: Some(found), .. }]
                    if *found == held
            ),
            "{threshold} {time_lock}: {:?}",
            adapted.steps
        );
    }
}

/// v4 has no such instruction.
#[test]
fn a_v4_multisig_never_runs_it_at_once() {
    let mut d = desk();
    let (member, member_id) = add_hot(&mut d, "Member");
    let squad = d.add_squad(SquadsProgram::V4, member_id);
    let tx = site_tx(squad.vault(0));
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let chain = [squad_account(
        &squad,
        1,
        0,
        0,
        &[(member.pubkey(), PERMISSION_ALL)],
    )];
    let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
    assert!(matches!(
        &adapted.steps[..],
        [AdaptedStep::Squads {
            executor: Some(_),
            held_back: None,
            ..
        }]
    ));
}
