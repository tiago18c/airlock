//! The digest of a transaction, and the writes the runtime would demote.
//! Whether the digest is the program's is proven in `flow-tests`, where a
//! transaction signed over it lands.

use super::*;
use crate::fixtures::pk;
use crate::ids::SYSTEM_PROGRAM_ID;
use crate::keys::{KeypairSigner, WalletSigner};
use crate::tx::{compile_v1_budget, decode_v1_transaction, ComputeBudgetSpec};
use agave_reserved_account_keys::ReservedAccountKeys;
use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_sdk_ids::{bpf_loader_upgradeable, sysvar};

fn transfer(from: Pubkey, to: Pubkey) -> Instruction {
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(from, true), AccountMeta::new(to, false)],
        data: vec![2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
    }
}

/// A transfer out of the account of `key`, wrapped for it, with a zeroed
/// signature: what the app would hand its authority to sign.
fn wrapped(key: &[u8], payer: Pubkey) -> Vec<Instruction> {
    let scheme = SchemeId::Ed25519;
    let account = address(&PROGRAM_ID, scheme, key).unwrap();
    vec![
        transfer(payer, pk(8)),
        advance(&[AdvanceEntry {
            scheme,
            key,
            next_key: &[],
            signature: &[0; 64],
        }])
        .unwrap(),
        passthrough(scheme, key, &[withdraw(scheme, key, &pk(9), 5).unwrap()]).unwrap(),
        transfer(payer, account),
    ]
}

#[test]
fn a_signature_over_the_digest_of_a_decoded_transaction_verifies_where_the_crate_rebuilds_it() {
    let authority = KeypairSigner::new_random();
    let key = authority.pubkey().to_bytes();
    let payer = pk(1);
    let instructions = wrapped(&key, payer);

    // What another device has: the transaction's bytes.
    let compiled = compile_v1_budget(
        payer,
        instructions.clone(),
        None,
        Some(Hash::new_from_array([3; 32])),
        ComputeBudgetSpec::default(),
    )
    .unwrap();
    let decoded = decode_v1_transaction(&compiled.wire).unwrap();
    let message = advance_message(&decoded.native_instructions(), &payer).unwrap();

    let account = address(&PROGRAM_ID, SchemeId::Ed25519, &key).unwrap();
    let nonce = [6u8; 32];
    let digest = signer_digest(&message, &nonce, &account, &[]);
    let signature = authority.try_sign_message(&digest).unwrap();

    let next = vector_core::verify_advance_signature(
        &SchemeId::Ed25519.core(),
        &key,
        &nonce,
        &instructions[..1],
        &instructions[2..],
        Some(&payer),
        signature.as_ref(),
    )
    .unwrap();
    assert_eq!(next, digest);
}

#[test]
fn the_message_changes_with_any_instruction_and_not_with_the_signatures() {
    let key = [7u8; 32];
    let payer = pk(1);
    let instructions = wrapped(&key, payer);
    let message = advance_message(&instructions, &payer).unwrap();

    let mut signed = instructions.clone();
    signed[1].data[1..].fill(9);
    assert_eq!(advance_message(&signed, &payer).unwrap(), message);

    let mut changed = instructions.clone();
    changed[3].data[4] = 2;
    assert_ne!(advance_message(&changed, &payer).unwrap(), message);
    // A payer no instruction names is not in the message. One that an
    // instruction names is, as a signer.
    assert_eq!(advance_message(&instructions, &pk(2)).unwrap(), message);
    assert_ne!(advance_message(&instructions, &pk(9)).unwrap(), message);
}

#[test]
fn a_digest_is_made_only_for_the_advance_the_transaction_holds() {
    let key = [7u8; 32];
    let payer = pk(1);
    let instructions = wrapped(&key, payer);

    assert_eq!(
        advance_message(&instructions[2..], &payer).unwrap_err(),
        VectorError::NoAdvance
    );
    let mut two = instructions.clone();
    two.push(instructions[1].clone());
    assert_eq!(
        advance_message(&two, &payer).unwrap_err(),
        VectorError::TwoAdvances
    );
    let mut bare = instructions.clone();
    bare[1].accounts.clear();
    assert_eq!(
        advance_message(&bare, &payer).unwrap_err(),
        VectorError::MalformedAdvance
    );
    assert_eq!(
        advance_accounts(&instructions).unwrap(),
        vec![address(&PROGRAM_ID, SchemeId::Ed25519, &key).unwrap()]
    );
}

