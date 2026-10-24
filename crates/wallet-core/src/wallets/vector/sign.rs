//! The keys Vector accounts' digests are signed with, and the check of a
//! signature made anywhere. Which key signs for a wallet of this device is
//! in `vector_wallets`.
//!
//! A digest is 32 bytes, shorter than any Solana message, so a digest
//! signed by a key that also pays fees can never pass for a transaction it
//! signed.

use super::scheme::SchemeId;
use super::xmss::XMSS_SALT_LEN;
use super::VectorError;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use falcon_rust::falcon512::{self, SecretKey as FalconSecret};
use fips204::ml_dsa_44::PrivateKey as MlDsaPrivate;
use fips204::traits::{SerDes, Signer as MlDsaSigner};
use k256::ecdsa::signature::hazmat::PrehashSigner;
use k256::ecdsa::SigningKey;
use rand::RngCore;
use solana_pubkey::Pubkey;
use solana_winternitz::hazmat::winternitz::SecretKey as WinternitzSecret;
use solana_winternitz::hazmat::xmss::SecretKey as XmssSecret;
use solana_winternitz::hazmat::OneTime;

/// Salts tried before a hash-based key gives up, as many as `hazmat` tries.
const SALT_TRIALS: usize = 4096;

/// A key of one scheme that this device holds.
pub enum VectorSigner {
    Ed25519(KeypairSigner),
    Secp256k1(SigningKey),
    Eip191(SigningKey),
    // The post-quantum and hash-based keys are kilobytes each, so they are
    // boxed and the enum stays the size of the small ones.
    MlDsa44(Box<MlDsaPrivate>),
    Falcon512(Box<FalconSecret>),
    Winternitz(Box<WinternitzSecret>),
    Xmss(Box<XmssSecret>),
}

impl VectorSigner {
    pub fn scheme(&self) -> SchemeId {
        match self {
            Self::Ed25519(_) => SchemeId::Ed25519,
            Self::Secp256k1(_) => SchemeId::Secp256k1,
            Self::Eip191(_) => SchemeId::Eip191,
            Self::MlDsa44(_) => SchemeId::MlDsa44,
            Self::Falcon512(_) => SchemeId::Falcon512,
            Self::Winternitz(_) => SchemeId::Winternitz,
            Self::Xmss(_) => SchemeId::Xmss,
        }
    }

    /// The public key that verifies this signer's signatures (current key).
    pub fn key(&self) -> Vec<u8> {
        match self {
            Self::Ed25519(signer) => signer.pubkey().to_bytes().to_vec(),
            Self::Secp256k1(key) => super::secp256k1_identity(key),
            Self::Eip191(key) => super::eip191_identity(key),
            Self::MlDsa44(key) => key.get_public_key().into_bytes().to_vec(),
            Self::Falcon512(key) => falcon512::PublicKey::from_secret_key(key).to_bytes(),
            Self::Winternitz(key) => key.verifying_key().to_bytes().to_vec(),
            Self::Xmss(key) => key.verifying_key().to_bytes().to_vec(),
        }
    }

    /// A signature over `digest`, as long as the scheme's.
    ///
    /// XMSS needs a reserved leaf: use [`Self::sign_leaf`].
    pub fn sign(&self, digest: &[u8; 32]) -> Result<Vec<u8>, VectorError> {
        let failed = || VectorError::SigningFailed {
            scheme: self.scheme().label(),
        };
        match self {
            Self::Ed25519(signer) => signer
                .try_sign_message(digest)
                .map(|signature| signature.as_ref().to_vec())
                .map_err(|_| failed()),
            Self::Secp256k1(key) => {
                let (signature, _) = key.sign_prehash(digest).map_err(|_| failed())?;
                Ok(signature.to_bytes().to_vec())
            }
            Self::Eip191(key) => {
                let envelope = vector_core::eip191_envelope_hash(digest);
                let (signature, recid) = key.sign_prehash(&envelope).map_err(|_| failed())?;
                let mut out = vec![0u8; 65];
                out[..64].copy_from_slice(&signature.to_bytes());
                // Raw recovery id 0 or 1: the chain rejects Ethereum's 27/28.
                out[64] = recid.to_byte();
                Ok(out)
            }
            Self::MlDsa44(key) => {
                // Empty NIST context: what solana-ml-dsa verifies.
                let signature = key.try_sign(digest, &[]).map_err(|_| failed())?;
                Ok(signature.to_vec())
            }
            Self::Falcon512(key) => {
                let mut signature = falcon512::sign(digest, key).to_bytes();
                // falcon-rust writes 0x59; the on-chain verifier wants 0x39.
                if signature.len() < SchemeId::Falcon512.signature_len() {
                    signature.resize(SchemeId::Falcon512.signature_len(), 0);
                }
                signature[0] = 0x39;
                Ok(signature)
            }
            Self::Winternitz(_) | Self::Xmss(_) => Err(VectorError::CannotSign {
                scheme: self.scheme().label(),
            }),
        }
    }

