//! Rebuilding for a Vector wallet: what is read, what is refused, and the
//! context signing for it needs.

use super::*;
use crate::review::signing::{signing_status_in, LocalKeys};
use crate::wallets::record::Authority;
use crate::wallets::relay::{advance_next_keys, authority_slots, AuthContext, VectorContext};
use crate::wallets::vector::{seed, SchemeId, PROGRAM_ID};

/// A Vector wallet whose key is on another device, paid for by the desk's
/// hot wallet.
fn add_vector(d: &mut Desk, key: &KeypairSigner) -> (WalletId, Pubkey) {
    let mut wallet = WalletRecord::vector(
        "Quantum",
        SchemeId::Ed25519,
        PROGRAM_ID,
        key.pubkey().to_bytes().to_vec(),
        Authority::Outside,
    );
    wallet.kind.set_fee_payer(Some(d.payer_id));
    let found = (wallet.id, wallet.address().unwrap());
    d.wallets.push(wallet);
    found
}

/// The account of `key` as the program writes it, at `nonce`.
fn vector_account(key: &[u8], scheme: SchemeId, nonce: [u8; 32]) -> RawAccount {
    let mut data = nonce.to_vec();
    data.extend_from_slice(&seed(key));
    data.extend_from_slice(&[scheme.byte(), 255, 1, 0]);
    data.extend_from_slice(key);
    data.resize(scheme.account_len(), 0);
    owned(PROGRAM_ID, data)
}

#[test]
fn a_transaction_for_a_vector_wallet_is_wrapped_with_the_account_as_read() {
    let mut d = desk();
    let key = KeypairSigner::new_random();
    let (_, account) = add_vector(&mut d, &key);
    let tx = site_tx(account);

    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    assert_eq!(
        plan.asked(),
        vec![AskedWallet::Vector {
            wallet: "Quantum".into(),
            scheme: SchemeId::Ed25519,
            account,
        }]
    );
    assert_eq!(plan.accounts_to_read(), vec![account]);
    // The site made the account pay. The hot wallet set for it does.
    assert_eq!(plan.fee_payer, d.payer.pubkey());

    let nonce = [6u8; 32];
    let read = [(
        account,
        vector_account(key.pubkey().as_ref(), SchemeId::Ed25519, nonce),
    )];
    let adapted = plan.build(&tx, &read, blockhash(2)).unwrap();
    assert_eq!(
        adapted.steps,
        vec![AdaptedStep::Vector {
            wallet: "Quantum".into(),
            scheme: SchemeId::Ed25519,
            account,
        }]
    );
    assert_eq!(adapted.context.len(), 1);
    assert_eq!(adapted.context[0].nonce, Some(nonce));
    assert_eq!(
        adapted.context[0].key.as_deref(),
        Some(key.pubkey().as_ref())
    );

    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(signer_keys(&rebuilt), vec![d.payer.pubkey()]);
    assert_eq!(authority_slots(&rebuilt).len(), 1);
    // The hot wallet waits for the account's key, which is elsewhere.
    let ctx = AuthContext {
        vector: &adapted.context,
    };
    let local = LocalKeys {
        keys: &[d.payer.pubkey()],
        vector: &[],
    };
    let status = signing_status_in(&rebuilt, &local, &ctx);
    assert_eq!(status.authorizations_elsewhere(), 1);
    assert!(status.waiting_for_authority);
    assert_eq!(status.signable, 0);
}

#[test]
fn an_account_that_cannot_sign_as_its_wallet_says_is_refused_before_wrapping() {
    let mut d = desk();
    let key = KeypairSigner::new_random();
    let (_, account) = add_vector(&mut d, &key);
    let tx = site_tx(account);
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let build = |read: &[(Pubkey, RawAccount)]| plan.build(&tx, read, blockhash(2)).unwrap_err();
    let ours = key.pubkey().to_bytes();

    assert_eq!(build(&[]), AdaptError::NotInitialized("Quantum".into()));
    let mut foreign = vector_account(&ours, SchemeId::Ed25519, [0; 32]);
    foreign.owner = pk(66);
    assert!(matches!(
        build(&[(account, foreign)]),
        AdaptError::VectorAccount { .. }
    ));
    let other = vector_account(&[9; 32], SchemeId::Ed25519, [0; 32]);
    assert_eq!(
        build(&[(account, other)]),
        AdaptError::OtherKey("Quantum".into())
    );
}

