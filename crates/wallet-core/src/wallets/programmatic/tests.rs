use super::*;
use crate::fixtures::{blockhash, pk, transfer};
use crate::keys::signer::{KeypairSigner, WalletSigner};

fn nonce(authority: Pubkey) -> ExecutorNonce {
    ExecutorNonce {
        account: pk(9),
        authority,
        value: blockhash(3),
    }
}

#[test]
fn the_derived_address_differs_from_the_key_and_is_stable() {
    let derived = programmatic_signer_address(&pk(1));
    assert_ne!(derived, pk(1));
    assert_eq!(derived, programmatic_signer_address(&pk(1)));
    assert_ne!(derived, programmatic_signer_address(&pk(2)));
    assert!(!derived.is_on_curve());
}

/// Vectors from the nonce program's own `derivations_match_frozen_vectors`
/// and the executor's `transition_commitment_matches_snapshot`.
#[test]
fn the_next_nonce_matches_the_programs_own_vectors() {
    let empty = v1::Message::default();
    let commitment = Hash::new_from_array(sha256(&[&empty.serialize()]));
    assert_eq!(
        commitment.to_string(),
        "CBg3iVEh1d3hGJDQQ7eQxEJhZ9txvkCeeMDSMaVTWZwc"
    );

    let next = sha256(&[
        NONCE_STEP_TAG,
        &[1; 32],
        &[2; 32],
        "GgBaCs3NCBuZN12kCJgAW63ydqohFkHEdfdEXBPzLHq"
            .parse::<Hash>()
            .unwrap()
            .as_ref(),
        &[4; 32],
    ]);
    assert_eq!(
        Hash::new_from_array(next).to_string(),
        "EgbzChWYoCDgPbJNWv8nVUqdxnoxDTRXg2stzPKhYZu4"
    );
}

#[test]
fn an_authority_signs_the_envelope_and_a_plain_signer_is_forwarded() {
    let authority = KeypairSigner::new_random();
    let derived = programmatic_signer_address(&authority.pubkey());
    let funder = pk(7);
    let authorization = Authorization::new(
        derived,
        &[transfer(derived, pk(2), 5), transfer(funder, pk(2), 5)],
        &[authority.pubkey()],
        nonce(derived),
    )
    .unwrap();

    assert_eq!(authorization.authorities, vec![authority.pubkey()]);
    assert_eq!(authorization.forwarded, vec![funder]);
    // The envelope names the executor and nothing else, and has no lifetime.
    assert_eq!(authorization.message.instructions.len(), 1);
    assert_eq!(authorization.message.lifetime_specifier, Hash::default());
    assert_eq!(authorization.execution.lifetime_specifier, blockhash(3));

    let signature = authorization.sign(&authority).unwrap();
    let submit = authorization
        .submit_instruction(&BTreeMap::from([(authority.pubkey(), signature)]))
        .unwrap();
    assert_eq!(submit.program_id, ED25519_SIGNER_PROGRAM_ID);
    assert_eq!(
        submit.accounts.len(),
        authorization.message.account_keys.len()
    );
    let signers: Vec<Pubkey> = submit
        .accounts
        .iter()
        .filter(|meta| meta.is_signer)
        .map(|meta| meta.pubkey)
        .collect();
    assert_eq!(signers, vec![funder]);
}

#[test]
fn a_missing_wrong_or_foreign_signature_is_refused() {
    let authority = KeypairSigner::new_random();
    let stranger = KeypairSigner::new_random();
    let derived = programmatic_signer_address(&authority.pubkey());
    let authorization = Authorization::new(
        derived,
        &[transfer(derived, pk(2), 5)],
        &[authority.pubkey()],
        nonce(derived),
    )
    .unwrap();

    assert_eq!(
        authorization.submit_instruction(&BTreeMap::new()),
        Err(ProgrammaticError::MissingSignature(authority.pubkey()))
    );
    let wrong = stranger
        .try_sign_message(&authorization.signing_bytes())
        .unwrap();
    assert_eq!(
        authorization.submit_instruction(&BTreeMap::from([(authority.pubkey(), wrong)])),
        Err(ProgrammaticError::BadSignature(authority.pubkey()))
    );
    assert_eq!(
        authorization.sign(&stranger),
        Err(ProgrammaticError::NotAnAuthority(stranger.pubkey()))
    );
}