    /// An XMSS or Winternitz signature at a leaf already reserved for this
    /// digest. Winternitz has one leaf (0).
    pub fn sign_leaf(&self, leaf: u32, digest: &[u8; 32]) -> Result<Vec<u8>, VectorError> {
        let failed = || VectorError::SigningFailed {
            scheme: self.scheme().label(),
        };
        match self {
            Self::Winternitz(key) => {
                let signature = key.sign_at(leaf, digest, os_fill).map_err(|_| failed())?;
                Ok(signature.to_bytes().to_vec())
            }
            Self::Xmss(key) => {
                let signature = key.sign_at(leaf, digest, os_fill).map_err(|_| failed())?;
                Ok(signature.to_bytes().to_vec())
            }
            _ => self.sign(digest),
        }
    }

    /// A salt this hash-based key accepts for `digest` at `leaf`: one whose
    /// encoding meets the scheme's target sum, drawn from the OS the way
    /// `hazmat` draws one. What it signs to test a salt stays here.
    pub fn accepted_salt(
        &self,
        leaf: u32,
        digest: &[u8; 32],
    ) -> Result<[u8; XMSS_SALT_LEN], VectorError> {
        if !self.scheme().is_stateful() {
            return Err(VectorError::SigningFailed {
                scheme: self.scheme().label(),
            });
        }
        for _ in 0..SALT_TRIALS {
            let mut salt = [0u8; XMSS_SALT_LEN];
            rand::thread_rng().fill_bytes(&mut salt);
            if self.sign_leaf_with_salt(leaf, digest, &salt).is_ok() {
                return Ok(salt);
            }
        }
        Err(VectorError::SigningFailed {
            scheme: self.scheme().label(),
        })
    }

    /// Reproduce a hash-based signature with a salt from a previous attempt.
    pub fn sign_leaf_with_salt(
        &self,
        leaf: u32,
        digest: &[u8; 32],
        salt: &[u8; XMSS_SALT_LEN],
    ) -> Result<Vec<u8>, VectorError> {
        let failed = || VectorError::SigningFailed {
            scheme: self.scheme().label(),
        };
        match self {
            Self::Winternitz(key) => {
                let signature = key
                    .sign_at_with_salt(leaf, digest, salt)
                    .ok_or_else(failed)?;
                Ok(signature.to_bytes().to_vec())
            }
            Self::Xmss(key) => {
                let signature = key
                    .sign_at_with_salt(leaf, digest, salt)
                    .ok_or_else(failed)?;
                Ok(signature.to_bytes().to_vec())
            }
            _ => self.sign(digest),
        }
    }
}

/// A key this device holds for the Vector account at `account`.
pub struct VectorKey {
    pub account: Pubkey,
    pub signer: VectorSigner,
    /// The leaf reserved for a hash-based signature: XMSS's next leaf, or
    /// Winternitz's only one. Required when signing with either.
    pub leaf: Option<u32>,
    /// The salt to sign with: one recorded before signing (Winternitz), or
    /// from a prior attempt at the same digest (XMSS).
    pub salt: Option<[u8; XMSS_SALT_LEN]>,
    /// The key this device made for the account to rotate into, and holds
    /// the secret of. An `Advance` that hands the account to any other key
    /// is not signed.
    pub next_key: Option<Vec<u8>>,
}

fn os_fill(buf: &mut [u8]) -> Result<(), solana_winternitz::SigningError> {
    rand::thread_rng().fill_bytes(buf);
    Ok(())
}

/// `signature` is `key`'s, of `scheme`, over `digest`.
pub fn verify(
    scheme: SchemeId,
    key: &[u8],
    digest: &[u8; 32],
    signature: &[u8],
) -> Result<(), VectorError> {
    scheme.check_key(key)?;
    scheme.check_signature(signature)?;
    vector_core::verify_signature(&scheme.core(), key, digest, signature).map_err(|_| {
        VectorError::DoesNotVerify {
            scheme: scheme.label(),
        }
    })
}
