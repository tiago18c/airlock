//! What a key of this device signs in an `Advance`: hash-based keys last,
//! only as the account's current key, and never a rotation to a key this
//! device did not make.

use super::*;
use crate::fixtures::{blockhash, pk, transfer};
use crate::keys::signer::KeypairSigner;
use crate::tx::builder::compile_v1;
use crate::wallets::vector::{
    address, winternitz_from_seed, xmss_from_seed, SchemeId, VectorError, VectorKey, VectorSigner,
    PROGRAM_ID,
};
use solana_instruction::Instruction;

/// A key of this device and the account it controls.
struct Holder {
    scheme: SchemeId,
    /// The key the account was created with, and signs with now.
    key: Vec<u8>,
    account: Pubkey,
    seed: [u8; 32],
}

impl Holder {
    fn ed25519() -> Self {
        let signer = KeypairSigner::new_random();
        Self::of(SchemeId::Ed25519, signer.seed_bytes())
    }

    fn of(scheme: SchemeId, seed: [u8; 32]) -> Self {
        let key = Self::signer_of(scheme, &seed).key();
        let account = address(&PROGRAM_ID, scheme, &key).unwrap();
        Self {
            scheme,
            key,
            account,
            seed,
        }
    }

    fn signer_of(scheme: SchemeId, seed: &[u8; 32]) -> VectorSigner {
        match scheme {
            SchemeId::Ed25519 => VectorSigner::Ed25519(KeypairSigner::from_seed(seed).unwrap()),
            SchemeId::Eip191 => {
                VectorSigner::Eip191(k256::ecdsa::SigningKey::from_slice(seed).unwrap())
            }
            SchemeId::Xmss => VectorSigner::Xmss(Box::new(xmss_from_seed(seed).unwrap())),
            SchemeId::Winternitz => {
                VectorSigner::Winternitz(Box::new(winternitz_from_seed(seed).unwrap()))
            }
            other => panic!("no test signer for {other:?}"),
        }
    }

    /// The wrap for this account. Without a next key, an XMSS account
    /// names the key it holds, which keeps it.
    fn wrap(&self, next_key: Option<Vec<u8>>) -> VectorWrap {
        let keeps = (self.scheme == SchemeId::Xmss).then(|| self.key.clone());
        VectorWrap {
            account: self.account,
            scheme: self.scheme,
            key: self.key.clone(),
            next_key: next_key.or(keeps),
        }
    }

    /// The key, with what a hash-based one signs with recorded.
    fn signer(&self, next_key: Option<Vec<u8>>) -> VectorKey {
        VectorKey {
            account: self.account,
            signer: Self::signer_of(self.scheme, &self.seed),
            leaf: self.scheme.is_stateful().then_some(0),
            salt: None,
            reserved_for: None,
            next_key,
        }
    }

    fn context(&self, nonce: u8) -> VectorContext {
        VectorContext {
            account: self.account,
            scheme: self.scheme,
            nonce: Some([nonce; 32]),
            key: Some(self.key.clone()),
            leaf_floor: 0,
        }
    }
}

/// The key a hash-based account rotates into.
fn next_key(seed: u8) -> Vec<u8> {
    winternitz_from_seed(&[seed; 32])
        .unwrap()
        .verifying_key()
        .to_bytes()
        .to_vec()
}

fn wrapped(wraps: &[VectorWrap], instructions: Vec<Instruction>) -> Vec<u8> {
    let site = compile_v1(pk(1), instructions, None, Some(blockhash(1)))
        .unwrap()
        .wire;
    let site = decode_v1_transaction(&site).unwrap();
    wrap_for_vector(&site, wraps, pk(1), blockhash(2)).unwrap()
}

