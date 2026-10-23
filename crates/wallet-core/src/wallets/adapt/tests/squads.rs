//! Transactions that name a Squads vault.

use super::*;

#[test]
fn a_vault_transaction_becomes_a_proposal_its_member_creates_approves_and_executes() {
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        let mut d = desk();
        let member = KeypairSigner::new_random();
        let member_wallet = WalletRecord::from_keypair_seed("Member", member.seed_bytes());
        let member_id = member_wallet.id;
        d.wallets.push(member_wallet);
        let squad = d.add_squad(program, member_id);
        let vault = squad.vault(0);

        let tx = site_tx(vault);
        let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
        assert_eq!(plan.accounts_to_read(), vec![squad.address]);
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
            vec![AdaptedStep::Squads {
                wallet: "Treasury".into(),
                vault,
                member: member.pubkey(),
                index: 5,
                threshold: 1,
                voters: vec![member.pubkey()],
                executor: Some(member.pubkey()),
                held_back: None,
            }],
            "{program:?}"
        );

        let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
        // The hot wallet pays, the member signs, the vault does neither.
        assert_eq!(
            signer_keys(&rebuilt),
            vec![d.payer.pubkey(), member.pubkey()]
        );
        assert!(is_plain_account(&rebuilt, &vault));
        let native = rebuilt.native_instructions();
        assert_eq!(native.len(), 4);
        assert!(native
            .iter()
            .all(|instruction| instruction.program_id == program.program_id()));
        // The stored transaction is the site's, for the next free number.
        let stored = StoredTransaction::parse(&native[0]).unwrap();
        assert_eq!((stored.squad, stored.vault), (squad, vault));
        assert_eq!(stored.instructions, tx.native_instructions());
        assert_eq!(native[0].accounts[1].pubkey, squad.transaction(5));
        assert_eq!(native[1].accounts[1].pubkey, squad.proposal(5));
        // Room to run what the site asked for, on top of the proposal.
        assert_eq!(
            rebuilt.budget.compute_unit_limit,
            Some(DEFAULT_CU_PER_INSTRUCTION + SQUADS_OVERHEAD_CU)
        );

        let both: [&dyn WalletSigner; 2] = [&d.payer, &member];
        let signed = apply_local_signatures(&adapted.wire, &both).unwrap();
        assert!(decode_v1_transaction(&signed).unwrap().fully_signed());
    }
}

/// Issue #64: a vault send the approving member both pays for and receives
/// must still wrap. `native_instructions` marks the fee payer as a signer
/// wherever it appears; the wrap must not treat that as an extra signer.
#[test]
fn a_send_to_the_fee_payer_member_still_wraps_as_a_proposal() {
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        let mut d = desk();
        let member = KeypairSigner::new_random();
        let member_wallet = WalletRecord::from_keypair_seed("Member", member.seed_bytes());
        let member_id = member_wallet.id;
        d.wallets.push(member_wallet);
        let squad = d.add_squad(program, member_id);
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
        assert!(
            tx.native_instructions()[0]
                .accounts
                .iter()
                .any(|meta| meta.pubkey == member_pk && meta.is_signer),
            "{program:?}: fee payer named as recipient is reconstructed as a signer"
        );
        let plan = plan_adaptation(&tx, &d.wallets, None)
            .unwrap_or_else(|err| panic!("{program:?}: plan: {err}"))
            .unwrap_or_else(|| panic!("{program:?}: expected a Squads plan"));
        let chain = [squad_account(
            &squad,
            1,
            0,
            0,
            &[(member_pk, PERMISSION_ALL)],
        )];
        let adapted = plan
            .build(&tx, &chain, blockhash(4))
            .unwrap_or_else(|err| panic!("{program:?}: build: {err}"));
        assert_eq!(
            adapted.steps,
            vec![AdaptedStep::Squads {
                wallet: "Treasury".into(),
                vault,
                member: member_pk,
                index: 1,
                threshold: 1,
                voters: vec![member_pk],
                executor: Some(member_pk),
                held_back: None,
            }],
            "{program:?}"
        );
        let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
        assert_eq!(signer_keys(&rebuilt), vec![member_pk], "{program:?}");
        assert!(is_plain_account(&rebuilt, &vault), "{program:?}");
        // The stored transfer names the member as recipient only, not signer.
        let stored = StoredTransaction::parse(&rebuilt.native_instructions()[0]).unwrap();
        assert_eq!(stored.instructions.len(), 1, "{program:?}");
        assert!(
            stored.instructions[0]
                .accounts
                .iter()
                .any(|meta| meta.pubkey == member_pk && !meta.is_signer),
            "{program:?}: {:?}",
            stored.instructions[0].accounts
        );
        let keys: [&dyn WalletSigner; 1] = [&member];
        let signed = apply_local_signatures(&adapted.wire, &keys).unwrap();
        assert!(
            decode_v1_transaction(&signed).unwrap().fully_signed(),
            "{program:?}"
        );
    }
}