#[test]
fn the_message_is_the_one_the_client_crate_makes_from_every_key() {
    let ed = [7u8; 32];
    let secp = [2u8; 33];
    let ml = vec![5u8; SchemeId::MlDsa44.key_len()];
    let wots = vec![3u8; SchemeId::Winternitz.key_len()];
    let wots_next = vec![4u8; SchemeId::Winternitz.key_len()];
    let signers = [
        (SchemeId::Ed25519, ed.as_slice(), [].as_slice()),
        (SchemeId::Secp256k1, secp.as_slice(), [].as_slice()),
        (SchemeId::MlDsa44, ml.as_slice(), [].as_slice()),
        (SchemeId::Winternitz, wots.as_slice(), wots_next.as_slice()),
    ];
    let zeros: Vec<Vec<u8>> = signers
        .iter()
        .map(|(scheme, ..)| vec![0; scheme.signature_len()])
        .collect();
    let laid: Vec<_> = signers
        .iter()
        .zip(&zeros)
        .map(|((scheme, key, next_key), signature)| AdvanceEntry {
            scheme: *scheme,
            key,
            next_key,
            signature,
        })
        .collect();
    let ed_account = address(&PROGRAM_ID, SchemeId::Ed25519, &ed).unwrap();
    let before = vec![transfer(pk(1), pk(8))];
    let after = vec![
        passthrough(
            SchemeId::Ed25519,
            &ed,
            &[withdraw(SchemeId::Ed25519, &ed, &pk(9), 5).unwrap()],
        )
        .unwrap(),
        transfer(pk(1), ed_account),
    ];
    let mut instructions = before.clone();
    instructions.push(advance(&laid).unwrap());
    instructions.extend(after.clone());

    let cores: Vec<_> = signers.iter().map(|(scheme, ..)| scheme.core()).collect();
    let keyed: Vec<_> = cores
        .iter()
        .zip(&signers)
        .map(|(core, (_, key, next_key))| vector_core::AdvanceSigner {
            scheme: core,
            identity: key,
            next_key,
        })
        .collect();
    // A payer the instructions name, and one they do not.
    for payer in [pk(1), pk(2)] {
        let theirs = vector_core::advance_message(&keyed, &before, &after, Some(&payer));
        assert_eq!(advance_message(&instructions, &payer).unwrap(), theirs);
    }
}

#[test]
fn a_rotating_digest_covers_the_next_key_as_the_crate_does() {
    let scheme = SchemeId::Winternitz;
    let key = vec![3u8; scheme.key_len()];
    let next = vec![4u8; scheme.key_len()];
    let payer = pk(1);
    let account = address(&PROGRAM_ID, scheme, &key).unwrap();
    let before = vec![transfer(payer, pk(8))];
    let after =
        vec![passthrough(scheme, &key, &[withdraw(scheme, &key, &pk(9), 5).unwrap()]).unwrap()];
    let mut instructions = before.clone();
    instructions.push(
        advance(&[AdvanceEntry {
            scheme,
            key: &key,
            next_key: &next,
            signature: &vec![0; scheme.signature_len()],
        }])
        .unwrap(),
    );
    instructions.extend(after.clone());

    let nonce = [6u8; 32];
    let message = advance_message(&instructions, &payer).unwrap();
    let ours = signer_digest(&message, &nonce, &account, &next);
    let theirs = vector_core::rotating_advance_vector_digest(
        &scheme.core(),
        &nonce,
        &key,
        &next,
        &before,
        &after,
        Some(&payer),
    );
    assert_eq!(ours, theirs);
    assert_ne!(ours, signer_digest(&message, &nonce, &account, &key));
}

#[test]
fn a_write_the_runtime_would_refuse_is_found() {
    let payer = pk(1);
    let plain = wrapped(&[7; 32], payer);
    assert_eq!(demoted_writable(&plain, &payer), None);

    let mut sysvar_written = plain.clone();
    sysvar_written[0]
        .accounts
        .push(AccountMeta::new(sysvar::clock::ID, false));
    assert_eq!(
        demoted_writable(&sysvar_written, &payer),
        Some(sysvar::clock::ID)
    );

    // A program the transaction calls, marked writable by another instruction.
    let mut program_written = plain.clone();
    program_written[0]
        .accounts
        .push(AccountMeta::new(PROGRAM_ID, false));
    assert_eq!(demoted_writable(&program_written, &payer), Some(PROGRAM_ID));

    // Unless the upgradeable loader is in the transaction: then it stays
    // writable, and the loader itself is read-only.
    program_written[0]
        .accounts
        .push(AccountMeta::new_readonly(bpf_loader_upgradeable::ID, false));
    assert_eq!(demoted_writable(&program_written, &payer), None);

    // Written only inside a Passthrough, the flag is in the message all the
    // same.
    let mut inner = plain.clone();
    inner[2]
        .accounts
        .push(AccountMeta::new(sysvar::rent::ID, false));
    assert_eq!(demoted_writable(&inner, &payer), Some(sysvar::rent::ID));
}

#[test]
fn the_reserved_keys_are_the_runtimes() {
    let ours: std::collections::HashSet<_> = super::digest::RESERVED.iter().copied().collect();
    let runtime: std::collections::HashSet<_> =
        ReservedAccountKeys::all_keys_iter().copied().collect();
    assert_eq!(ours, runtime);
}