#[test]
fn a_hash_based_key_waits_until_every_other_account_has_signed() {
    let plain = Holder::ed25519();
    let xmss = Holder::of(SchemeId::Xmss, [3; 32]);
    let wire = wrapped(
        &[plain.wrap(None), xmss.wrap(None)],
        vec![
            transfer(plain.account, pk(5), 1),
            transfer(xmss.account, pk(6), 2),
        ],
    );
    let known = [plain.context(1), xmss.context(2)];
    let ctx = AuthContext { vector: &known };

    // The Ed25519 account signs on another device, and has not yet.
    let held = sign_vector_authorizations(&wire, &[xmss.signer(None)], &ctx).unwrap();
    assert_eq!(held.signed, 0);
    assert!(digests_to_sign(&wire, &[xmss.account], &known).is_empty());

    let first = sign_vector_authorizations(&wire, &[plain.signer(None)], &ctx).unwrap();
    assert_eq!(first.signed, 1);
    assert_eq!(
        digests_to_sign(&first.wire, &[xmss.account], &known)
            .keys()
            .collect::<Vec<_>>(),
        vec![&xmss.account]
    );
    let last = sign_vector_authorizations(&first.wire, &[xmss.signer(None)], &ctx).unwrap();
    assert_eq!(last.signed, 1);

    // Both on this device: one press signs both.
    let both =
        sign_vector_authorizations(&wire, &[plain.signer(None), xmss.signer(None)], &ctx).unwrap();
    assert_eq!(both.signed, 2);
}

#[test]
fn only_accounts_that_sign_now_have_a_digest() {
    let plain = Holder::ed25519();
    let other = Holder::ed25519();
    let wire = wrapped(&[plain.wrap(None)], vec![transfer(plain.account, pk(5), 1)]);
    let known = [plain.context(1)];
    // A key for an account the transaction does not list.
    assert!(digests_to_sign(&wire, &[other.account], &known).is_empty());
    // Not a v1 transaction this app could have wrapped.
    assert!(digests_to_sign(&[1, 2, 3], &[plain.account], &known).is_empty());
    let signed = sign_vector_authorizations(
        &wire,
        &[plain.signer(None)],
        &AuthContext { vector: &known },
    )
    .unwrap();
    assert_eq!(digests_to_sign(&wire, &[plain.account], &known).len(), 1);
    // Its place is filled.
    assert!(digests_to_sign(&signed.wire, &[plain.account], &known).is_empty());
}

#[test]
fn a_rotation_to_a_key_this_device_did_not_make_is_not_signed() {
    let once = Holder::of(SchemeId::Winternitz, [4; 32]);
    let theirs = next_key(5);
    let wire = wrapped(
        &[once.wrap(Some(theirs.clone()))],
        vec![transfer(once.account, pk(5), 1)],
    );
    let known = [once.context(1)];
    let ctx = AuthContext { vector: &known };

    let mine = next_key(6);
    assert_eq!(
        sign_vector_authorizations(&wire, &[once.signer(Some(mine))], &ctx).unwrap_err(),
        SignError::from(VectorError::RotatesElsewhere(once.account))
    );
    assert_eq!(
        sign_vector_authorizations(&wire, &[once.signer(None)], &ctx).unwrap_err(),
        SignError::from(VectorError::RotatesElsewhere(once.account))
    );
    let signed = sign_vector_authorizations(&wire, &[once.signer(Some(theirs))], &ctx).unwrap();
    assert_eq!(signed.signed, 1);
    // The signature covers the next key the Advance names.
    let after = decode_v1_transaction(&signed.wire).unwrap();
    assert_eq!(authority_slots_in(&after, &ctx)[0].verified, Some(true));
}

#[test]
fn a_winternitz_key_does_not_sign_an_advance_that_hands_the_account_back_to_it() {
    let once = Holder::of(SchemeId::Winternitz, [4; 32]);
    let wire = wrapped(
        &[once.wrap(Some(once.key.clone()))],
        vec![transfer(once.account, pk(5), 1)],
    );
    let known = [once.context(1)];
    assert_eq!(
        sign_vector_authorizations(
            &wire,
            &[once.signer(Some(next_key(6)))],
            &AuthContext { vector: &known }
        )
        .unwrap_err(),
        SignError::from(VectorError::KeyReused(once.account))
    );
}

#[test]
fn a_hash_based_key_signs_only_as_the_key_the_account_holds_now() {
    let xmss = Holder::of(SchemeId::Xmss, [7; 32]);
    let wire = wrapped(&[xmss.wrap(None)], vec![transfer(xmss.account, pk(5), 1)]);
    // The account rotated: it holds another key than this device's.
    let mut moved = xmss.context(1);
    moved.key = Some(next_key(8));
    let known = [moved];
    assert_eq!(
        sign_vector_authorizations(&wire, &[xmss.signer(None)], &AuthContext { vector: &known })
            .unwrap_err(),
        SignError::from(VectorError::NotCurrentKey(xmss.account))
    );
}

