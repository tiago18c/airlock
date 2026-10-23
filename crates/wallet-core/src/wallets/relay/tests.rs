use super::*;
use crate::fixtures::{blockhash, pk, transfer};
use crate::keys::signer::KeypairSigner;
use crate::keys::signer::WalletSigner;
use crate::transport::protocol::Signed;
use crate::tx::builder::{
    compile_v1, compile_v1_budget, ComputeBudgetSpec, DEFAULT_CU_PER_INSTRUCTION, MAX_CU_LIMIT,
};
use crate::tx::signatures::apply_local_signatures;
use crate::wallets::programmatic::{
    programmatic_signer_address, Execution, ExecutorNonce, Submission, MESSAGE_EXECUTOR_PROGRAM_ID,
};

struct Setup {
    hot: KeypairSigner,
    authority: KeypairSigner,
    address: Pubkey,
    nonce: ExecutorNonce,
}

fn setup() -> Setup {
    let authority = KeypairSigner::new_random();
    let address = programmatic_signer_address(&authority.pubkey());
    Setup {
        hot: KeypairSigner::new_random(),
        address,
        nonce: ExecutorNonce {
            account: pk(9),
            authority: address,
            value: blockhash(3),
        },
        authority,
    }
}

impl Setup {
    /// What Build compiles when the acting wallet is a programmatic
    /// signer: its address spends, the hot wallet pays.
    fn plain(&self) -> DecodedTx {
        let wire = compile_v1(
            self.hot.pubkey(),
            vec![transfer(self.address, pk(2), 5)],
            None,
            Some(blockhash(1)),
        )
        .unwrap()
        .wire;
        decode_v1_transaction(&wire).unwrap()
    }

    fn wrapped(&self) -> Vec<u8> {
        wrap_for_programmatic(
            &self.plain(),
            &[self.authority.pubkey()],
            self.nonce,
            self.hot.pubkey(),
            blockhash(1),
        )
        .unwrap()
    }
}

#[test]
fn a_transaction_naming_a_programmatic_signer_is_rebuilt_around_its_instructions() {
    let s = setup();
    let plain = s.plain();
    // As built, it asks for a signature no key can make.
    assert_eq!(
        programmatic_signers_of(&plain, &[s.authority.pubkey(), pk(40)]),
        vec![(s.authority.pubkey(), s.address)]
    );

    let wrapped = decode_v1_transaction(&s.wrapped()).unwrap();
    // Only the hot wallet signs the transaction now.
    let signers: Vec<Pubkey> = wrapped.signers.iter().map(|slot| slot.pubkey).collect();
    assert_eq!(signers, vec![s.hot.pubkey()]);
    assert_eq!(wrapped.lifetime, blockhash(1));
    assert_eq!(wrapped.instructions.len(), 1);
    assert_eq!(
        authority_slots(&wrapped),
        vec![AuthoritySlot {
            instruction: 0,
            authority: AuthorityId::Ed25519(s.authority.pubkey()),
            address: s.address,
            place: None,
            next_key_len: 0,
            signed: false,
            verified: None,
        }]
    );
    // The instructions are the same ones, run by the executor.
    let submission = Submission::parse(&wrapped.instructions[0].data).unwrap();
    let execute = submission.execute().unwrap();
    assert_eq!(execute.program_id, MESSAGE_EXECUTOR_PROGRAM_ID);
    let run = Execution::parse(&execute).unwrap().instructions();
    assert_eq!(run, plain.native_instructions());
    // One instruction's default, plus the overhead of the way in.
    assert_eq!(
        wrapped.budget.compute_unit_limit,
        Some(DEFAULT_CU_PER_INSTRUCTION + RELAY_OVERHEAD_CU)
    );
}

