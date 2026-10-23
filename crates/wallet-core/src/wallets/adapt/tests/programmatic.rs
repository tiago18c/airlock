//! Transactions that name a programmatic signer.

use super::*;

#[test]
fn a_transaction_of_ordinary_keys_is_left_alone() {
    let d = desk();
    let tx = site_tx(d.payer.pubkey());
    assert_eq!(plan_adaptation(&tx, &d.wallets, None), Ok(None));
    // An address nobody here knows is somebody else's to sign for.
    assert_eq!(
        plan_adaptation(&site_tx(pk(77)), &d.wallets, None),
        Ok(None)
    );
}

#[test]
fn a_programmatic_signer_that_was_made_to_pay_gets_its_hot_wallet() {
    let d = desk();
    let tx = site_tx(d.address);
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    assert_eq!(plan.fee_payer, d.payer.pubkey());
    assert_eq!(plan.accounts_to_read(), vec![d.nonce]);

    let adapted = plan.build(&tx, &[d.nonce_account()], blockhash(4)).unwrap();
    assert_eq!(
        adapted.steps,
        vec![AdaptedStep::Programmatic {
            wallet: "Signer".into(),
            authority: d.authority.pubkey(),
            address: d.address,
        }]
    );
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    // The hot wallet is the only signer. The address is still there, as
    // an account the instructions inside use.
    assert_eq!(signer_keys(&rebuilt), vec![d.payer.pubkey()]);
    assert!(is_plain_account(&rebuilt, &d.address));
    assert_eq!(rebuilt.lifetime, blockhash(4));
    assert_eq!(rebuilt.instructions.len(), 1);
    assert_eq!(
        rebuilt.instructions[0].program_id,
        ED25519_SIGNER_PROGRAM_ID
    );
    // What runs is what the site asked for.
    let submission = Submission::parse(&rebuilt.instructions[0].data).unwrap();
    let execution = Execution::parse(&submission.execute().unwrap()).unwrap();
    assert_eq!(execution.instructions(), tx.native_instructions());
    assert_eq!(execution.nonce_value, blockhash(7));

    // This device holds both keys, so one press signs all of it.
    let local = [d.payer.pubkey(), d.authority.pubkey()];
    assert_eq!(signing_status(&rebuilt, &local).missing_elsewhere(), 0);
    let signed = sign_locally(&adapted.wire, &d.keys()).unwrap();
    assert_eq!((signed.authorizations, signed.signatures), (1, 1));
    let signed = decode_v1_transaction(&signed.wire).unwrap();
    assert!(signing_status(&signed, &local).complete());
}

