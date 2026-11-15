//! A signature made by an Ethereum wallet's `personal_sign` over a Vector
//! digest, pasted back into the app. The wallet signs the 39 bytes
//! `"vector:" || digest` ([`signed_message`]).

use super::scheme::SchemeId;
use super::sign::{signed_message, verify};
use super::VectorError;
use k256::ecdsa::{RecoveryId, Signature as SecpSignature, VerifyingKey as SecpVerifyingKey};
use sha3::{Digest as _, Keccak256};

/// Read a pasted EIP-191 `personal_sign` result for `address` over `digest`.
///
/// Hex, with or without `0x`. A legacy `v` of 27 or 28 is moved to 0 or 1
/// before the signature is kept, the recovery id the program reads either
/// way. When the signature recovers `address` only over the hex *text* of
/// what it should sign, the error says the wallet signed the text instead of
/// the 39 bytes.
pub fn parse_personal_sign(
    address: &[u8],
    digest: &[u8; 32],
    pasted: &str,
) -> Result<Vec<u8>, VectorError> {
    SchemeId::Eip191.check_key(address)?;
    let mut signature = decode_hex_signature(pasted)?;
    if signature.len() != SchemeId::Eip191.signature_len() {
        return Err(VectorError::BadPersonalSign);
    }
    if signature[64] == 27 || signature[64] == 28 {
        signature[64] -= 27;
    }
    if verify(SchemeId::Eip191, address, digest, &signature).is_ok() {
        return Ok(signature);
    }
    if recovers_eth_address(
        address,
        &personal_sign_hash(digest_hex_text(digest).as_bytes()),
        &signature,
    ) {
        return Err(VectorError::SignedDigestAsText);
    }
    Err(VectorError::DoesNotVerify {
        scheme: SchemeId::Eip191.label(),
    })
}

fn decode_hex_signature(pasted: &str) -> Result<Vec<u8>, VectorError> {
    let trimmed = pasted.trim();
    let hex = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    hex::decode(hex).map_err(|_| VectorError::BadPersonalSign)
}

/// The hex the Sign screen shows for the wallet to sign, as text.
fn digest_hex_text(digest: &[u8; 32]) -> String {
    format!("0x{}", hex::encode(signed_message(digest)))
}

fn personal_sign_hash(message: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    hasher.update(message);
    hasher.finalize().into()
}

fn recovers_eth_address(address: &[u8], prehash: &[u8; 32], signature: &[u8]) -> bool {
    let Ok(sig) = SecpSignature::from_slice(&signature[..64]) else {
        return false;
    };
    let Some(recid) = RecoveryId::from_byte(signature[64]) else {
        return false;
    };
    let Ok(recovered) = SecpVerifyingKey::recover_from_prehash(prehash, &sig, recid) else {
        return false;
    };
    let uncompressed = recovered.to_sec1_point(false);
    let hash: [u8; 32] = Keccak256::digest(&uncompressed.as_bytes()[1..]).into();
    hash[12..32] == *address
}