#[test]
fn the_compute_limit_and_priority_fee_carry_over_and_the_loaded_limit_starts_over() {
    let s = setup();
    let wire = compile_v1_budget(
        s.hot.pubkey(),
        vec![transfer(s.address, pk(2), 5)],
        None,
        Some(blockhash(1)),
        ComputeBudgetSpec {
            cu_limit: Some(MAX_CU_LIMIT - 10),
            cu_price_micro_lamports: Some(1_000),
            loaded_accounts_data_size: Some(70_000),
        },
    )
    .unwrap()
    .wire;
    let plain = decode_v1_transaction(&wire).unwrap();
    let wrapped = wrap_for_programmatic(
        &plain,
        &[s.authority.pubkey()],
        s.nonce,
        s.hot.pubkey(),
        blockhash(1),
    )
    .unwrap();
    let wrapped = decode_v1_transaction(&wrapped).unwrap();
    assert_eq!(wrapped.budget.compute_unit_limit, Some(MAX_CU_LIMIT));
    assert_eq!(wrapped.budget.priority_fee, plain.budget.priority_fee);
    // 70,000 bytes was enough for the transfer alone, not for this.
    assert_eq!(
        wrapped.budget.loaded_accounts_data_size_limit,
        Some(DEFAULT_LOADED_ACCOUNTS_DATA_SIZE)
    );

    // Once a simulation has said what it loads, the limit is that plus 5%.
    let signed = sign_authorizations(&wrapped.wire, &[&s.authority])
        .unwrap()
        .wire;
    let signed = apply_local_signatures(&signed, &[&s.hot]).unwrap();
    let fitted = decode_v1_transaction(&fit_loaded_accounts(&signed, 497_297).unwrap()).unwrap();
    assert_eq!(fitted.budget.loaded_accounts_data_size_limit, Some(522_162));
    // The authorization is still signed. The transaction has to be again.
    assert!(authority_slots(&fitted).iter().all(|slot| slot.signed));
    assert!(!fitted.signers.iter().any(|slot| slot.filled));
    assert_eq!(
        fitted.instructions,
        decode_v1_transaction(&signed).unwrap().instructions
    );
}

#[test]
fn what_cannot_be_wrapped_is_refused_with_the_reason() {
    let s = setup();
    let plain = s.plain();
    let wrap = |tx: &DecodedTx, authorities: &[Pubkey], payer: Pubkey| {
        wrap_for_programmatic(tx, authorities, s.nonce, payer, blockhash(1))
    };
    assert_eq!(
        wrap(&plain, &[pk(40)], s.hot.pubkey()),
        Err(WrapError::NothingToWrap)
    );
    assert_eq!(
        wrap(&plain, &[s.authority.pubkey()], s.address),
        Err(WrapError::PayerCannotSign(s.address))
    );
    let again = decode_v1_transaction(&s.wrapped()).unwrap();
    assert_eq!(
        wrap(&again, &[s.authority.pubkey()], s.hot.pubkey()),
        Err(WrapError::AlreadyWrapped)
    );
    let on_nonce = compile_v1(
        s.hot.pubkey(),
        vec![transfer(s.address, pk(2), 5)],
        Some(crate::tx::builder::NonceSpec {
            account: pk(3),
            authority: s.hot.pubkey(),
            hash: blockhash(4),
        }),
        None,
    )
    .unwrap();
    assert_eq!(
        wrap(
            &decode_v1_transaction(&on_nonce.wire).unwrap(),
            &[s.authority.pubkey()],
            s.hot.pubkey()
        ),
        Err(WrapError::DurableNonce)
    );
}

