//! Vector accounts against the deployed program: what `wallets/vector`
//! builds is what the program takes, and its digest is the program's.

use crate::support::{logs, Chain, SOL};
use solana_pubkey::Pubkey;
use wallet_core::vector::{
    address, advance, advance_message, initialize, passthrough, signer_digest, withdraw,
    AdvanceEntry, SchemeId, VectorAccount, VectorError, PROGRAM_ID,
};
use wallet_core::{
    apply_local_signatures, compile_v1_budget, decode_v1_transaction, ComputeBudgetSpec,
    Instruction, KeypairSigner, WalletSigner,
};

const SCHEME: SchemeId = SchemeId::Ed25519;

/// An initialized Ed25519 account holding one SOL, the key that signs for
/// it, and who pays.
fn funded_account(chain: &mut Chain) -> (KeypairSigner, KeypairSigner, Pubkey) {
    let payer = KeypairSigner::new_random();
    chain.fund(&payer.pubkey(), 10 * SOL);
    let authority = KeypairSigner::new_random();
    let key = authority.pubkey().to_bytes();
    chain
        .send(
            &[&payer],
            initialize(&payer.pubkey(), SCHEME, &key).unwrap(),
        )
        .unwrap_or_else(|failure| panic!("initialize: {}", logs(&failure)));
    let account = address(&PROGRAM_ID, SCHEME, &key).unwrap();
    chain.fund(&account, SOL);
    (payer, authority, account)
}

fn compile(chain: &Chain, payer: &KeypairSigner, instructions: Vec<Instruction>) -> Vec<u8> {
    let compiled = compile_v1_budget(
        payer.pubkey(),
        instructions,
        None,
        Some(chain.latest_blockhash()),
        ComputeBudgetSpec::default(),
    )
    .unwrap();
    apply_local_signatures(&compiled.wire, &[payer as &dyn WalletSigner]).unwrap()
}

/// Wrap `inner` for the account of `authority`, sign it at `nonce` the way
/// another device would, from the transaction's bytes, and return the wire
/// with the payer's signature too.
fn signed_wire(
    chain: &Chain,
    payer: &KeypairSigner,
    authority: &KeypairSigner,
    inner: &[Instruction],
    nonce: [u8; 32],
) -> Vec<u8> {
    let key = authority.pubkey().to_bytes();
    let build = |signature: &[u8]| {
        vec![
            advance(&[AdvanceEntry {
                scheme: SCHEME,
                key: &key,
                next_key: &[],
                signature,
            }])
            .unwrap(),
            passthrough(SCHEME, &key, inner).unwrap(),
        ]
    };

    let unsigned = decode_v1_transaction(&compile(chain, payer, build(&[0; 64]))).unwrap();
    let message = advance_message(&unsigned.native_instructions(), &payer.pubkey()).unwrap();
    let account = address(&PROGRAM_ID, SCHEME, &key).unwrap();
    let digest = signer_digest(&message, &nonce, &account, &[]);
    let signature = authority.try_sign_message(&digest).unwrap();
    compile(chain, payer, build(signature.as_ref()))
}

fn read(chain: &Chain, account: &Pubkey) -> VectorAccount {
    VectorAccount::parse(&PROGRAM_ID, &chain.account(account).unwrap()).unwrap()
}

#[test]
fn an_account_the_program_initializes_is_read_back_with_its_key() {
    let mut chain = Chain::new();
    let (_, authority, account) = funded_account(&mut chain);
    let key = authority.pubkey().to_bytes();

    let read = read(&chain, &account);
    assert_eq!(read.scheme, SCHEME);
    assert!(read.created_with(&key));
    assert!(!read.created_with(&[0; 32]));
    assert_eq!(read.current_key(), Some(key.as_slice()));
}

#[test]
fn an_ml_dsa_account_is_complete_only_once_grown_twice() {
    let mut chain = Chain::new();
    let payer = KeypairSigner::new_random();
    chain.fund(&payer.pubkey(), 10 * SOL);
    let key = vec![3u8; SchemeId::MlDsa44.key_len()];
    let account = address(&PROGRAM_ID, SchemeId::MlDsa44, &key).unwrap();
    let budget = ComputeBudgetSpec {
        cu_limit: Some(1_400_000),
        ..ComputeBudgetSpec::default()
    };

    let mut calls = initialize(&payer.pubkey(), SchemeId::MlDsa44, &key).unwrap();
    assert_eq!(calls.len(), 3);
    let grows = calls.split_off(1);
    chain
        .send_with(&[&payer], calls, budget.clone())
        .unwrap_or_else(|failure| panic!("initialize: {}", logs(&failure)));
    let short = VectorAccount::parse(&PROGRAM_ID, &chain.account(&account).unwrap());
    assert!(
        matches!(short, Err(VectorError::Incomplete { .. })),
        "{short:?}"
    );

    // Each later Initialize grows it; one transaction can hold all three,
    // as the app sends them, but they also land one at a time.
    for grow in grows {
        chain
            .send_with(&[&payer], vec![grow], budget.clone())
            .unwrap_or_else(|failure| panic!("grow: {}", logs(&failure)));
    }
    let read = read(&chain, &account);
    assert_eq!(read.scheme, SchemeId::MlDsa44);
    assert!(read.created_with(&key));
    assert_eq!(read.current_key(), Some(key.as_slice()));
}

#[test]
fn a_withdrawal_signed_over_the_digest_lands_and_moves_the_nonce() {
    let mut chain = Chain::new();
    let (payer, authority, account) = funded_account(&mut chain);
    let key = authority.pubkey().to_bytes();
    let receiver = KeypairSigner::new_random().pubkey();
    let before = read(&chain, &account);

    let lamports = SOL / 4;
    let inner = [withdraw(SCHEME, &key, &receiver, lamports).unwrap()];
    let wire = signed_wire(&chain, &payer, &authority, &inner, before.nonce);
    let landed = chain
        .send_wire(&wire)
        .unwrap_or_else(|failure| panic!("withdraw: {}", logs(&failure)));
    println!("Ed25519 Advance and Withdraw: {} CU", landed.compute_units);
    assert_eq!(chain.balance(&receiver), lamports);

    // The nonce moved, so the same signature does not land twice.
    assert_ne!(read(&chain, &account).nonce, before.nonce);
    assert!(chain.send_wire(&wire).is_err());
    assert_eq!(chain.balance(&receiver), lamports);
}

#[test]
fn a_signature_over_other_instructions_or_another_nonce_does_not_land() {
    let mut chain = Chain::new();
    let (payer, authority, account) = funded_account(&mut chain);
    let key = authority.pubkey().to_bytes();
    let receiver = KeypairSigner::new_random().pubkey();
    let nonce = read(&chain, &account).nonce;
    let small = [withdraw(SCHEME, &key, &receiver, 1_000).unwrap()];

    let stale = signed_wire(&chain, &payer, &authority, &small, [9; 32]);
    assert!(chain.send_wire(&stale).is_err());

    // Signed for 1,000 lamports, sent for 2,000.
    let signed = signed_wire(&chain, &payer, &authority, &small, nonce);
    let mut instructions = decode_v1_transaction(&signed)
        .unwrap()
        .native_instructions();
    let large = [withdraw(SCHEME, &key, &receiver, 2_000).unwrap()];
    instructions[1] = passthrough(SCHEME, &key, &large).unwrap();
    assert!(chain
        .send_wire(&compile(&chain, &payer, instructions))
        .is_err());

    assert_eq!(chain.balance(&receiver), 0);
    assert_eq!(read(&chain, &account).nonce, nonce);
}