#[test]
fn a_hash_based_key_with_nothing_recorded_does_not_sign() {
    let xmss = Holder::of(SchemeId::Xmss, [9; 32]);
    let wire = wrapped(&[xmss.wrap(None)], vec![transfer(xmss.account, pk(5), 1)]);
    let known = [xmss.context(1)];
    let mut key = xmss.signer(None);
    key.leaf = None;
    assert_eq!(
        sign_vector_authorizations(&wire, &[key], &AuthContext { vector: &known }).unwrap_err(),
        SignError::from(VectorError::NotReserved(xmss.account))
    );
}

#[test]
fn a_hash_based_key_signs_only_the_digest_it_was_recorded_for() {
    for scheme in [SchemeId::Xmss, SchemeId::Winternitz] {
        let holder = Holder::of(scheme, [9; 32]);
        let next = (scheme == SchemeId::Winternitz).then(|| next_key(4));
        let wire = wrapped(
            &[holder.wrap(next.clone())],
            vec![transfer(holder.account, pk(5), 1)],
        );
        let known = [holder.context(1)];
        let digest = digests_to_sign(&wire, &[holder.account], &known)[&holder.account];
        let ctx = AuthContext { vector: &known };

        // Recorded for another transaction: refused.
        let mut key = holder.signer(next.clone());
        key.reserved_for = Some([0xee; 32]);
        assert_eq!(
            sign_vector_authorizations(&wire, &[key], &ctx).unwrap_err(),
            SignError::from(VectorError::NotReserved(holder.account)),
            "{scheme:?}"
        );
        // Recorded for this one: signed.
        let mut key = holder.signer(next);
        key.reserved_for = Some(digest);
        if scheme == SchemeId::Winternitz {
            key.salt = Some(key.signer.accepted_salt(0, &digest).unwrap());
        }
        assert_eq!(
            sign_vector_authorizations(&wire, &[key], &ctx)
                .unwrap()
                .signed,
            1,
            "{scheme:?}"
        );
    }
}

#[test]
fn a_personal_sign_result_is_checked_and_written_at_the_accounts_place() {
    let eth = Holder::of(SchemeId::Eip191, [12; 32]);
    let wire = wrapped(&[eth.wrap(None)], vec![transfer(eth.account, pk(5), 1)]);
    let known = [eth.context(1)];
    let ctx = AuthContext { vector: &known };
    let tx = decode_v1_transaction(&wire).unwrap();
    let digest = personal_sign_digest(&tx, &eth.account, &ctx).unwrap();

    // What an Ethereum wallet returns: v as 27 or 28.
    let mut signature = Holder::signer_of(SchemeId::Eip191, &eth.seed)
        .sign(&digest)
        .unwrap();
    signature[64] += 27;
    let pasted = format!("0x{}", hex::encode(&signature));
    let written = insert_personal_sign(&wire, &eth.account, &pasted, &ctx, None).unwrap();
    let after = decode_v1_transaction(&written.wire).unwrap();
    let slot = authority_slots_in(&after, &ctx)[0];
    assert_eq!((slot.signed, slot.verified), (true, Some(true)));

    // Without the account's address in the context, the record's is used.
    let mut unknown = eth.context(1);
    unknown.key = None;
    let bare = [unknown];
    let bare = AuthContext { vector: &bare };
    assert!(insert_personal_sign(&wire, &eth.account, &pasted, &bare, None).is_err());
    assert!(insert_personal_sign(&wire, &eth.account, &pasted, &bare, Some(&eth.key)).is_ok());

    // A signature by another key does not verify.
    let other = Holder::signer_of(SchemeId::Eip191, &[13; 32])
        .sign(&digest)
        .unwrap();
    assert!(insert_personal_sign(&wire, &eth.account, &hex::encode(other), &ctx, None).is_err());
    // personal_sign is for EIP-191 accounts only.
    let plain = Holder::ed25519();
    let wire = wrapped(&[plain.wrap(None)], vec![transfer(plain.account, pk(5), 1)]);
    let known = [plain.context(1)];
    let tx = decode_v1_transaction(&wire).unwrap();
    assert!(personal_sign_digest(&tx, &plain.account, &AuthContext { vector: &known }).is_err());
}