#[test]
fn a_vector_wallet_and_a_programmatic_signer_share_one_rebuild() {
    let mut d = desk();
    let key = KeypairSigner::new_random();
    let (_, account) = add_vector(&mut d, &key);
    let both = decode_v1_transaction(
        &compile_v1(
            d.payer.pubkey(),
            vec![transfer(account, pk(2), 5), transfer(d.address, pk(3), 5)],
            None,
            Some(blockhash(1)),
        )
        .unwrap()
        .wire,
    )
    .unwrap();
    let plan = plan_adaptation(&both, &d.wallets, None).unwrap().unwrap();
    assert_eq!(plan.asked().len(), 2, "{:?}", plan.asked());
    let read = [
        d.nonce_account(),
        (
            account,
            vector_account(key.pubkey().as_ref(), SchemeId::Ed25519, [1; 32]),
        ),
    ];
    let adapted = plan.build(&both, &read, blockhash(2)).unwrap();
    assert!(
        matches!(
            adapted.steps.as_slice(),
            [AdaptedStep::Programmatic { .. }, AdaptedStep::Vector { .. }]
        ),
        "{:?}",
        adapted.steps
    );
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(authority_slots(&rebuilt).len(), 2);

    let plan = plan_adaptation(&site_tx(account), &d.wallets, None)
        .unwrap()
        .unwrap();
    assert_eq!(
        plan.clone().unless_vector_off(false),
        Err(AdaptError::VectorOff("Quantum".into()))
    );
    assert_eq!(plan.clone().unless_vector_off(true), Ok(plan));
    // A plan for another kind of wallet is not the setting's business.
    let other = plan_adaptation(&site_tx(d.address), &d.wallets, None)
        .unwrap()
        .unwrap();
    assert!(other.unless_vector_off(false).is_ok());
}

#[test]
fn an_mldsa_member_refuses_when_even_create_and_propose_do_not_fit() {
    let mut d = desk();
    let (public, _) = crate::wallets::vector::new_mldsa44_authority();
    let mut wallet = WalletRecord::vector(
        "Dilithium",
        SchemeId::MlDsa44,
        PROGRAM_ID,
        public.clone(),
        Authority::Outside,
    );
    wallet.kind.set_fee_payer(Some(d.payer_id));
    let account = wallet.address().unwrap();
    let vector_id = wallet.id;
    d.wallets.push(wallet);
    let squad = d.add_squad(SquadsProgram::V4, vector_id);
    let vault = squad.vault(0);
    let payments: Vec<Instruction> = (0..40u8).map(|n| transfer(vault, pk(100 + n), 1)).collect();
    let wire = compile_v1(vault, payments, None, Some(blockhash(1)))
        .unwrap()
        .wire;
    let tx = decode_v1_transaction(&wire).unwrap();
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let read = [
        squad_account(&squad, 1, 0, 0, &[(account, PERMISSION_ALL)]),
        (account, vector_account(&public, SchemeId::MlDsa44, [1; 32])),
    ];
    let error = plan.build(&tx, &read, blockhash(2)).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("too large") && message.contains("bytes"),
        "{message}"
    );
}

#[test]
fn an_mldsa_member_still_executes_when_a_stacked_cu_estimate_would_have_exceeded() {
    let mut d = desk();
    let (public, _) = crate::wallets::vector::new_mldsa44_authority();
    let mut wallet = WalletRecord::vector(
        "Dilithium",
        SchemeId::MlDsa44,
        PROGRAM_ID,
        public.clone(),
        Authority::Outside,
    );
    wallet.kind.set_fee_payer(Some(d.payer_id));
    let account = wallet.address().unwrap();
    let vector_id = wallet.id;
    d.wallets.push(wallet);
    let squad = d.add_squad(SquadsProgram::V4, vector_id);
    let vault = squad.vault(0);
    // Eight transfers used to trip a static "site CU + wrap overhead" check
    // above 1.4M and drop execute. The wrap caps at the maximum; simulation
    // sizes the limit afterwards, so execute stays when the bytes fit.
    let payments: Vec<Instruction> = (0..8u8).map(|n| transfer(vault, pk(100 + n), 1)).collect();
    let wire = compile_v1(vault, payments, None, Some(blockhash(1)))
        .unwrap()
        .wire;
    let tx = decode_v1_transaction(&wire).unwrap();
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let read = [
        squad_account(&squad, 1, 0, 0, &[(account, PERMISSION_ALL)]),
        (account, vector_account(&public, SchemeId::MlDsa44, [1; 32])),
    ];
    let adapted = plan.build(&tx, &read, blockhash(2)).unwrap();
    assert!(
        matches!(
            &adapted.steps[0],
            AdaptedStep::Squads {
                voters,
                executor: Some(exec),
                held_back: None,
                ..
            } if voters == &[account] && exec == &account
        ),
        "{:?}",
        adapted.steps[0]
    );
    assert!(matches!(adapted.steps[1], AdaptedStep::Vector { .. }));
}

