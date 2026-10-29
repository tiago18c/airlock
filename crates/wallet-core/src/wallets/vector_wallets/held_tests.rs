//! Which key signs for a Vector wallet, and the check of what it signed.

use super::*;
use crate::ids::WalletId;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::wallets::record::{Authority, WalletRecord};
use crate::wallets::vector::*;
use k256::ecdsa::signature::hazmat::PrehashSigner;
use k256::ecdsa::{Signature as SecpSignature, SigningKey};
use sha2::Digest;
use wallet_vault::SecretBytes;

fn vector(key: &[u8], scheme: SchemeId, authority: Authority) -> WalletRecord {
    WalletRecord::vector("Quantum", scheme, PROGRAM_ID, key.to_vec(), authority)
}

fn held_ed25519(key: &KeypairSigner) -> Authority {
    Authority::Held {
        secret: SecretBytes(key.seed_bytes().to_vec()),
        next_leaf: 0,
    }
}

fn held_secp(secret: &[u8]) -> Authority {
    Authority::Held {
        secret: SecretBytes(secret.to_vec()),
        next_leaf: 0,
    }
}

fn secp_key() -> SigningKey {
    // A fixed non-zero scalar so the known-answer checks stay reproducible.
    SigningKey::from_slice(&[
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
        26, 27, 28, 29, 30, 31, 32,
    ])
    .unwrap()
}

#[test]
fn the_key_kept_in_the_record_or_a_hot_wallet_signs_and_nothing_else_does() {
    let key = KeypairSigner::new_random();
    let public = key.pubkey().to_bytes();

    let kept = vector(&public, SchemeId::Ed25519, held_ed25519(&key));
    let id = kept.id;
    let wallets = [kept];
    let signer = VectorSigner::from_record(&wallets, id).unwrap();
    assert_eq!(signer.key(), public.to_vec());
    let keys = local_vector_keys(&wallets);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].account, wallets[0].address().unwrap());

    let hot = WalletRecord::from_keypair_seed("Hot", key.seed_bytes());
    let through = vector(
        &public,
        SchemeId::Ed25519,
        Authority::Wallet { wallet: hot.id },
    );
    let id = through.id;
    let wallets = [hot, through];
    assert!(VectorSigner::from_record(&wallets, id).is_ok());

    let elsewhere = vector(&public, SchemeId::Ed25519, Authority::Outside);
    let id = elsewhere.id;
    let wallets = [elsewhere];
    assert_eq!(
        VectorSigner::from_record(&wallets, id).err(),
        Some(VectorError::SignedElsewhere("Quantum".into()))
    );
    assert!(local_vector_keys(&wallets).is_empty());

    // A key that is not the one the account was made with.
    let other = vector(&[9; 32], SchemeId::Ed25519, held_ed25519(&key));
    let id = other.id;
    assert_eq!(
        VectorSigner::from_record(&[other], id).err(),
        Some(VectorError::NotTheKey {
            wallet: "Quantum".into()
        })
    );
    assert_eq!(
        VectorSigner::from_record(&[], WalletId::new()).err(),
        Some(VectorError::NotVector)
    );
}

#[test]
fn a_secp256k1_key_in_the_record_signs_its_digest() {
    let secret = secp_key();
    let public = secp256k1_identity(&secret);
    let kept = vector(&public, SchemeId::Secp256k1, held_secp(&secret.to_bytes()));
    let id = kept.id;
    let wallets = [kept];
    let signer = VectorSigner::from_record(&wallets, id).unwrap();
    assert_eq!(signer.scheme(), SchemeId::Secp256k1);
    assert_eq!(signer.key(), public.to_vec());
    let digest = [7u8; 32];
    let signature = signer.sign(&digest).unwrap();
    assert_eq!(signature.len(), 64);
    verify(SchemeId::Secp256k1, &public, &digest, &signature).unwrap();
}