#[test]
fn a_key_the_site_chose_to_pay_keeps_paying() {
    let d = desk();
    let sponsor = pk(60);
    let wire = compile_v1(
        sponsor,
        vec![transfer(d.address, pk(2), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    let tx = decode_v1_transaction(&wire).unwrap();
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    assert_eq!(plan.fee_payer, sponsor);
    let adapted = plan.build(&tx, &[d.nonce_account()], blockhash(4)).unwrap();
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(signer_keys(&rebuilt), vec![sponsor]);
    // The sponsor is not here, so this device cannot finish it alone.
    let local = [d.payer.pubkey(), d.authority.pubkey()];
    assert_eq!(signing_status(&rebuilt, &local).missing_elsewhere(), 1);
}

#[test]
fn the_acting_hot_wallet_pays_when_none_was_chosen() {
    let mut d = desk();
    // An authority kept elsewhere, and no fee payer set.
    let cold = WalletRecord::programmatic("Cold", pk(33), None);
    let address = cold.address().unwrap();
    d.wallets.push(cold);
    let tx = site_tx(address);
    assert_eq!(
        plan_adaptation(&tx, &d.wallets, None),
        Err(AdaptError::NoNonce("Cold".into()))
    );
    if let Some(wallet) = d.wallets.last_mut() {
        wallet.kind.set_executor_nonce(Some(pk(91)));
    }
    assert_eq!(
        plan_adaptation(&tx, &d.wallets, None),
        Err(AdaptError::NoFeePayer("Cold".into()))
    );
    // A programmatic signer cannot be the wallet that pays.
    assert_eq!(
        plan_adaptation(&tx, &d.wallets, Some(d.signer_id)),
        Err(AdaptError::NoFeePayer("Cold".into()))
    );
    let plan = plan_adaptation(&tx, &d.wallets, Some(d.payer_id))
        .unwrap()
        .unwrap();
    assert_eq!(plan.fee_payer, d.payer.pubkey());
}

/// Add a second programmatic signer, ahead of the desk's own in the vault.
/// Its authority key is kept elsewhere.
fn second_signer(d: &mut Desk, nonce: Option<Pubkey>) -> (KeypairSigner, Pubkey) {
    let authority = KeypairSigner::new_random();
    let mut wallet = WalletRecord::programmatic("Second", authority.pubkey(), None);
    wallet.kind.set_fee_payer(Some(d.payer_id));
    wallet.kind.set_executor_nonce(nonce);
    let address = wallet.address().unwrap();
    d.wallets.insert(0, wallet);
    (authority, address)
}

/// `payer` pays, and both `d`'s signer and `other` send five lamports.
fn both_send(d: &Desk, payer: Pubkey, other: Pubkey) -> DecodedTx {
    let wire = compile_v1(
        payer,
        vec![transfer(d.address, pk(2), 5), transfer(other, pk(2), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    decode_v1_transaction(&wire).unwrap()
}

#[test]
fn several_programmatic_signers_share_one_authorization_on_the_nonce_one_of_them_has() {
    let mut d = desk();
    let (second, other) = second_signer(&mut d, None);
    let tx = both_send(&d, d.payer.pubkey(), other);

    // The first in the vault has no nonce. The second lends its own.
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    assert_eq!(plan.accounts_to_read(), vec![d.nonce]);
    let adapted = plan.build(&tx, &[d.nonce_account()], blockhash(4)).unwrap();
    assert_eq!(adapted.steps.len(), 2);

    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(signer_keys(&rebuilt), vec![d.payer.pubkey()]);
    assert_eq!(rebuilt.instructions.len(), 1);
    let slots = authority_slots(&rebuilt);
    assert!(slots.iter().all(|slot| !slot.signed));
    let mut authorities: Vec<Pubkey> = slots.iter().map(|slot| slot.authority.shown_as()).collect();
    authorities.sort();
    let mut expected = vec![d.authority.pubkey(), second.pubkey()];
    expected.sort();
    assert_eq!(authorities, expected);

    // With no nonce on either, the first is the one named.
    for wallet in &mut d.wallets {
        wallet.kind.set_executor_nonce(None);
    }
    assert_eq!(
        plan_adaptation(&tx, &d.wallets, None),
        Err(AdaptError::NoNonce("Second".into()))
    );
}

#[test]
fn a_hot_key_beside_a_programmatic_signer_still_signs_the_transaction() {
    let mut d = desk();
    let member = KeypairSigner::new_random();
    d.wallets.push(WalletRecord::from_keypair_seed(
        "Member",
        member.seed_bytes(),
    ));
    let rebuilt_for = |payer: Pubkey| {
        let tx = both_send(&d, payer, member.pubkey());
        let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
        let adapted = plan.build(&tx, &[d.nonce_account()], blockhash(4)).unwrap();
        decode_v1_transaction(&adapted.wire).unwrap()
    };

    let relayed = rebuilt_for(d.payer.pubkey());
    assert_eq!(
        signer_keys(&relayed),
        vec![d.payer.pubkey(), member.pubkey()]
    );
    assert_eq!(authority_slots(&relayed).len(), 1);

    // A member that also pays signs once.
    let paid_by_member = rebuilt_for(member.pubkey());
    assert_eq!(signer_keys(&paid_by_member), vec![member.pubkey()]);
}

#[test]
fn a_nonce_none_of_the_signers_controls_is_refused_naming_the_wallet_it_came_from() {
    let mut d = desk();
    let (_, other) = second_signer(&mut d, None);
    let tx = both_send(&d, d.payer.pubkey(), other);
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();

    let (nonce, mut account) = d.nonce_account();
    account.data[32..64].copy_from_slice(pk(99).as_ref());
    assert_eq!(
        plan.build(&tx, &[(nonce, account)], blockhash(4)),
        Err(AdaptError::ForeignNonce("Signer".into()))
    );
    assert_eq!(
        plan.build(&tx, &[], blockhash(4)),
        Err(AdaptError::NotOnChain("The nonce account of Signer".into()))
    );
}
