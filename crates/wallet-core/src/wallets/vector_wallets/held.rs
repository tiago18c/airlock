//! The keys this device holds for its Vector wallets: built from each
//! wallet's record, or from the hot wallet that is its authority.

use crate::ids::WalletId;
use crate::keys::signer::KeypairSigner;
use crate::wallets::record::{Authority, WalletRecord};
use crate::wallets::vector::{
    signs_here, winternitz_from_seed, xmss_from_seed, SchemeId, VectorError, VectorKey,
    VectorSigner,
};
use falcon_rust::falcon512::SecretKey as FalconSecret;
use fips204::ml_dsa_44::{self, PrivateKey as MlDsaPrivate};
use fips204::traits::SerDes;
use k256::ecdsa::SigningKey;
use solana_pubkey::Pubkey;

impl VectorSigner {
    /// The key that signs for the Vector wallet `id`: the one its record
    /// keeps, or the hot wallet that is its authority. It has to be the key
    /// the account was created with.
    pub fn from_record(wallets: &[WalletRecord], id: WalletId) -> Result<Self, VectorError> {
        let wallet = wallets
            .iter()
            .find(|wallet| wallet.id == id)
            .ok_or(VectorError::NotVector)?;
        let settings = wallet
            .kind
            .vector_settings()
            .ok_or(VectorError::NotVector)?;
        if !signs_here(settings.scheme) {
            return Err(VectorError::CannotSign {
                scheme: settings.scheme.label(),
            });
        }
        let bad_held = || VectorError::BadHeldKey {
            wallet: wallet.label.clone(),
        };
        let signer = match (settings.scheme, settings.authority) {
            (SchemeId::Ed25519, Authority::Held { secret, .. }) => {
                let key = KeypairSigner::from_seed(&secret.0).map_err(|_| bad_held())?;
                Self::Ed25519(key)
            }
            (SchemeId::Ed25519, Authority::Wallet { wallet: authority }) => {
                let key = wallets
                    .iter()
                    .find(|other| other.id == *authority)
                    .and_then(|other| other.signer().ok())
                    .ok_or_else(|| VectorError::SignedElsewhere(wallet.label.clone()))?;
                Self::Ed25519(key)
            }
            (SchemeId::Secp256k1, Authority::Held { secret, .. }) => {
                let key = SigningKey::from_slice(&secret.0).map_err(|_| bad_held())?;
                Self::Secp256k1(key)
            }
            (SchemeId::Eip191, Authority::Held { secret, .. }) => {
                let key = SigningKey::from_slice(&secret.0).map_err(|_| bad_held())?;
                Self::Eip191(key)
            }
            (SchemeId::MlDsa44, Authority::Held { secret, .. }) => {
                let bytes: [u8; ml_dsa_44::SK_LEN] =
                    secret.0.as_slice().try_into().map_err(|_| bad_held())?;
                let key = MlDsaPrivate::try_from_bytes(bytes).map_err(|_| bad_held())?;
                Self::MlDsa44(Box::new(key))
            }
            (SchemeId::Falcon512, Authority::Held { secret, .. }) => {
                let key = FalconSecret::from_bytes(&secret.0).map_err(|_| bad_held())?;
                Self::Falcon512(Box::new(key))
            }
            (SchemeId::Winternitz, Authority::Held { secret, .. }) => {
                let seed: [u8; 32] = secret.0.as_slice().try_into().map_err(|_| bad_held())?;
                let key = winternitz_from_seed(&seed).map_err(|_| bad_held())?;
                Self::Winternitz(Box::new(key))
            }
            (SchemeId::Xmss, Authority::Held { secret, .. }) => {
                let seed: [u8; 32] = secret.0.as_slice().try_into().map_err(|_| bad_held())?;
                let key = xmss_from_seed(&seed).map_err(|_| bad_held())?;
                Self::Xmss(Box::new(key))
            }
            (_, Authority::Outside) => {
                return Err(VectorError::SignedElsewhere(wallet.label.clone()))
            }
            (scheme, Authority::Wallet { .. }) => {
                return Err(VectorError::CannotSign {
                    scheme: scheme.label(),
                })
            }
        };
        // Rotating schemes keep the Initialize key in the record for life;
        // the Held seed is the key that signs now.
        if !settings.scheme.is_stateful() && signer.key() != settings.key {
            return Err(VectorError::NotTheKey {
                wallet: wallet.label.clone(),
            });
        }
        Ok(signer)
    }
}

/// Every Vector account of `wallets` whose key this device holds. One whose
/// key is elsewhere, or of a scheme this build cannot sign with, is left out.
/// XMSS leaves are not reserved here: the Sign path sets [`VectorKey::leaf`].
pub fn local_vector_keys(wallets: &[WalletRecord]) -> Vec<VectorKey> {
    wallets
        .iter()
        .filter(|wallet| wallet.kind.vector_settings().is_some())
        .filter_map(|wallet| {
            Some(VectorKey {
                account: wallet.address()?,
                signer: VectorSigner::from_record(wallets, wallet.id).ok()?,
                leaf: None,
                salt: None,
                reserved_for: None,
                next_key: wallet.vector_prepared_next_key().map(<[u8]>::to_vec),
            })
        })
        .collect()
}

/// The addresses of [`local_vector_keys`], without building a key. A screen
/// asks this on every render: rebuilding an XMSS key takes tens of
/// milliseconds, and the secrets do not need to be in memory to say which
/// accounts this device signs for.
pub fn held_vector_accounts(wallets: &[WalletRecord]) -> Vec<Pubkey> {
    wallets
        .iter()
        .filter_map(|wallet| {
            let settings = wallet.kind.vector_settings()?;
            if !signs_here(settings.scheme) {
                return None;
            }
            let holds = match settings.authority {
                Authority::Held { .. } => true,
                Authority::Wallet { wallet: authority } => {
                    settings.scheme == SchemeId::Ed25519
                        && wallets
                            .iter()
                            .any(|other| other.id == *authority && other.kind.can_sign())
                }
                Authority::Outside => false,
            };
            holds.then(|| wallet.address()).flatten()
        })
        .collect()
}