#[test]
fn a_multisig_acted_on_by_a_vector_wallet_proposes_through_its_advance() {
    let mut d = desk();
    let key = KeypairSigner::new_random();
    let (vector_id, account) = add_vector(&mut d, &key);
    let squad = d.add_squad(SquadsProgram::V4, vector_id);
    let vault = squad.vault(0);
    let plan = plan_adaptation(&site_tx(vault), &d.wallets, None)
        .unwrap()
        .unwrap();
    assert_eq!(plan.accounts_to_read(), vec![squad.address, account]);
    let read = [
        squad_account(&squad, 1, 0, 0, &[(account, PERMISSION_ALL)]),
        (
            account,
            vector_account(key.pubkey().as_ref(), SchemeId::Ed25519, [1; 32]),
        ),
    ];
    let adapted = plan.build(&site_tx(vault), &read, blockhash(2)).unwrap();
    assert!(matches!(
        adapted.steps.as_slice(),
        [AdaptedStep::Squads { .. }, AdaptedStep::Vector { .. }]
    ));
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(signer_keys(&rebuilt), vec![d.payer.pubkey()]);
    assert_eq!(authority_slots(&rebuilt)[0].address, account);
}

/// An XMSS account created with `first`, holding `current` at `next_leaf`.
fn xmss_account(first: &[u8], current: &[u8], next_leaf: u32) -> RawAccount {
    let mut data = [7u8; 32].to_vec();
    data.extend_from_slice(&seed(first));
    data.extend_from_slice(&[SchemeId::Xmss.byte(), 255, 1, 0]);
    data.extend_from_slice(current);
    data.extend_from_slice(&next_leaf.to_le_bytes());
    owned(PROGRAM_ID, data)
}

/// The key the rebuilt transaction's `Advance` names for the XMSS account
/// `account` to sign with next.
fn next_key_in(wire: &[u8], account: Pubkey) -> Vec<u8> {
    let tx = decode_v1_transaction(wire).unwrap();
    let known = [VectorContext {
        account,
        scheme: SchemeId::Xmss,
        nonce: None,
        key: None,
        leaf_floor: 0,
    }];
    let found = advance_next_keys(&tx, &AuthContext { vector: &known });
    assert_eq!(found.len(), 1);
    found[0].2.clone()
}

#[test]
fn an_xmss_wallet_rotates_only_when_few_leaves_are_left_and_never_into_the_key_it_holds() {
    let mut d = desk();
    let (first, next) = (vec![5u8; 41], vec![6u8; 41]);
    let mut wallet = WalletRecord::vector(
        "Leaves",
        SchemeId::Xmss,
        PROGRAM_ID,
        first.clone(),
        Authority::Outside,
    );
    wallet.kind.set_fee_payer(Some(d.payer_id));
    wallet.accept_vector_next_pubkey(next.clone()).unwrap();
    let account = wallet.address().unwrap();
    d.wallets.push(wallet);
    let tx = site_tx(account);
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let build = |held: &[u8], next_leaf: u32| {
        let read = [(account, xmss_account(&first, held, next_leaf))];
        plan.build(&tx, &read, blockhash(2))
    };

    // A next key is ready, but leaves are left: the Advance names the key
    // the account holds, which keeps it.
    assert_eq!(
        next_key_in(&build(&first, 10).unwrap().wire, account),
        first
    );
    // Past the threshold, the account moves to it.
    assert_eq!(
        next_key_in(&build(&first, 250).unwrap().wire, account),
        next
    );
    // The account already holds it: rotating into it again is refused.
    assert!(matches!(
        build(&next, 250),
        Err(AdaptError::Message(text)) if text.contains("already holds the next key")
    ));
}
