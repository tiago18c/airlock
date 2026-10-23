//! Replacing a fee payer whose key is elsewhere.

use super::*;

/// A site's transaction that `sponsor` pays for and `from` spends in.
fn sponsored(sponsor: Pubkey, from: Pubkey) -> DecodedTx {
    let wire = compile_v1(
        sponsor,
        vec![transfer(from, pk(2), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    decode_v1_transaction(&wire).unwrap()
}

#[test]
fn a_payer_that_only_pays_can_be_replaced_by_a_hot_wallet() {
    let d = desk();
    let sponsor = KeypairSigner::new_random();
    let wire = compile_v1(
        sponsor.pubkey(),
        vec![transfer(d.authority.pubkey(), pk(2), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    // The sponsor signed before handing it over.
    let wire = apply_local_signatures(&wire, &[&sponsor as &dyn WalletSigner]).unwrap();
    let tx = decode_v1_transaction(&wire).unwrap();
    assert!(tx.signers[0].valid);

    // Nobody is acting, so nobody is offered.
    assert_eq!(plan_payer_swap(&tx, &d.wallets, None), None);
    // A programmatic signer cannot pay.
    assert_eq!(plan_payer_swap(&tx, &d.wallets, Some(d.signer_id)), None);
    let swap = plan_payer_swap(&tx, &d.wallets, Some(d.payer_id)).unwrap();
    assert_eq!(
        swap,
        PayerSwap {
            from: sponsor.pubkey(),
            to: d.payer.pubkey(),
        }
    );

    let swapped = decode_v1_transaction(&swap_fee_payer(&tx, &swap).unwrap()).unwrap();
    // The sponsor and its signature are gone. Nothing else changed.
    assert_eq!(
        signer_keys(&swapped),
        vec![d.payer.pubkey(), d.authority.pubkey()]
    );
    assert!(!swapped
        .accounts
        .iter()
        .any(|account| account.pubkey == sponsor.pubkey()));
    assert!(swapped.signers.iter().all(|slot| !slot.filled));
    assert_eq!(swapped.native_instructions(), tx.native_instructions());
    assert_eq!((swapped.lifetime, swapped.budget), (tx.lifetime, tx.budget));
    // Both remaining signers are here.
    let signed = sign_locally(&swapped.wire, &d.keys()).unwrap();
    assert!(decode_v1_transaction(&signed.wire).unwrap().fully_signed());
}

#[test]
fn a_hot_wallet_already_in_the_transaction_takes_the_payers_place() {
    let d = desk();
    let tx = sponsored(pk(60), d.payer.pubkey());
    let swap = plan_payer_swap(&tx, &d.wallets, Some(d.payer_id)).unwrap();
    let swapped = decode_v1_transaction(&swap_fee_payer(&tx, &swap).unwrap()).unwrap();
    assert_eq!(signer_keys(&swapped), vec![d.payer.pubkey()]);
    assert_eq!(swapped.native_instructions().len(), 1);
}

#[test]
fn a_payer_that_is_needed_or_is_ours_is_not_replaced() {
    let mut d = desk();
    let acting = Some(d.payer_id);
    // The payer also spends, so its signature is needed either way.
    let wire = compile_v1(
        pk(60),
        vec![transfer(pk(60), pk(2), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    let spends = decode_v1_transaction(&wire).unwrap();
    assert_eq!(plan_payer_swap(&spends, &d.wallets, acting), None);
    // Named without signing for it, it is still flagged as a signer there.
    let wire = compile_v1(
        pk(60),
        vec![transfer(d.payer.pubkey(), pk(60), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    let receives = decode_v1_transaction(&wire).unwrap();
    assert_eq!(plan_payer_swap(&receives, &d.wallets, acting), None);

    // A wallet of this device pays: a key signs, an address is rebuilt.
    let own = site_tx(d.payer.pubkey());
    assert_eq!(plan_payer_swap(&own, &d.wallets, acting), None);
    let own = site_tx(d.address);
    assert_eq!(plan_payer_swap(&own, &d.wallets, acting), None);

    // A vault's proposal never keeps the site's payer in the first place.
    let squad = d.add_squad(SquadsProgram::V4, d.payer_id);
    let vault = sponsored(pk(60), squad.vault(0));
    assert_eq!(plan_payer_swap(&vault, &d.wallets, acting), None);
}

#[test]
fn a_sponsored_request_for_a_programmatic_signer_can_be_paid_by_its_hot_wallet() {
    let d = desk();
    let tx = sponsored(pk(60), d.address);
    let local = [d.payer.pubkey(), d.authority.pubkey()];

    // As the site built it, the sponsor still has to sign after the rebuild.
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let kept = plan.build(&tx, &[d.nonce_account()], blockhash(4)).unwrap();
    let kept = decode_v1_transaction(&kept.wire).unwrap();
    assert_eq!(signing_status(&kept, &local).missing_elsewhere(), 1);

    // The signer's own fee payer is offered, whoever the app is acting as.
    let swap = plan_payer_swap(&tx, &d.wallets, None).unwrap();
    assert_eq!(swap.to, d.payer.pubkey());
    let swapped = decode_v1_transaction(&swap_fee_payer(&tx, &swap).unwrap()).unwrap();
    // This is what gets simulated: the address spends, the hot wallet pays.
    assert_eq!(signer_keys(&swapped), vec![d.payer.pubkey(), d.address]);

    let plan = plan_adaptation(&swapped, &d.wallets, None)
        .unwrap()
        .unwrap();
    assert_eq!(plan.fee_payer, d.payer.pubkey());
    let rebuilt = plan
        .build(&swapped, &[d.nonce_account()], blockhash(4))
        .unwrap();
    let signed = sign_locally(&rebuilt.wire, &d.keys()).unwrap();
    let signed = decode_v1_transaction(&signed.wire).unwrap();
    assert_eq!(signer_keys(&signed), vec![d.payer.pubkey()]);
    assert!(signing_status(&signed, &local).complete());
}

#[test]
fn a_filled_advance_refuses_a_swap_to_a_payer_an_instruction_names() {
    use crate::wallets::relay::{
        authority_slots, sign_vector_authorizations, wrap_for_vector, AuthContext, VectorContext,
        VectorWrap,
    };
    use crate::wallets::vector::{address, SchemeId, VectorKey, VectorSigner, PROGRAM_ID};
    use solana_instruction::{AccountMeta, Instruction};

    let d = desk();
    let key = KeypairSigner::new_random();
    let account = address(&PROGRAM_ID, SchemeId::Ed25519, key.pubkey().as_ref()).unwrap();
    let sponsor = pk(60);
    // The site's payer only pays. A top-level instruction names the hot
    // wallet that would take its place.
    let peek = Instruction {
        program_id: pk(40),
        accounts: vec![AccountMeta::new(d.payer.pubkey(), false)],
        data: vec![],
    };
    let site = decode_v1_transaction(
        &compile_v1(
            sponsor,
            vec![transfer(account, pk(2), 5), peek],
            None,
            Some(blockhash(1)),
        )
        .unwrap()
        .wire,
    )
    .unwrap();
    let wrap = VectorWrap {
        account,
        scheme: SchemeId::Ed25519,
        key: key.pubkey().to_bytes().to_vec(),
        next_key: None,
    };
    let wire = wrap_for_vector(&site, &[wrap], sponsor, blockhash(2)).unwrap();
    let known = [VectorContext {
        account,
        scheme: SchemeId::Ed25519,
        nonce: Some([1; 32]),
        key: Some(key.pubkey().to_bytes().to_vec()),
        leaf_floor: 0,
    }];
    let keys = [VectorKey {
        leaf: None,
        salt: None,
        next_key: None,
        account,
        signer: VectorSigner::Ed25519(KeypairSigner::from_seed(&key.seed_bytes()).unwrap()),
    }];

    // Before the Advance is signed, the hot wallet is offered.
    let open = decode_v1_transaction(&wire).unwrap();
    let swap = plan_payer_swap(&open, &d.wallets, Some(d.payer_id)).unwrap();
    assert_eq!(swap.to, d.payer.pubkey());

    let signed = sign_vector_authorizations(&wire, &keys, &AuthContext { vector: &known }).unwrap();
    let tx = decode_v1_transaction(&signed.wire).unwrap();
    assert!(authority_slots(&tx).iter().any(|slot| slot.signed));
    // Once it is signed, putting that hot wallet in as fee payer would change
    // the flags the account covered.
    assert_eq!(plan_payer_swap(&tx, &d.wallets, Some(d.payer_id)), None);
}