#[test]
fn an_eip191_key_in_the_record_signs_through_the_envelope() {
    let secret = secp_key();
    let address = eip191_identity(&secret);
    let kept = vector(&address, SchemeId::Eip191, held_secp(&secret.to_bytes()));
    let id = kept.id;
    let wallets = [kept];
    let signer = VectorSigner::from_record(&wallets, id).unwrap();
    assert_eq!(signer.scheme(), SchemeId::Eip191);
    assert_eq!(signer.key(), address.to_vec());
    let digest = [9u8; 32];
    let signature = signer.sign(&digest).unwrap();
    assert_eq!(signature.len(), 65);
    assert!(signature[64] <= 1, "v must be the raw recovery id");
    verify(SchemeId::Eip191, &address, &digest, &signature).unwrap();
}

#[test]
fn an_mldsa44_key_in_the_record_signs_its_digest() {
    let (public, secret) = new_mldsa44_authority();
    let kept = vector(
        &public,
        SchemeId::MlDsa44,
        Authority::Held {
            secret: SecretBytes(secret),
            next_leaf: 0,
        },
    );
    let id = kept.id;
    let wallets = [kept];
    let signer = VectorSigner::from_record(&wallets, id).unwrap();
    assert_eq!(signer.scheme(), SchemeId::MlDsa44);
    assert_eq!(signer.key(), public);
    let digest = [11u8; 32];
    let signature = signer.sign(&digest).unwrap();
    assert_eq!(signature.len(), SchemeId::MlDsa44.signature_len());
    verify(SchemeId::MlDsa44, &public, &digest, &signature).unwrap();
}

#[test]
fn an_xmss_seed_stream_is_pinned_and_signs_a_leaf() {
    let seed = [7u8; 32];
    let key = xmss_from_seed(&seed).unwrap();
    let public = key.verifying_key().to_bytes().to_vec();
    // Changing the ChaCha20 construction changes this hash.
    assert_eq!(
        &hex::encode(sha2::Sha256::digest(&public))[..16],
        "87e6e6bb6eedbac5"
    );
    let kept = vector(
        &public,
        SchemeId::Xmss,
        Authority::Held {
            secret: SecretBytes(seed.to_vec()),
            next_leaf: 0,
        },
    );
    let id = kept.id;
    let wallets = [kept];
    let signer = VectorSigner::from_record(&wallets, id).unwrap();
    assert_eq!(signer.scheme(), SchemeId::Xmss);
    assert_eq!(signer.key(), public);
    let digest = [15u8; 32];
    let signature = signer.sign_leaf(0, &digest).unwrap();
    assert_eq!(signature.len(), SchemeId::Xmss.signature_len());
    verify(SchemeId::Xmss, &public, &digest, &signature).unwrap();
}

#[test]
fn a_winternitz_seed_stream_is_pinned_and_signs_once() {
    let seed = [9u8; 32];
    let key = winternitz_from_seed(&seed).unwrap();
    let public = key.verifying_key().to_bytes().to_vec();
    assert_eq!(
        &hex::encode(sha2::Sha256::digest(&public))[..16],
        "f1d0be87e87cc889"
    );
    let kept = vector(
        &public,
        SchemeId::Winternitz,
        Authority::Held {
            secret: SecretBytes(seed.to_vec()),
            next_leaf: 0,
        },
    );
    let id = kept.id;
    let wallets = [kept];
    let signer = VectorSigner::from_record(&wallets, id).unwrap();
    assert_eq!(signer.scheme(), SchemeId::Winternitz);
    assert_eq!(signer.key(), public);
    let digest = [21u8; 32];
    let signature = signer.sign_leaf(0, &digest).unwrap();
    assert_eq!(signature.len(), SchemeId::Winternitz.signature_len());
    verify(SchemeId::Winternitz, &public, &digest, &signature).unwrap();
}

#[test]
fn a_falcon512_key_in_the_record_signs_with_a_0x39_header() {
    let (public, secret) = new_falcon512_authority();
    let kept = vector(
        &public,
        SchemeId::Falcon512,
        Authority::Held {
            secret: SecretBytes(secret),
            next_leaf: 0,
        },
    );
    let id = kept.id;
    let wallets = [kept];
    let signer = VectorSigner::from_record(&wallets, id).unwrap();
    assert_eq!(signer.scheme(), SchemeId::Falcon512);
    assert_eq!(signer.key(), public);
    let digest = [13u8; 32];
    let signature = signer.sign(&digest).unwrap();
    assert_eq!(signature.len(), SchemeId::Falcon512.signature_len());
    assert_eq!(signature[0], 0x39);
    verify(SchemeId::Falcon512, &public, &digest, &signature).unwrap();
}