/// Build compiles with a hot wallet paying and the vault signing only
/// its own instruction. That payer is kept, whatever the multisig has set.
#[test]
fn a_vault_transaction_built_here_keeps_the_hot_wallet_that_pays_it() {
    let mut d = desk();
    let member = KeypairSigner::new_random();
    let member_wallet = WalletRecord::from_keypair_seed("Member", member.seed_bytes());
    let member_id = member_wallet.id;
    d.wallets.push(member_wallet);
    let squad = d.add_squad(SquadsProgram::V4, member_id);
    let vault = squad.vault(0);
    let built = |payer: Pubkey| {
        let wire = compile_v1(
            payer,
            vec![transfer(vault, pk(2), 5)],
            None,
            Some(blockhash(1)),
        )
        .unwrap()
        .wire;
        decode_v1_transaction(&wire).unwrap()
    };

    let tx = built(member.pubkey());
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    assert_eq!(plan.fee_payer, member.pubkey());
    assert_eq!(
        plan.asked(),
        vec![AskedWallet::Squads {
            wallet: "Treasury".into(),
            vault,
            member: member.pubkey(),
        }]
    );
    let chain = [squad_account(
        &squad,
        1,
        0,
        0,
        &[(member.pubkey(), PERMISSION_ALL)],
    )];
    let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(signer_keys(&rebuilt), vec![member.pubkey()]);
    assert!(is_plain_account(&rebuilt, &vault));

    // A payer that is not a wallet here cannot pay a proposal's rent.
    let plan = plan_adaptation(&built(pk(60)), &d.wallets, None)
        .unwrap()
        .unwrap();
    assert_eq!(plan.fee_payer, d.payer.pubkey());

    // An offline wallet here pays too: its signature comes from the device
    // that holds its key.
    let cold = WalletRecord::offline("Cold", pk(61));
    d.wallets.push(cold);
    let plan = plan_adaptation(&built(pk(61)), &d.wallets, None)
        .unwrap()
        .unwrap();
    assert_eq!(plan.fee_payer, pk(61));
}

#[test]
fn a_proposal_is_only_taken_as_far_as_the_member_may() {
    let mut d = desk();
    let member = KeypairSigner::new_random();
    let member_wallet = WalletRecord::from_keypair_seed("Member", member.seed_bytes());
    let member_id = member_wallet.id;
    d.wallets.push(member_wallet);
    let squad = d.add_squad(SquadsProgram::V4, member_id);
    let tx = site_tx(squad.vault(0));
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let build = |threshold: u16, time_lock: u32, permissions: u8| {
        let chain = [squad_account(
            &squad,
            threshold,
            time_lock,
            0,
            &[(member.pubkey(), permissions)],
        )];
        plan.build(&tx, &chain, blockhash(4))
    };
    let taken = |adapted: Adapted| match &adapted.steps[0] {
        AdaptedStep::Squads {
            voters,
            executor,
            held_back,
            ..
        } => (
            !voters.is_empty(),
            executor.is_some(),
            decode_v1_transaction(&adapted.wire)
                .unwrap()
                .instructions
                .len(),
            held_back.clone(),
        ),
        other => panic!("{other:?}"),
    };

    // Two approvals needed: this one, and the proposal waits.
    assert_eq!(
        taken(build(2, 0, PERMISSION_ALL).unwrap()),
        (true, false, 3, Some(HeldBack::Votes { missing: 1 }))
    );
    // A time lock has to pass between approval and execution.
    assert_eq!(
        taken(build(1, 60, PERMISSION_ALL).unwrap()),
        (true, false, 3, Some(HeldBack::TimeLock))
    );
    // May propose and vote, not execute.
    assert_eq!(
        taken(build(1, 0, PERMISSION_INITIATE | PERMISSION_VOTE).unwrap()),
        (true, false, 3, Some(HeldBack::NoExecutor))
    );
    // May only propose.
    assert_eq!(
        taken(build(1, 0, PERMISSION_INITIATE).unwrap()),
        (false, false, 2, Some(HeldBack::Votes { missing: 1 }))
    );
    // Without execution the stored instructions do not run here.
    let waiting = build(2, 0, PERMISSION_ALL).unwrap();
    assert_eq!(
        decode_v1_transaction(&waiting.wire)
            .unwrap()
            .budget
            .compute_unit_limit,
        Some(SQUADS_OVERHEAD_CU)
    );
    assert_eq!(
        build(1, 0, PERMISSION_VOTE | PERMISSION_EXECUTE),
        Err(AdaptError::CannotPropose {
            squad: "Treasury".into(),
            member: member.pubkey(),
        })
    );
}

