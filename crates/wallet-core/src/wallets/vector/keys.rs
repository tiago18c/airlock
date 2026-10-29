//! New keys for Held Vector authorities, and the hash-based keys rebuilt
//! from the 32-byte seeds the vault keeps.
//!
//! Seeds made here come from the OS (`OsRng`) on each call.
//!
//! An XMSS or Winternitz key is rebuilt from its seed through a ChaCha20
//! stream (Construction 3 Gen via `hazmat`). Changing that stream changes
//! every key: the known-answer test in `sign_tests` pins it.

use super::scheme::SchemeId;
use super::VectorError;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::ChaCha20;
use falcon_rust::falcon512;
use fips204::ml_dsa_44;
use fips204::traits::SerDes;
use k256::ecdsa::SigningKey;
use k256::elliptic_curve::Generate;
use rand::RngCore;
use solana_pubkey::Pubkey;
use solana_winternitz::hazmat::winternitz::SecretKey as WinternitzSecret;
use solana_winternitz::hazmat::xmss::SecretKey as XmssSecret;
use std::str::FromStr;

/// A new secp256k1 key for a Held Vector authority: public identity, then
/// the 32-byte secret kept in the record.
pub fn new_secp256k1_authority() -> (Vec<u8>, Vec<u8>) {
    let key = SigningKey::generate();
    (secp256k1_identity(&key), key.to_bytes().to_vec())
}

/// The 33-byte compressed public key a secp256k1 account is known by.
///
/// Derived here rather than through `vector_core`, whose helpers take an older
/// k256's key type.
pub(crate) fn secp256k1_identity(key: &SigningKey) -> Vec<u8> {
    key.verifying_key().to_sec1_point(true).as_bytes().to_vec()
}

/// The 20-byte Ethereum address an EIP-191 account is known by.
pub(crate) fn eip191_identity(key: &SigningKey) -> Vec<u8> {
    let uncompressed = key.verifying_key().to_sec1_point(false);
    vector_core::eth_address_from_pubkey(uncompressed.as_bytes()).to_vec()
}

/// A new EIP-191 key for a Held Vector authority: Ethereum address, then the
/// 32-byte secret kept in the record.
pub fn new_eip191_authority() -> (Vec<u8>, Vec<u8>) {
    let key = SigningKey::generate();
    (eip191_identity(&key), key.to_bytes().to_vec())
}

/// A new ML-DSA-44 key for a Held Vector authority: public key, then the
/// private key bytes kept in the record.
pub fn new_mldsa44_authority() -> (Vec<u8>, Vec<u8>) {
    let (public, secret) = ml_dsa_44::try_keygen().expect("OS RNG for ML-DSA-44");
    (public.into_bytes().to_vec(), secret.into_bytes().to_vec())
}

/// A new Falcon-512 key for a Held Vector authority: public key, then the
/// secret key bytes kept in the record.
pub fn new_falcon512_authority() -> (Vec<u8>, Vec<u8>) {
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let (secret, public) = falcon512::keygen(seed);
    (public.to_bytes(), secret.to_bytes())
}

/// A new Winternitz key for a Held Vector authority: public key, then the
/// 32-byte seed the ChaCha20 stream expands into the hazmat secret.
pub fn new_winternitz_authority() -> (Vec<u8>, Vec<u8>) {
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let key = winternitz_from_seed(&seed).expect("OS RNG for Winternitz seed stream");
    (key.verifying_key().to_bytes().to_vec(), seed.to_vec())
}

/// A new XMSS key for a Held Vector authority: public key (root ‖ P), then
/// the 32-byte seed the ChaCha20 stream expands into the hazmat secret.
pub fn new_xmss_authority() -> (Vec<u8>, Vec<u8>) {
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let key = xmss_from_seed(&seed).expect("OS RNG for XMSS seed stream");
    (key.verifying_key().to_bytes().to_vec(), seed.to_vec())
}

/// Rebuild the Winternitz hazmat key from the vault's 32-byte seed.
pub fn winternitz_from_seed(seed: &[u8; 32]) -> Result<WinternitzSecret, VectorError> {
    WinternitzSecret::generate(hash_seed_fill(seed)).map_err(|_| VectorError::SigningFailed {
        scheme: SchemeId::Winternitz.label(),
    })
}

/// Rebuild the XMSS hazmat key from the vault's 32-byte seed.
pub fn xmss_from_seed(seed: &[u8; 32]) -> Result<XmssSecret, VectorError> {
    XmssSecret::generate(hash_seed_fill(seed)).map_err(|_| VectorError::SigningFailed {
        scheme: SchemeId::Xmss.label(),
    })
}

/// ChaCha20 keystream from `seed` with a zero nonce. Fills every Gen buffer
/// the same way every time, so a known seed yields a known public key.
fn hash_seed_fill(
    seed: &[u8; 32],
) -> impl FnMut(&mut [u8]) -> Result<(), solana_winternitz::SigningError> + '_ {
    let mut cipher = ChaCha20::new(seed.into(), &[0u8; 12].into());
    move |buf| {
        buf.fill(0);
        cipher.apply_keystream(buf);
        Ok(())
    }
}

/// A new key for an account of `scheme` to rotate into: public key, then
/// the seed kept until the rotation lands. `None` for a scheme that keeps
/// its key.
pub fn new_rotating_key(scheme: SchemeId) -> Option<(Vec<u8>, Vec<u8>)> {
    match scheme {
        SchemeId::Winternitz => Some(new_winternitz_authority()),
        SchemeId::Xmss => Some(new_xmss_authority()),
        SchemeId::Ed25519
        | SchemeId::Eip191
        | SchemeId::Secp256k1
        | SchemeId::Falcon512
        | SchemeId::MlDsa44 => None,
    }
}

/// A new key of `scheme` to keep in a Vector wallet's record: what the
/// account is created with, then the secret the record keeps.
pub fn new_held_key(scheme: SchemeId) -> (Vec<u8>, Vec<u8>) {
    match scheme {
        SchemeId::Ed25519 => {
            let key = KeypairSigner::new_random();
            (key.pubkey().to_bytes().to_vec(), key.seed_bytes().to_vec())
        }
        SchemeId::Secp256k1 => new_secp256k1_authority(),
        SchemeId::Eip191 => new_eip191_authority(),
        SchemeId::MlDsa44 => new_mldsa44_authority(),
        SchemeId::Falcon512 => new_falcon512_authority(),
        SchemeId::Winternitz => new_winternitz_authority(),
        SchemeId::Xmss => new_xmss_authority(),
    }
}

/// A key of `scheme` as a person pastes it: an Ed25519 key in base58, any
/// other in hex, with or without `0x`.
pub fn parse_public_key(scheme: SchemeId, text: &str) -> Result<Vec<u8>, VectorError> {
    let text = text.trim();
    let bad = || VectorError::BadKeyText {
        scheme: scheme.label(),
    };
    let key = match scheme {
        SchemeId::Ed25519 => Pubkey::from_str(text)
            .map_err(|_| bad())?
            .to_bytes()
            .to_vec(),
        SchemeId::Eip191
        | SchemeId::Secp256k1
        | SchemeId::Falcon512
        | SchemeId::MlDsa44
        | SchemeId::Winternitz
        | SchemeId::Xmss => {
            let hex = text
                .strip_prefix("0x")
                .or_else(|| text.strip_prefix("0X"))
                .unwrap_or(text);
            hex::decode(hex).map_err(|_| bad())?
        }
    };
    scheme.check_key(&key)?;
    Ok(key)
}
