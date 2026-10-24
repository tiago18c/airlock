use super::*;
use crate::fixtures::{blockhash, compile, pk, transfer};
use crate::keys::signer::KeypairSigner;
use crate::tx::builder::compile_v1;
use crate::tx::decode::decode_v1_transaction;
use crate::wallets::programmatic::{programmatic_signer_address, ExecutorNonce};
use crate::wallets::relay::wrap_for_programmatic;

fn states(status: &SigningStatus) -> Vec<SignerState> {
    status.lines.iter().map(|line| line.state).collect()
}

#[test]
fn an_ordinary_transaction_is_signed_by_whoever_holds_a_key() {
    let payer = KeypairSigner::new_random();
    let other = KeypairSigner::new_random();
    let wire = compile(payer.pubkey(), vec![transfer(other.pubkey(), pk(2), 5)]).wire;
    let tx = decode_v1_transaction(&wire).unwrap();

    let status = signing_status(&tx, &[payer.pubkey()]);
    assert_eq!(
        status.lines.iter().map(|l| l.role).collect::<Vec<_>>(),
        vec![SignerRole::FeePayer, SignerRole::Signer]
    );
    assert_eq!(
        states(&status),
        vec![SignerState::Local, SignerState::Missing]
    );
    assert_eq!((status.signable, status.missing_elsewhere()), (1, 1));
    assert!(!status.waiting_for_authority && !status.complete());

    let signed = sign_locally(&wire, &[&payer as &dyn WalletSigner]).unwrap();
    assert_eq!((signed.authorizations, signed.signatures), (0, 1));
    let status = signing_status(
        &decode_v1_transaction(&signed.wire).unwrap(),
        &[payer.pubkey()],
    );
    assert_eq!(
        states(&status),
        vec![SignerState::Signed, SignerState::Missing]
    );
    assert_eq!(status.signable, 0);
    // Nothing more for this device.
    assert!(matches!(
        sign_locally(&signed.wire, &[&payer as &dyn WalletSigner]),
        Err(SignError::NoLocalSigner)
    ));

    let both = sign_locally(&signed.wire, &[&other as &dyn WalletSigner]).unwrap();
    assert!(signing_status(&decode_v1_transaction(&both.wire).unwrap(), &[]).complete());
}

fn wrapped(hot: &KeypairSigner, authority: &KeypairSigner) -> Vec<u8> {
    wrapped_for(hot, &[authority])
}

/// A transfer out of each authority's programmatic signer, in one
/// authorization on the first one's nonce, relayed by `hot`.
fn wrapped_for(hot: &KeypairSigner, authorities: &[&KeypairSigner]) -> Vec<u8> {
    let keys: Vec<Pubkey> = authorities.iter().map(|key| key.pubkey()).collect();
    let addresses: Vec<Pubkey> = keys.iter().map(programmatic_signer_address).collect();
    let built = compile_v1(
        hot.pubkey(),
        addresses
            .iter()
            .map(|address| transfer(*address, pk(2), 5))
            .collect(),
        None,
        Some(blockhash(1)),
    )
    .unwrap();
    wrap_for_programmatic(
        &decode_v1_transaction(&built.wire).unwrap(),
        &keys,
        ExecutorNonce {
            account: pk(9),
            authority: addresses[0],
            value: blockhash(3),
        },
        hot.pubkey(),
        blockhash(1),
    )
    .unwrap()
}

#[test]
fn the_payer_waits_until_every_authority_elsewhere_has_signed() {
    let hot = KeypairSigner::new_random();
    let (a, b) = (KeypairSigner::new_random(), KeypairSigner::new_random());
    let wire = wrapped_for(&hot, &[&a, &b]);
    let status = signing_status(&decode_v1_transaction(&wire).unwrap(), &[hot.pubkey()]);
    assert_eq!(status.total(), 3);
    assert_eq!(status.authorizations_elsewhere(), 2);
    assert_eq!(status.signable, 0);
    assert!(status.waiting_for_authority);

    // One of the two arrives. The payer still waits for the other.
    let one = sign_locally(&wire, &[&a as &dyn WalletSigner]).unwrap();
    assert_eq!((one.authorizations, one.signatures), (1, 0));
    let status = signing_status(&decode_v1_transaction(&one.wire).unwrap(), &[hot.pubkey()]);
    assert_eq!(status.authorizations_elsewhere(), 1);
    assert_eq!(status.signable, 0);
    assert!(status.waiting_for_authority);

    let both = sign_locally(&one.wire, &[&b as &dyn WalletSigner]).unwrap();
    let status = signing_status(&decode_v1_transaction(&both.wire).unwrap(), &[hot.pubkey()]);
    assert_eq!(status.authorizations_elsewhere(), 0);
    // The payer's own missing signature is not an authorization.
    assert_eq!((status.signable, status.waiting_for_authority), (1, false));
}

