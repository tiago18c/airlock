//! Wrapping for Vector accounts, the places of their signatures, and signing
//! them. That what is built here lands is proven in `flow-tests`.

use super::*;
use crate::fixtures::{blockhash, pk, transfer};
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::tx::builder::{compile_v1, NonceSpec};
use crate::wallets::vector::{
    address, parse_passthrough, signer_digest, verify, SchemeId, VectorError, VectorKey,
    VectorSigner, PROGRAM_ID,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_sdk_ids::sysvar;

/// An Ed25519 key and the account it controls.
struct Owner {
    key: KeypairSigner,
    account: Pubkey,
}

impl Owner {
    fn new() -> Self {
        let key = KeypairSigner::new_random();
        let account = address(&PROGRAM_ID, SchemeId::Ed25519, key.pubkey().as_ref()).unwrap();
        Self { key, account }
    }

    fn wrap(&self) -> VectorWrap {
        VectorWrap {
            account: self.account,
            scheme: SchemeId::Ed25519,
            key: self.key.pubkey().to_bytes().to_vec(),
            next_key: None,
        }
    }

    fn signer(&self) -> VectorKey {
        VectorKey {
            account: self.account,
            signer: VectorSigner::Ed25519(
                KeypairSigner::from_seed(&self.key.seed_bytes()).unwrap(),
            ),
            leaf: None,
            salt: None,
            next_key: None,
        }
    }

    fn context(&self, nonce: [u8; 32]) -> VectorContext {
        VectorContext {
            account: self.account,
            scheme: SchemeId::Ed25519,
            nonce: Some(nonce),
            key: Some(self.key.pubkey().to_bytes().to_vec()),
            leaf_floor: 0,
        }
    }
}

fn decoded(payer: Pubkey, instructions: Vec<Instruction>) -> DecodedTx {
    let wire = compile_v1(payer, instructions, None, Some(blockhash(1)))
        .unwrap()
        .wire;
    decode_v1_transaction(&wire).unwrap()
}

fn wrapped(owners: &[&Owner], instructions: Vec<Instruction>) -> DecodedTx {
    let wraps: Vec<VectorWrap> = owners.iter().map(|owner| owner.wrap()).collect();
    let wire = wrap_for_vector(&decoded(pk(1), instructions), &wraps, pk(1), blockhash(2)).unwrap();
    decode_v1_transaction(&wire).unwrap()
}

#[test]
fn each_run_of_vector_instructions_becomes_one_passthrough_in_place() {
    let (a, b) = (Owner::new(), Owner::new());
    let site = decoded(
        pk(1),
        vec![
            transfer(a.account, pk(5), 1),
            transfer(a.account, pk(6), 2),
            transfer(pk(1), pk(7), 3),
            transfer(b.account, pk(8), 4),
            transfer(a.account, pk(9), 5),
        ],
    );
    let wire = wrap_for_vector(&site, &[b.wrap(), a.wrap()], pk(1), blockhash(2)).unwrap();
    let tx = decode_v1_transaction(&wire).unwrap();
    let native = tx.native_instructions();
    let programs: Vec<Pubkey> = native.iter().map(|ix| ix.program_id).collect();
    let system = crate::ids::SYSTEM_PROGRAM_ID;
    assert_eq!(programs, vec![PROGRAM_ID, PROGRAM_ID, system, PROGRAM_ID]);
    // The Advance lists the accounts in the order they were given, and so
    // does a Passthrough that runs as both.
    assert_eq!(vector_accounts(&tx), vec![b.account, a.account]);
    let runs: Vec<(Vec<Pubkey>, usize)> = [1, 3]
        .map(|at| {
            let read = parse_passthrough(&native[at]).unwrap();
            let runs_as = read.signers.iter().map(|signer| signer.account).collect();
            (runs_as, read.instructions.len())
        })
        .to_vec();
    assert_eq!(
        runs,
        vec![(vec![a.account], 2), (vec![b.account, a.account], 2)]
    );
    // No account but the payer signs the new transaction.
    assert_eq!(tx.signers.len(), 1);
    // What the site's instructions had, then each signature's and each
    // Passthrough's.
    assert_eq!(
        tx.budget.compute_unit_limit,
        Some(site.stated_cu_limit() + 2 * SchemeId::Ed25519.advance_cu() + 2 * PASSTHROUGH_CU)
    );
}

#[test]
fn a_transfer_out_becomes_a_withdraw_and_any_other_system_debit_is_refused() {
    let owner = Owner::new();
    let tx = wrapped(&[&owner], vec![transfer(owner.account, pk(5), 42)]);
    let inner = parse_passthrough(&tx.native_instructions()[1]).unwrap();
    let key = owner.key.pubkey().to_bytes();
    let mut expected =
        crate::wallets::vector::withdraw(SchemeId::Ed25519, &key, &pk(5), 42).unwrap();
    expected.accounts[0].is_signer = true;
    assert_eq!(inner.instructions, vec![expected]);

    let create =
        solana_system_interface::instruction::create_account(&owner.account, &pk(5), 1, 0, &pk(6));
    let tx = decoded(pk(1), vec![transfer(pk(1), pk(2), 1), create]);
    assert_eq!(
        wrap_for_vector(&tx, &[owner.wrap()], pk(1), blockhash(2)).unwrap_err(),
        WrapError::VectorSystemDebit {
            instruction: 1,
            account: owner.account
        }
    );
}

#[test]
fn what_one_advance_cannot_sign_is_refused() {
    let (a, b) = (Owner::new(), Owner::new());
    let both = [a.wrap(), b.wrap()];
    let wrap = |tx: &DecodedTx| wrap_for_vector(tx, &both, pk(1), blockhash(2));

    let plain = decoded(pk(1), vec![transfer(pk(1), pk(2), 1)]);
    assert_eq!(wrap(&plain).unwrap_err(), WrapError::NothingToWrap);

    let pays = decoded(pk(1), vec![transfer(a.account, pk(2), 1)]);
    assert_eq!(
        wrap_for_vector(&pays, &both, a.account, blockhash(2)).unwrap_err(),
        WrapError::PayerCannotSign(a.account)
    );

    let on_nonce = decode_v1_transaction(
        &compile_v1(
            pk(1),
            vec![transfer(a.account, pk(2), 1)],
            Some(NonceSpec {
                account: pk(30),
                authority: pk(1),
                hash: blockhash(3),
            }),
            None,
        )
        .unwrap()
        .wire,
    )
    .unwrap();
    assert_eq!(wrap(&on_nonce).unwrap_err(), WrapError::DurableNonce);

    let once = wrapped(&[&a], vec![transfer(a.account, pk(2), 1)]);
    assert_eq!(wrap(&once).unwrap_err(), WrapError::AlreadyWrapped);

    // Another program, asked to write a sysvar.
    let clock = Instruction {
        program_id: pk(40),
        accounts: vec![
            AccountMeta::new(a.account, true),
            AccountMeta::new(sysvar::clock::ID, false),
        ],
        data: vec![],
    };
    assert_eq!(
        wrap(&decoded(pk(1), vec![clock])).unwrap_err(),
        WrapError::Vector(VectorError::DemotedWritable(sysvar::clock::ID))
    );
}

#[test]
fn two_mldsa44_signatures_do_not_fit_in_a_v1_transaction() {
    let a = vec![1u8; SchemeId::MlDsa44.key_len()];
    let b = vec![2u8; SchemeId::MlDsa44.key_len()];
    let (a_account, b_account) = (
        address(&PROGRAM_ID, SchemeId::MlDsa44, &a).unwrap(),
        address(&PROGRAM_ID, SchemeId::MlDsa44, &b).unwrap(),
    );
    let site = decoded(
        pk(1),
        vec![transfer(a_account, pk(5), 1), transfer(b_account, pk(6), 2)],
    );
    let wraps = [
        VectorWrap {
            account: a_account,
            scheme: SchemeId::MlDsa44,
            key: a,
            next_key: None,
        },
        VectorWrap {
            account: b_account,
            scheme: SchemeId::MlDsa44,
            key: b,
            next_key: None,
        },
    ];
    let err = wrap_for_vector(&site, &wraps, pk(1), blockhash(2)).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("4096") && message.contains("bytes"),
        "{message}"
    );
}