#[test]
fn a_site_that_makes_the_programmatic_signer_pay_gets_a_hot_payer() {
    let s = setup();
    let wire = compile_v1(
        s.address,
        vec![transfer(s.address, pk(2), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    let wrapped = wrap_for_programmatic(
        &decode_v1_transaction(&wire).unwrap(),
        &[s.authority.pubkey()],
        s.nonce,
        s.hot.pubkey(),
        blockhash(2),
    )
    .unwrap();
    let wrapped = decode_v1_transaction(&wrapped).unwrap();
    assert_eq!(wrapped.fee_payer(), Some(s.hot.pubkey()));
    assert_eq!(wrapped.signers.len(), 1);
}

#[test]
fn the_authority_signs_first_and_the_payer_after() {
    let s = setup();
    let wrapped = s.wrapped();
    let both: [&dyn WalletSigner; 2] = [&s.authority, &s.hot];

    let authorized = sign_authorizations(&wrapped, &both).unwrap();
    assert_eq!((authorized.signed, authorized.dropped), (1, 0));
    assert_eq!(authorized.wire.len(), wrapped.len());
    let tx = decode_v1_transaction(&authorized.wire).unwrap();
    assert!(authority_slots(&tx)[0].signed);
    assert!(!tx.fully_signed());

    // Nothing left for an authority to do: the bytes come back as they are.
    let again = sign_authorizations(&authorized.wire, &both).unwrap();
    assert_eq!((again.signed, &again.wire), (0, &authorized.wire));

    let signed = apply_local_signatures(&authorized.wire, &both).unwrap();
    let tx = decode_v1_transaction(&signed).unwrap();
    assert!(tx.fully_signed() && authority_slots(&tx)[0].signed);
}

#[test]
fn a_payer_signature_made_too_early_is_dropped_and_reported() {
    let s = setup();
    let early = apply_local_signatures(&s.wrapped(), &[&s.hot as &dyn WalletSigner]).unwrap();
    assert!(decode_v1_transaction(&early).unwrap().fully_signed());

    let authorized = sign_authorizations(&early, &[&s.authority as &dyn WalletSigner]).unwrap();
    assert_eq!((authorized.signed, authorized.dropped), (1, 1));
    let tx = decode_v1_transaction(&authorized.wire).unwrap();
    assert!(!tx.signers[0].filled);
}

#[test]
fn an_authorization_signed_on_another_device_comes_back_as_a_signature() {
    let s = setup();
    let wrapped = s.wrapped();

    // The device with the authority key signs and shows a signature.
    let cold = sign_authorizations(&wrapped, &[&s.authority as &dyn WalletSigner]).unwrap();
    let objects = authority_signature_objects(&decode_v1_transaction(&cold.wire).unwrap());
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].pubkey, s.authority.pubkey());
    assert!(authority_signature_objects(&decode_v1_transaction(&wrapped).unwrap()).is_empty());

    // The device that relays still holds the unsigned transaction.
    let signed = objects[0].signed.clone();
    let Signed::Authorization {
        authority,
        signature,
    } = signed
    else {
        panic!("it says it signs an authorization");
    };
    let (wire, used) = accept_signature(&wrapped, &signed).unwrap();
    assert_eq!(used, SignatureUse::Authorization { dropped: 0 });
    assert_eq!(wire, cold.wire);

    // It fits nowhere else: not as a transaction signature, not twice,
    // not in an authorization over other instructions.
    let as_transaction = Signed::Transaction {
        pubkey: authority,
        signature,
    };
    assert!(accept_signature(&wrapped, &as_transaction).is_err());
    assert!(accept_signature(&wire, &signed).is_err());
    let other = wrap_for_programmatic(
        &decode_v1_transaction(
            &compile_v1(
                s.hot.pubkey(),
                vec![transfer(s.address, pk(2), 6)],
                None,
                Some(blockhash(1)),
            )
            .unwrap()
            .wire,
        )
        .unwrap(),
        &[s.authority.pubkey()],
        s.nonce,
        s.hot.pubkey(),
        blockhash(1),
    )
    .unwrap();
    assert!(accept_signature(&other, &signed).is_err());
    // An authorization for an account needs what this build does not have.
    let for_account = Signed::AccountAuthorization {
        account: [1; 32],
        nonce: [2; 32],
        signature: signature.to_vec(),
    };
    assert!(accept_signature(&wrapped, &for_account).is_err());
}

#[test]
fn one_key_can_be_the_payer_and_the_authority() {
    // A hot wallet that is its own programmatic signer's authority.
    let key = KeypairSigner::new_random();
    let address = programmatic_signer_address(&key.pubkey());
    let plain = compile_v1(
        key.pubkey(),
        vec![transfer(address, pk(2), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    let wrapped = wrap_for_programmatic(
        &decode_v1_transaction(&plain).unwrap(),
        &[key.pubkey()],
        ExecutorNonce {
            account: pk(9),
            authority: address,
            value: blockhash(3),
        },
        key.pubkey(),
        blockhash(1),
    )
    .unwrap();

    let authorized = sign_authorizations(&wrapped, &[&key as &dyn WalletSigner]).unwrap();
    let signed = apply_local_signatures(&authorized.wire, &[&key as &dyn WalletSigner]).unwrap();
    let tx = decode_v1_transaction(&signed).unwrap();
    assert!(tx.fully_signed() && authority_slots(&tx)[0].signed);

    // Each of its two signatures goes to its own place.
    let outer: [u8; SIGNATURE_BYTES] = signed[signed.len() - SIGNATURE_BYTES..].try_into().unwrap();
    let outer = Signed::Transaction {
        pubkey: key.pubkey().to_bytes(),
        signature: outer,
    };
    let (_, used) = accept_signature(&authorized.wire, &outer).unwrap();
    assert_eq!(used, SignatureUse::Transaction);
    let inner = authority_signature_objects(&tx)[0].signed.clone();
    let (_, used) = accept_signature(&wrapped, &inner).unwrap();
    assert_eq!(used, SignatureUse::Authorization { dropped: 0 });
    // Neither is taken for the other: the object says which it is.
    assert!(accept_signature(&wrapped, &outer).is_err());
}

#[test]
fn a_newer_blockhash_keeps_the_authorization_and_drops_the_payer_signature() {
    let s = setup();
    let both: [&dyn WalletSigner; 2] = [&s.authority, &s.hot];
    let authorized = sign_authorizations(&s.wrapped(), &both).unwrap();
    let signed = apply_local_signatures(&authorized.wire, &both).unwrap();

    let fresh = with_blockhash(&signed, blockhash(8)).unwrap();
    let tx = decode_v1_transaction(&fresh).unwrap();
    assert_eq!(tx.lifetime, blockhash(8));
    assert!(!tx.signers[0].filled);
    assert!(authority_slots(&tx)[0].signed);
    // The same blockhash again changes nothing, signatures included.
    assert_eq!(with_blockhash(&signed, blockhash(1)).unwrap(), signed);
}

/// Two programmatic signers spend in one transaction the hot wallet pays
/// for, wrapped on the first one's nonce.
fn wrapped_for_two(s: &Setup, second: &KeypairSigner) -> Vec<u8> {
    let other = programmatic_signer_address(&second.pubkey());
    let plain = compile_v1(
        s.hot.pubkey(),
        vec![transfer(s.address, pk(2), 5), transfer(other, pk(2), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    wrap_for_programmatic(
        &decode_v1_transaction(&plain).unwrap(),
        &[s.authority.pubkey(), second.pubkey()],
        s.nonce,
        s.hot.pubkey(),
        blockhash(1),
    )
    .unwrap()
}

/// What the device holding `key` shows after signing `wire`: its one
/// authorization signature.
fn signature_from(wire: &[u8], key: &KeypairSigner) -> Signed {
    let signed = sign_authorizations(wire, &[key as &dyn WalletSigner]).unwrap();
    let objects = authority_signature_objects(&decode_v1_transaction(&signed.wire).unwrap());
    assert_eq!(objects.len(), 1);
    objects[0].signed.clone()
}

#[test]
fn two_authorities_sign_the_same_transaction_apart_and_both_come_back_in_any_order() {
    let s = setup();
    let second = KeypairSigner::new_random();
    let wrapped = wrapped_for_two(&s, &second);
    assert_eq!(
        authority_slots(&decode_v1_transaction(&wrapped).unwrap()).len(),
        2
    );

    // Each device was shown the unsigned transaction and returns one signature.
    let first = signature_from(&wrapped, &s.authority);
    let second_one = signature_from(&wrapped, &second);

    let add = |wire: &[u8], signed: &Signed| {
        let (wire, used) = accept_signature(wire, signed).unwrap();
        assert_eq!(used, SignatureUse::Authorization { dropped: 0 });
        wire
    };
    let one_way = add(&add(&wrapped, &first), &second_one);
    let other_way = add(&add(&wrapped, &second_one), &first);
    assert_eq!(one_way, other_way);
    let tx = decode_v1_transaction(&one_way).unwrap();
    assert!(authority_slots(&tx).iter().all(|slot| slot.signed));

    // A newer blockhash leaves both in place, and the payer completes it.
    let fresh = with_blockhash(&one_way, blockhash(8)).unwrap();
    let tx = decode_v1_transaction(&fresh).unwrap();
    assert!(authority_slots(&tx).iter().all(|slot| slot.signed));
    let signed = apply_local_signatures(&fresh, &[&s.hot as &dyn WalletSigner]).unwrap();
    assert!(decode_v1_transaction(&signed).unwrap().fully_signed());
}

#[test]
fn a_signature_that_is_already_in_is_refused_with_that_reason() {
    let s = setup();
    let second = KeypairSigner::new_random();
    let wrapped = wrapped_for_two(&s, &second);
    let signed = signature_from(&wrapped, &s.authority);
    let Signed::Authorization { signature, .. } = signed else {
        panic!("it says it signs an authorization");
    };
    let (wire, _) = accept_signature(&wrapped, &signed).unwrap();

    let again = accept_signature(&wire, &signed).unwrap_err();
    assert_eq!(
        again.to_string(),
        "this authority's signature is already in the transaction"
    );
    // The other authority's place is still open, and a wrong signature for
    // it is told apart from one that is already there.
    let misplaced = Signed::Authorization {
        authority: second.pubkey().to_bytes(),
        signature,
    };
    let wrong = accept_signature(&wire, &misplaced).unwrap_err();
    assert_eq!(
        wrong.to_string(),
        "signature does not match this authorization"
    );
}

/// Instructions before the first the programmatic signer signs and after
/// the last stay at the top level. One between them moves inside, so
/// nothing runs out of order.
#[test]
fn a_programmatic_wrap_keeps_the_order_of_the_instructions() {
    let s = setup();
    let other = KeypairSigner::new_random();
    let before = transfer(s.hot.pubkey(), pk(3), 1);
    let first = transfer(s.address, pk(2), 5);
    let between = transfer(other.pubkey(), pk(4), 2);
    let last = transfer(s.address, pk(2), 6);
    let after = transfer(s.hot.pubkey(), pk(5), 3);
    let wire = compile_v1(
        s.hot.pubkey(),
        vec![
            before.clone(),
            first.clone(),
            between.clone(),
            last.clone(),
            after.clone(),
        ],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    let tx = decode_v1_transaction(&wire).unwrap();
    let wrapped = wrap_for_programmatic(
        &tx,
        &[s.authority.pubkey()],
        s.nonce,
        s.hot.pubkey(),
        blockhash(1),
    )
    .unwrap();

    let rebuilt = decode_v1_transaction(&wrapped).unwrap();
    let top = rebuilt.native_instructions();
    assert_eq!(top.len(), 3);
    assert_eq!(top[0].data, before.data);
    assert_eq!(top[2].data, after.data);
    let submission = Submission::parse(&top[1].data).unwrap();
    let run = Execution::parse(&submission.execute().unwrap())
        .unwrap()
        .instructions();
    let datas: Vec<Vec<u8>> = run.iter().map(|ix| ix.data.clone()).collect();
    assert_eq!(datas, vec![first.data, between.data, last.data]);
    // The key that signs the instruction in between signs the transaction.
    assert!(rebuilt
        .signers
        .iter()
        .any(|slot| slot.pubkey == other.pubkey()));
}