#[test]
fn a_submit_reads_back_and_takes_a_late_signature() {
    let authority = KeypairSigner::new_random();
    let derived = programmatic_signer_address(&authority.pubkey());
    let funder = pk(7);
    let inner = [transfer(derived, pk(2), 5), transfer(funder, pk(2), 5)];
    let authorization =
        Authorization::new(derived, &inner, &[authority.pubkey()], nonce(derived)).unwrap();

    // Built before the authority has signed: full size, nothing valid.
    let mut submit = authorization
        .submit_instruction_unsigned(&BTreeMap::new())
        .unwrap();
    let read = Submission::parse(&submit.data).unwrap();
    assert_eq!(read.signing_bytes, authorization.signing_bytes());
    assert_eq!(
        read.authorities().collect::<Vec<_>>(),
        vec![(authority.pubkey(), false)]
    );
    assert!(read
        .signers
        .iter()
        .any(|signer| signer.pubkey == funder && signer.role == SubmitRole::Forwarded));

    // What it wraps is readable all the way down.
    let execution = Execution::parse(&read.execute().unwrap()).unwrap();
    assert_eq!(execution.nonce_account, pk(9));
    assert_eq!(execution.nonce_authority, derived);
    assert_eq!(execution.nonce_value, blockhash(3));
    let run = execution.instructions();
    assert_eq!(run.len(), 2);
    assert_eq!(run[0].data, inner[0].data);
    assert!(run[0].accounts[0].is_signer && run[0].accounts[0].pubkey == derived);

    // A stranger's signature is refused and leaves the data alone.
    let stranger = KeypairSigner::new_random();
    let wrong = stranger.try_sign_message(&read.signing_bytes).unwrap();
    let before = submit.data.clone();
    assert_eq!(
        fill_submit_signature(&mut submit.data, &authority.pubkey(), &wrong),
        Err(ProgrammaticError::BadSignature(authority.pubkey()))
    );
    assert_eq!(
        fill_submit_signature(&mut submit.data, &stranger.pubkey(), &wrong),
        Err(ProgrammaticError::NotAnAuthority(stranger.pubkey()))
    );
    assert_eq!(submit.data, before);

    // The authority's own signature turns it into the signed instruction.
    let signature = authorization.sign(&authority).unwrap();
    fill_submit_signature(&mut submit.data, &authority.pubkey(), &signature).unwrap();
    let signed = authorization
        .submit_instruction(&BTreeMap::from([(authority.pubkey(), signature)]))
        .unwrap();
    assert_eq!(submit, signed);
    assert_eq!(
        Submission::parse(&submit.data)
            .unwrap()
            .authorities()
            .collect::<Vec<_>>(),
        vec![(authority.pubkey(), true)]
    );
}

#[test]
fn bytes_that_are_not_a_submit_are_not_read() {
    assert_eq!(Submission::parse(&[]), None);
    assert_eq!(Submission::parse(&[SUBMIT_TAG, 1, 1, 2, 3]), None);
    let authority = KeypairSigner::new_random();
    let derived = programmatic_signer_address(&authority.pubkey());
    let authorization = Authorization::new(
        derived,
        &[transfer(derived, pk(2), 5)],
        &[authority.pubkey()],
        nonce(derived),
    )
    .unwrap();
    let mut data = authorization
        .submit_instruction_unsigned(&BTreeMap::new())
        .unwrap()
        .data;
    // Bytes after the message are not part of what was signed.
    data.push(0);
    assert_eq!(Submission::parse(&data), None);
}

#[test]
fn an_authority_nothing_needs_is_refused() {
    let authority = pk(1);
    let err = Authorization::new(
        pk(7),
        &[transfer(pk(7), pk(2), 5)],
        &[authority],
        nonce(pk(7)),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ProgrammaticError::UnusedAuthority {
            authority,
            derived: programmatic_signer_address(&authority),
        }
    );
}

#[test]
fn a_nonce_account_is_read_from_its_72_bytes() {
    let mut data = vec![0u8; EXECUTOR_NONCE_LEN];
    assert_eq!(ExecutorNonce::parse(pk(9), &data), None);
    data[..32].fill(3);
    data[32..64].fill(5);
    data[64..].copy_from_slice(&42u64.to_le_bytes());
    assert_eq!(
        ExecutorNonce::parse(pk(9), &data),
        Some(ExecutorNonce {
            account: pk(9),
            authority: pk(5),
            value: blockhash(3),
        })
    );
    assert_eq!(ExecutorNonce::parse(pk(9), &data[1..]), None);
}
