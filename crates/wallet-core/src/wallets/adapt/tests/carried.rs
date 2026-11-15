//! A vault transaction that creates an account at one of the proposal's
//! ephemeral signers, which the program signs for when it runs.

use super::*;

/// A transfer from the vault and a new account at `new`, which signs.
fn creating(vault: Pubkey, new: Pubkey) -> DecodedTx {
    let create = solana_system_interface::instruction::create_account(
        &vault,
        &new,
        1_000_000,
        0,
        &crate::ids::SYSTEM_PROGRAM_ID,
    );
    let wire = compile_v1(vault, vec![create], None, Some(blockhash(1)))
        .unwrap()
        .wire;
    decode_v1_transaction(&wire).unwrap()
}

/// A multisig of one member here, which may do everything, at
/// `transaction_index` on chain.
fn squad_desk(program: SquadsProgram) -> (Desk, KeypairSigner, Squad) {
    let mut d = desk();
    let member = KeypairSigner::new_random();
    let member_wallet = WalletRecord::from_keypair_seed("Member", member.seed_bytes());
    let member_id = member_wallet.id;
    d.wallets.push(member_wallet);
    let squad = d.add_squad(program, member_id);
    (d, member, squad)
}

#[test]
fn a_new_account_at_an_ephemeral_signer_is_stored_and_left_to_the_program() {
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        let (d, member, squad) = squad_desk(program);
        let vault = squad.vault(0);
        // The chain is at 4, so the proposal is 5.
        let new = squad.ephemeral_signer(5, 0);
        let tx = creating(vault, new);

        // Without saying it is carried, it is another signer.
        assert!(matches!(
            plan_adaptation(&tx, &d.wallets, None),
            Err(AdaptError::OtherSigner { signer, .. }) if signer == new
        ));
        let plan = plan_adaptation_carrying(&tx, &d.wallets, None, &[], &[new])
            .unwrap()
            .unwrap();
        let chain = [squad_account(
            &squad,
            1,
            0,
            4,
            &[(member.pubkey(), PERMISSION_ALL)],
        )];
        let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
        // A smart account could run it at once, and does not: the address
        // is derived from the stored transaction.
        assert!(
            matches!(
                adapted.steps[0],
                AdaptedStep::Squads {
                    index: 5,
                    executor: Some(_),
                    held_back: None,
                    ..
                }
            ),
            "{program:?}: {:?}",
            adapted.steps
        );
        let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
        // The new account signs nowhere at the top: no key exists for it.
        assert_eq!(
            signer_keys(&rebuilt),
            vec![d.payer.pubkey(), member.pubkey()]
        );
        assert!(is_plain_account(&rebuilt, &new));
        let native = rebuilt.native_instructions();
        let stored = StoredTransaction::parse(&native[0]).unwrap();
        assert_eq!(stored.instructions, tx.native_instructions());
        // The stored transaction declares one ephemeral signer.
        assert_eq!(stored.ephemeral_signers, 1, "{program:?}");
        // The execution names it, not as a signer.
        let execute = native.last().unwrap();
        let named = execute
            .accounts
            .iter()
            .find(|meta| meta.pubkey == new)
            .unwrap();
        assert!(!named.is_signer && named.is_writable, "{program:?}");

        let both: [&dyn WalletSigner; 2] = [&d.payer, &member];
        let signed = apply_local_signatures(&adapted.wire, &both).unwrap();
        assert!(decode_v1_transaction(&signed).unwrap().fully_signed());
    }
}

#[test]
fn a_proposal_that_waits_still_carries_the_new_account() {
    let (d, member, squad) = squad_desk(SquadsProgram::V4);
    let vault = squad.vault(0);
    let new = squad.ephemeral_signer(1, 0);
    let tx = creating(vault, new);
    let plan = plan_adaptation_carrying(&tx, &d.wallets, None, &[], &[new])
        .unwrap()
        .unwrap();
    // Two approvals needed, one here.
    let chain = [squad_account(
        &squad,
        2,
        0,
        0,
        &[(member.pubkey(), PERMISSION_ALL), (pk(8), PERMISSION_ALL)],
    )];
    let adapted = plan.build(&tx, &chain, blockhash(4)).unwrap();
    assert!(matches!(
        &adapted.steps[0],
        AdaptedStep::Squads {
            index: 1,
            executor: None,
            held_back: Some(HeldBack::Votes { missing: 1 }),
            ..
        }
    ));
}

#[test]
fn addresses_derived_for_another_proposal_number_are_refused() {
    let (d, member, squad) = squad_desk(SquadsProgram::V4);
    let vault = squad.vault(0);
    // Derived for proposal 5, and another proposal was stored since.
    let new = squad.ephemeral_signer(5, 0);
    let tx = creating(vault, new);
    let plan = plan_adaptation_carrying(&tx, &d.wallets, None, &[], &[new])
        .unwrap()
        .unwrap();
    let chain = [squad_account(
        &squad,
        1,
        0,
        5,
        &[(member.pubkey(), PERMISSION_ALL)],
    )];
    assert_eq!(
        plan.build(&tx, &chain, blockhash(4)),
        Err(AdaptError::CarriedMoved {
            squad: "Treasury".into(),
            derived_for: "5".into(),
            index: 6,
        })
    );
    // One of two that is not the proposal's second.
    let second = squad.ephemeral_signer(6, 2);
    let two = {
        let create = |to: Pubkey| {
            solana_system_interface::instruction::create_account(
                &vault,
                &to,
                1,
                0,
                &crate::ids::SYSTEM_PROGRAM_ID,
            )
        };
        let wire = compile_v1(
            vault,
            vec![create(squad.ephemeral_signer(6, 0)), create(second)],
            None,
            Some(blockhash(1)),
        )
        .unwrap()
        .wire;
        decode_v1_transaction(&wire).unwrap()
    };
    let carried = [squad.ephemeral_signer(6, 0), second];
    let plan = plan_adaptation_carrying(&two, &d.wallets, None, &[], &carried)
        .unwrap()
        .unwrap();
    assert!(matches!(
        plan.build(&two, &chain, blockhash(4)),
        Err(AdaptError::CarriedMoved { index: 6, .. })
    ));
}