#[test]
fn with_one_authority_here_and_one_elsewhere_sign_fills_only_its_own() {
    let hot = KeypairSigner::new_random();
    let (here, away) = (KeypairSigner::new_random(), KeypairSigner::new_random());
    let wire = wrapped_for(&hot, &[&here, &away]);
    let local = [hot.pubkey(), here.pubkey()];
    let status = signing_status(&decode_v1_transaction(&wire).unwrap(), &local);
    assert_eq!(status.signable, 1);
    assert_eq!(status.authorizations_elsewhere(), 1);
    assert!(status.waiting_for_authority);

    let done = sign_locally(&wire, &[&hot as &dyn WalletSigner, &here]).unwrap();
    assert_eq!((done.authorizations, done.signatures), (1, 0));
    let status = signing_status(&decode_v1_transaction(&done.wire).unwrap(), &local);
    assert_eq!((status.signable, status.authorizations_elsewhere()), (0, 1));
}

#[test]
fn the_payer_waits_for_an_authority_that_is_elsewhere() {
    let hot = KeypairSigner::new_random();
    let authority = KeypairSigner::new_random();
    let wire = wrapped(&hot, &authority);
    let tx = decode_v1_transaction(&wire).unwrap();

    // The relaying device: it holds the payer's key only.
    let status = signing_status(&tx, &[hot.pubkey()]);
    assert_eq!(
        status.lines[0].role,
        SignerRole::Authority {
            address: programmatic_signer_address(&authority.pubkey())
        }
    );
    assert_eq!(
        states(&status),
        vec![SignerState::Missing, SignerState::Local]
    );
    assert_eq!(status.signable, 0);
    assert!(status.waiting_for_authority);
    assert!(matches!(
        sign_locally(&wire, &[&hot as &dyn WalletSigner]),
        Err(SignError::NoLocalSigner)
    ));

    // The device with the authority key: one thing to sign, and it does
    // not need the payer.
    let status = signing_status(&tx, &[authority.pubkey()]);
    assert_eq!(
        states(&status),
        vec![SignerState::Local, SignerState::Missing]
    );
    assert_eq!(status.signable, 1);
    assert!(!status.waiting_for_authority);
    let cold = sign_locally(&wire, &[&authority as &dyn WalletSigner]).unwrap();
    assert_eq!((cold.authorizations, cold.signatures), (1, 0));

    // Back on the relaying device the payer can now sign.
    let tx = decode_v1_transaction(&cold.wire).unwrap();
    let status = signing_status(&tx, &[hot.pubkey()]);
    assert_eq!(
        states(&status),
        vec![SignerState::Signed, SignerState::Local]
    );
    assert_eq!(status.signable, 1);
    assert!(!status.waiting_for_authority);
    let done = sign_locally(&cold.wire, &[&hot as &dyn WalletSigner]).unwrap();
    assert_eq!((done.authorizations, done.signatures), (0, 1));
    assert!(signing_status(&decode_v1_transaction(&done.wire).unwrap(), &[]).complete());
}

#[test]
fn a_device_with_both_keys_signs_the_authorization_then_the_transaction() {
    let hot = KeypairSigner::new_random();
    let authority = KeypairSigner::new_random();
    let wire = wrapped(&hot, &authority);
    let keys = [hot.pubkey(), authority.pubkey()];
    let status = signing_status(&decode_v1_transaction(&wire).unwrap(), &keys);
    assert_eq!(status.signable, 2);
    let signers = [&hot as &dyn WalletSigner, &authority];

    // The authorization changes what runs, so the transaction is simulated
    // again before anything signs over its limits: a press of its own.
    let first = sign_locally(&wire, &signers).unwrap();
    assert_eq!(
        (first.authorizations, first.signatures, first.dropped),
        (1, 0, 0)
    );
    assert!(first.transaction_next);
    let status = signing_status(&decode_v1_transaction(&first.wire).unwrap(), &keys);
    assert!(!status.complete());
    assert_eq!(next_step(&status), NextSignature::Transaction);

    let second = sign_locally(&first.wire, &signers).unwrap();
    assert_eq!(
        (second.authorizations, second.signatures, second.dropped),
        (0, 1, 0)
    );
    assert!(!second.transaction_next);
    assert!(signing_status(&decode_v1_transaction(&second.wire).unwrap(), &keys).complete());
}