#[test]
fn a_proposal_too_large_to_execute_at_once_is_left_approved() {
    let mut d = desk();
    let member = KeypairSigner::new_random();
    let member_wallet = WalletRecord::from_keypair_seed("Member", member.seed_bytes());
    let member_id = member_wallet.id;
    d.wallets.push(member_wallet);
    let squad = d.add_squad(SquadsProgram::V4, member_id);
    let vault = squad.vault(0);
    // Enough recipients that naming each once more, for the execution,
    // passes the size of a transaction.
    let payments: Vec<Instruction> = (0..50u8).map(|n| transfer(vault, pk(100 + n), 1)).collect();
    let wire = compile_v1(vault, payments, None, Some(blockhash(1)))
        .unwrap()
        .wire;
    let tx = decode_v1_transaction(&wire).unwrap();
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
        adapted.steps[0],
        AdaptedStep::Squads {
            ref voters,
            executor: None,
            held_back: Some(HeldBack::Size { .. }),
            ..
        } if voters.len() == 1
    ));
}

#[test]
fn a_multisig_whose_member_is_a_programmatic_signer_takes_both_steps() {
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        let mut d = desk();
        let squad = d.add_squad(program, d.signer_id);
        let vault = squad.vault(0);
        let tx = site_tx(vault);
        let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
        assert_eq!(plan.accounts_to_read(), vec![squad.address, d.nonce]);
        let chain = [
            squad_account(&squad, 1, 0, 0, &[(d.address, PERMISSION_ALL)]),
            d.nonce_account(),
        ];
        let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
        assert_eq!(adapted.steps.len(), 2, "{program:?}");
        assert!(matches!(
            adapted.steps[0],
            AdaptedStep::Squads {
                member,
                executor: Some(executor),
                ..
            } if member == d.address && executor == d.address
        ));
        assert!(matches!(adapted.steps[1], AdaptedStep::Programmatic { .. }));

        // One signer, the hot wallet. Neither address without a key signs.
        let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
        assert_eq!(signer_keys(&rebuilt), vec![d.payer.pubkey()]);
        assert!(is_plain_account(&rebuilt, &vault));
        assert!(is_plain_account(&rebuilt, &d.address));
        // Submit carries Execute, which carries the four Squads steps,
        // the first of which stores the site's instructions.
        let submission = Submission::parse(&rebuilt.instructions[0].data).unwrap();
        let run = Execution::parse(&submission.execute().unwrap())
            .unwrap()
            .instructions();
        assert_eq!(run.len(), 4);
        let stored = StoredTransaction::parse(&run[0]).unwrap();
        assert_eq!(stored.instructions, tx.native_instructions());
        // Inside, the member signs as the programmatic signer's address.
        assert!(run[0]
            .accounts
            .iter()
            .any(|meta| meta.pubkey == d.address && meta.is_signer));

        let signed = sign_locally(&adapted.wire, &d.keys()).unwrap();
        assert_eq!((signed.authorizations, signed.signatures), (1, 1));
        assert!(decode_v1_transaction(&signed.wire).unwrap().fully_signed());
    }
}

/// A 2-of-2 multisig on `d` acted on by `acting`, with `with` approving
/// together with it.
fn add_pair(d: &mut Desk, program: SquadsProgram, acting: WalletId, with: WalletId) -> Squad {
    let squad = d.add_squad(program, acting);
    d.wallets
        .last_mut()
        .unwrap()
        .kind
        .set_approves_with(vec![with]);
    squad
}

/// A hot key of this device, added to `d`.
fn add_hot(d: &mut Desk, label: &str) -> (KeypairSigner, WalletId) {
    let key = KeypairSigner::new_random();
    let wallet = WalletRecord::from_keypair_seed(label, key.seed_bytes());
    let id = wallet.id;
    d.wallets.push(wallet);
    (key, id)
}