#[test]
fn a_signature_verifies_only_over_its_digest_and_with_its_key() {
    let key = KeypairSigner::new_random();
    let public = key.pubkey().to_bytes();
    let signer = VectorSigner::Ed25519(KeypairSigner::from_seed(&key.seed_bytes()).unwrap());
    let digest = [3u8; 32];
    let signature = signer.sign(&digest).unwrap();
    assert_eq!(signature.len(), SchemeId::Ed25519.signature_len());
    verify(SchemeId::Ed25519, &public, &digest, &signature).unwrap();

    let refused = Err(VectorError::DoesNotVerify { scheme: "Ed25519" });
    assert_eq!(
        verify(SchemeId::Ed25519, &public, &[4; 32], &signature),
        refused
    );
    assert!(verify(SchemeId::Ed25519, &[7; 32], &digest, &signature).is_err());
    assert!(matches!(
        verify(SchemeId::Ed25519, &public, &digest, &signature[..63]),
        Err(VectorError::SignatureLength { .. })
    ));
}

#[test]
fn a_high_s_secp256k1_signature_still_verifies() {
    let secret = secp_key();
    let public = secp256k1_identity(&secret);
    let digest = [5u8; 32];
    let (raw, _) = secret.sign_prehash(&digest).unwrap();
    let bytes: [u8; 64] = raw.to_bytes().into();
    let is_high = |bytes: &[u8; 64]| {
        let sig = SecpSignature::from_slice(bytes).unwrap();
        sig.normalize_s() != sig
    };
    let sig = SecpSignature::from_slice(&bytes).unwrap();
    // Low-s becomes high-s by negating s; an already-high signature is kept.
    let high: [u8; 64] = if is_high(&bytes) {
        bytes
    } else {
        SecpSignature::from_scalars(sig.r(), -(*sig.s()))
            .unwrap()
            .to_bytes()
            .into()
    };
    assert!(is_high(&high), "the crafted signature must be high-s");
    verify(SchemeId::Secp256k1, &public, &digest, &high).unwrap();
}

#[test]
fn parse_personal_sign_normalizes_v_and_refuses_a_text_signature() {
    let secret = secp_key();
    let address = eip191_identity(&secret);
    let digest = [11u8; 32];
    let signer = VectorSigner::Eip191(secret.clone());
    let mut signature = signer.sign(&digest).unwrap();
    // The chain takes a raw recovery id, 0 or 1.
    assert!(signature[64] <= 1);

    // Legacy Ethereum v: 27 or 28.
    signature[64] += 27;
    let normalized = parse_personal_sign(&address, &digest, &hex::encode(&signature)).unwrap();
    assert!(normalized[64] <= 1);
    verify(SchemeId::Eip191, &address, &digest, &normalized).unwrap();
    // 0x prefix is accepted.
    parse_personal_sign(&address, &digest, &format!("0x{}", hex::encode(&signature))).unwrap();

    // A signature over the digest's hex text recovers the address but is
    // refused with a sentence that says so.
    let text = format!("0x{}", hex::encode(digest));
    let mut hasher = sha3::Keccak256::new();
    use sha3::Digest;
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", text.len()).as_bytes());
    hasher.update(text.as_bytes());
    let prehash: [u8; 32] = hasher.finalize().into();
    let (sig, recid) = secret.sign_prehash(&prehash).unwrap();
    let mut text_sig = vec![0u8; 65];
    text_sig[..64].copy_from_slice(&sig.to_bytes());
    text_sig[64] = recid.to_byte();
    assert_eq!(
        parse_personal_sign(&address, &digest, &hex::encode(&text_sig)),
        Err(VectorError::SignedDigestAsText)
    );
}