#[test]
fn a_signature_goes_where_the_context_or_the_length_alone_puts_it() {
    use crate::wallets::vector::{advance, AdvanceEntry};
    let ed = [7u8; 32];
    let ml = vec![5u8; SchemeId::MlDsa44.key_len()];
    let ed_account = address(&PROGRAM_ID, SchemeId::Ed25519, &ed).unwrap();
    let ml_account = address(&PROGRAM_ID, SchemeId::MlDsa44, &ml).unwrap();
    let zeros = |scheme: SchemeId| vec![0u8; scheme.signature_len()];
    let (ed_zeros, ml_zeros) = (zeros(SchemeId::Ed25519), zeros(SchemeId::MlDsa44));
    let instruction = advance(&[
        AdvanceEntry {
            scheme: SchemeId::Ed25519,
            key: &ed,
            next_key: &[],
            signature: &ed_zeros,
        },
        AdvanceEntry {
            scheme: SchemeId::MlDsa44,
            key: &ml,
            next_key: &[],
            signature: &ml_zeros,
        },
    ])
    .unwrap();
    let tx = decoded(pk(1), vec![instruction]);

    // 64 + 2,420 could as well be 2,420 + 64.
    let unknown = authority_slots(&tx);
    assert!(unknown.iter().all(|slot| slot.place.is_none()));
    let one = [VectorContext {
        account: ml_account,
        scheme: SchemeId::MlDsa44,
        nonce: None,
        key: None,
        leaf_floor: 0,
    }];
    let known = authority_slots_in(&tx, &AuthContext { vector: &one });
    let places: Vec<_> = known
        .iter()
        .map(|slot| (slot.address, slot.place))
        .collect();
    assert_eq!(
        places,
        vec![(ed_account, Some((1, 64))), (ml_account, Some((65, 2420)))]
    );
}