/// A programmatic member that creates and would execute cannot have another
/// member's vote between its instructions: they share one `Submit`, which
/// would run its execute before that vote. Another member executes.
#[test]
fn a_programmatic_creator_leaves_execute_to_a_member_after_its_authorization() {
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        let mut d = desk();
        let (cosigner, cosigner_id) = add_hot(&mut d, "Cosigner");
        let signer_id = d.signer_id;
        let squad = add_pair(&mut d, program, signer_id, cosigner_id);
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
                &adapted.steps[0],
                AdaptedStep::Squads { voters, executor: Some(executor), held_back: None, .. }
                    if *voters == vec![d.address, cosigner.pubkey()]
                        && *executor == cosigner.pubkey()
            ),
            "{program:?}: {:?}",
            adapted.steps[0]
        );

        // The `Submit` runs create, propose and the programmatic vote. The
        // other vote, then execute, follow it at the top level.
        let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
        assert_eq!(rebuilt.instructions.len(), 3, "{program:?}");
        let submission = Submission::parse(&rebuilt.instructions[0].data).unwrap();
        let run = Execution::parse(&submission.execute().unwrap())
            .unwrap()
            .instructions();
        assert_eq!(run.len(), 3, "{program:?}");
        // A message flags an account once for all its instructions, so
        // compare what each does and who signs it.
        let shown = |instruction: &Instruction| {
            let signers: Vec<Pubkey> = instruction
                .accounts
                .iter()
                .filter(|meta| meta.is_signer)
                .map(|meta| meta.pubkey)
                .collect();
            (instruction.program_id, instruction.data.clone(), signers)
        };
        let top = rebuilt.native_instructions();
        let approve = squad.approve(1, &cosigner.pubkey());
        let execute = squad.execute(1, 0, &tx.native_instructions(), &cosigner.pubkey());
        assert_eq!(shown(&top[1]), shown(&approve), "{program:?}");
        assert_eq!(shown(&top[2]), shown(&execute), "{program:?}");
    }
}

/// A co-voter that could not sign here is left out, and the acting member's
/// part still goes ahead.
#[test]
fn a_co_voter_that_cannot_sign_is_left_out() {
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        // A programmatic signer without a nonce of its own.
        let mut d = desk();
        let (member, member_id) = add_hot(&mut d, "Member");
        let other = KeypairSigner::new_random();
        let mut no_nonce = WalletRecord::programmatic("No nonce", other.pubkey(), None);
        no_nonce.kind.set_fee_payer(Some(d.payer_id));
        let no_nonce_address = no_nonce.address().unwrap();
        let no_nonce_id = no_nonce.id;
        d.wallets.push(no_nonce);
        // An uninitialized Vector wallet.
        let key = KeypairSigner::new_random().pubkey().to_bytes().to_vec();
        let vector = WalletRecord::vector(
            "Quantum",
            crate::wallets::vector::SchemeId::Ed25519,
            crate::wallets::vector::PROGRAM_ID,
            key,
            crate::wallets::record::Authority::Outside,
        );
        let vector_address = vector.address().unwrap();
        let vector_id = vector.id;
        d.wallets.push(vector);

        for (co_voter, address) in [(no_nonce_id, no_nonce_address), (vector_id, vector_address)] {
            let squad = add_pair(&mut d, program, member_id, co_voter);
            let tx = site_tx(squad.vault(0));
            let members = [(member.pubkey(), PERMISSION_ALL), (address, PERMISSION_ALL)];
            let chain = [squad_account(&squad, 2, 0, 0, &members)];
            for offered in [true, false] {
                let plan = plan_adaptation(&tx, &d.wallets, None)
                    .unwrap()
                    .unwrap()
                    .unless_vector_off(offered)
                    .unwrap_or_else(|err| panic!("{program:?} {address}: {err}"));
                let adapted = plan
                    .build(&tx, &chain, blockhash(4))
                    .unwrap_or_else(|err| panic!("{program:?} {address}: {err}"));
                assert_eq!(
                    adapted.steps,
                    vec![AdaptedStep::Squads {
                        wallet: "Treasury".into(),
                        vault: squad.vault(0),
                        member: member.pubkey(),
                        index: 1,
                        threshold: 2,
                        voters: vec![member.pubkey()],
                        executor: None,
                        held_back: Some(HeldBack::Votes { missing: 1 }),
                    }],
                    "{program:?} {address}"
                );
            }
            d.wallets.pop();
        }
    }
}