#[test]
fn an_authorization_alone_is_not_followed_by_a_transaction_this_device_cannot_sign() {
    let hot = KeypairSigner::new_random();
    let authority = KeypairSigner::new_random();
    let wire = wrapped(&hot, &authority);
    let done = sign_locally(&wire, &[&authority as &dyn WalletSigner]).unwrap();
    assert_eq!((done.authorizations, done.signatures), (1, 0));
    assert!(!done.transaction_next);
}

#[test]
fn next_step_follows_authorizations_then_the_transaction() {
    let payer = KeypairSigner::new_random();
    let other = KeypairSigner::new_random();
    let wire = compile(payer.pubkey(), vec![transfer(other.pubkey(), pk(2), 5)]).wire;
    let tx = decode_v1_transaction(&wire).unwrap();
    let status = signing_status(&tx, &[payer.pubkey()]);
    assert_eq!(next_step(&status), NextSignature::Transaction);
    assert_eq!(tx_lifetime(&tx), TxLifetime::Blockhash(tx.lifetime));

    let hot = KeypairSigner::new_random();
    let authority = KeypairSigner::new_random();
    let wire = wrapped(&hot, &authority);
    let tx = decode_v1_transaction(&wire).unwrap();
    let before = signing_status(&tx, &[authority.pubkey()]);
    assert_eq!(next_step(&before), NextSignature::Authorization);
    assert!(matches!(tx_lifetime(&tx), TxLifetime::Blockhash(_)));

    let cold = sign_locally(&wire, &[&authority as &dyn WalletSigner]).unwrap();
    let after = signing_status(&decode_v1_transaction(&cold.wire).unwrap(), &[hot.pubkey()]);
    assert_eq!(next_step(&after), NextSignature::Transaction);

    let done = sign_locally(&cold.wire, &[&hot as &dyn WalletSigner]).unwrap();
    let complete = signing_status(&decode_v1_transaction(&done.wire).unwrap(), &[]);
    assert_eq!(next_step(&complete), NextSignature::Nothing);
}

#[test]
fn a_vector_authority_row_names_the_key_then_the_account() {
    use crate::keys::signer::WalletSigner;
    use crate::wallets::relay::{wrap_for_vector, AuthContext, VectorContext, VectorWrap};
    use crate::wallets::vector::{address, SchemeId, PROGRAM_ID};

    let payer = KeypairSigner::new_random();
    let authority = KeypairSigner::new_random();
    let account = address(&PROGRAM_ID, SchemeId::Ed25519, authority.pubkey().as_ref()).unwrap();
    let built = compile(payer.pubkey(), vec![transfer(account, pk(2), 5)]).wire;
    let wire = wrap_for_vector(
        &decode_v1_transaction(&built).unwrap(),
        &[VectorWrap {
            account,
            scheme: SchemeId::Ed25519,
            key: authority.pubkey().to_bytes().to_vec(),
            next_key: None,
        }],
        payer.pubkey(),
        blockhash(2),
    )
    .unwrap();
    let tx = decode_v1_transaction(&wire).unwrap();
    let known = [VectorContext {
        account,
        scheme: SchemeId::Ed25519,
        nonce: Some([3; 32]),
        key: Some(authority.pubkey().to_bytes().to_vec()),
        leaf_floor: 0,
    }];
    let status = signing_status_in(
        &tx,
        &LocalKeys {
            keys: &[payer.pubkey()],
            vector: &[account],
        },
        &AuthContext { vector: &known },
    );
    let line = &status.lines[0];
    assert_eq!(line.authority_chip(), authority.pubkey());
    assert_eq!(line.role, SignerRole::Authority { address: account });
    assert_ne!(line.authority_chip(), account);
}

#[test]
fn tx_lifetime_names_a_durable_nonce_account() {
    use crate::tx::builder::NonceSpec;

    let payer = KeypairSigner::new_random();
    let nonce = pk(4);
    let wire = compile_v1(
        payer.pubkey(),
        vec![transfer(payer.pubkey(), pk(2), 5)],
        Some(NonceSpec {
            account: nonce,
            authority: payer.pubkey(),
            hash: blockhash(7),
        }),
        None,
    )
    .unwrap()
    .wire;
    let tx = decode_v1_transaction(&wire).unwrap();
    assert!(tx.uses_durable_nonce);
    assert_eq!(
        tx_lifetime(&tx),
        TxLifetime::DurableNonce { account: nonce }
    );
}