#[test]
fn a_signature_made_here_verifies_over_the_digest_and_needs_the_nonce() {
    let owner = Owner::new();
    let tx = wrapped(&[&owner], vec![transfer(owner.account, pk(5), 42)]);
    let keys = [owner.signer()];

    let unknown = sign_vector_authorizations(&tx.wire, &keys, &AuthContext::default());
    assert_eq!(unknown.unwrap_err(), SignError::NoNonce(owner.account));

    let nonce = [4u8; 32];
    let known = [owner.context(nonce)];
    let ctx = AuthContext { vector: &known };
    let signed = sign_vector_authorizations(&tx.wire, &keys, &ctx).unwrap();
    assert_eq!(signed.signed, 1);
    let after = decode_v1_transaction(&signed.wire).unwrap();
    let slot = authority_slots_in(&after, &ctx)[0];
    assert_eq!((slot.signed, slot.verified), (true, Some(true)));

    // The same signature, checked by hand.
    let message =
        crate::wallets::vector::advance_message(&after.native_instructions(), &pk(1)).unwrap();
    let signature = &after.instructions[0].data[1..65];
    let key = owner.key.pubkey().to_bytes();
    verify(
        SchemeId::Ed25519,
        &key,
        &signer_digest(&message, &nonce, &owner.account, &[]),
        signature,
    )
    .unwrap();
    // Signed already, so a second press signs nothing.
    let again = sign_vector_authorizations(&signed.wire, &keys, &ctx).unwrap();
    assert_eq!((again.signed, again.wire), (0, signed.wire));
}

#[test]
fn a_signature_from_another_device_is_checked_against_the_nonce_and_the_key() {
    let owner = Owner::new();
    let tx = wrapped(&[&owner], vec![transfer(owner.account, pk(5), 42)]);
    let nonce = [4u8; 32];
    let known = [owner.context(nonce)];
    let ctx = AuthContext { vector: &known };
    let message =
        crate::wallets::vector::advance_message(&tx.native_instructions(), &pk(1)).unwrap();
    let sign_at = |nonce: &[u8; 32]| {
        owner
            .key
            .try_sign_message(&signer_digest(&message, nonce, &owner.account, &[]))
            .unwrap()
            .as_ref()
            .to_vec()
    };
    let insert = |nonce: &[u8; 32], signature: &[u8], ctx: &AuthContext| {
        insert_vector_signature(&tx.wire, &owner.account, nonce, signature, ctx)
    };

    // At another nonce than the account holds.
    let stale = insert(&[3; 32], &sign_at(&[3; 32]), &ctx).unwrap_err();
    assert!(stale.to_string().contains("another nonce"), "{stale}");
    // Made over something else.
    let wrong = insert(&nonce, &sign_at(&[3; 32]), &ctx).unwrap_err();
    assert_eq!(
        wrong,
        SignError::Vector(VectorError::DoesNotVerify { scheme: "Ed25519" })
    );
    // Too short for its place.
    assert!(insert(&nonce, &[1; 63], &ctx).is_err());

    let placed = insert(&nonce, &sign_at(&nonce), &ctx).unwrap();
    let slot = authority_slots_in(&decode_v1_transaction(&placed.wire).unwrap(), &ctx)[0];
    assert_eq!((slot.signed, slot.verified), (true, Some(true)));

    // A device that knows neither the nonce nor the key places it as it is.
    let blind = insert(&[3; 32], &sign_at(&[3; 32]), &AuthContext::default()).unwrap();
    let slot = authority_slots(&decode_v1_transaction(&blind.wire).unwrap())[0];
    assert_eq!((slot.signed, slot.verified), (true, None));
    // And the one that knows them finds it does not verify.
    let checked = authority_slots_in(&decode_v1_transaction(&blind.wire).unwrap(), &ctx)[0];
    assert_eq!(checked.verified, Some(false));
}

/// Instructions next to each other that different Vector accounts sign run
/// in one `Passthrough` that runs as both, and so does one both sign.
#[test]
fn adjacent_vector_instructions_share_one_passthrough() {
    let (a, b) = (Owner::new(), Owner::new());
    let both = [a.wrap(), b.wrap()];
    let mut shared = Instruction::new_with_bytes(pk(40), &[1], vec![]);
    shared.accounts = vec![
        AccountMeta::new_readonly(a.account, true),
        AccountMeta::new_readonly(b.account, true),
    ];
    let tx = decoded(
        pk(1),
        vec![
            transfer(a.account, pk(2), 1),
            transfer(b.account, pk(3), 2),
            shared,
            transfer(pk(1), pk(4), 3),
            transfer(b.account, pk(5), 4),
        ],
    );
    let wire = wrap_for_vector(&tx, &both, pk(1), blockhash(2)).unwrap();
    let wrapped = decode_v1_transaction(&wire).unwrap();
    let native = wrapped.native_instructions();
    // Advance, one Passthrough for the first three, the plain transfer, and
    // a Passthrough for the last.
    assert_eq!(native.len(), 4);
    let first = parse_passthrough(&native[1]).unwrap();
    let runs_as: Vec<Pubkey> = first.signers.iter().map(|signer| signer.account).collect();
    assert_eq!(runs_as, vec![a.account, b.account]);
    assert_eq!(first.instructions.len(), 3);
    assert_eq!(native[2].program_id, solana_system_interface::program::ID);
    let last = parse_passthrough(&native[3]).unwrap();
    assert_eq!(last.signers.len(), 1);
    assert_eq!(last.signers[0].account, b.account);
}
