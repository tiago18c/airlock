//! Moving a hash-based Vector account to its next key.
//!
//! An XMSS key runs out of leaves and a Winternitz key signs once, so the
//! account's place in the `Advance` names the key that signs after it. The device
//! that holds the key makes the next one and keeps its seed beside the
//! current one. Another device that builds the transactions is given only
//! the next public key, as a public key object.
//!
//! The seed replaces the current one only once the chain shows the account
//! holding the next key. Before that the current seed is the only key that
//! controls the account.

use crate::ids::WalletId;
use crate::wallets::record::{
    Authority, KeyPhrase, Protocol, PublicBytes, WalletKind, WalletRecord,
};
use crate::wallets::vault::VaultData;
use crate::wallets::vector::{
    find_phrase_key, new_rotating_key, normalize_key_phrase, phrase_key, SchemeId, VectorError,
    PHRASE_SEARCH_AHEAD,
};
use thiserror::Error;
use wallet_vault::SecretBytes;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RotationError {
    #[error("Unlock the vault first.")]
    Locked,
    #[error("That wallet is not on this device.")]
    NoSuchWallet,
    #[error("The vault could not be saved, so nothing changed. {0}")]
    Save(String),
    #[error("This public key is of a scheme this app does not know.")]
    UnknownScheme,
    #[error("A {0} account keeps its key, so it takes no next key.")]
    KeepsItsKey(&'static str),
    #[error("A {scheme} key is {expected} bytes, and this one is {found}.")]
    KeyLength {
        scheme: &'static str,
        expected: usize,
        found: usize,
    },
    #[error("No {0} Vector wallet here has its key on another device. A wallet whose key is kept on this device makes its own next key.")]
    NoWallet(&'static str),
    #[error("More than one {0} Vector wallet here has its key on another device. Act as the one this key belongs to, then scan it again.")]
    Choose(&'static str),
    #[error("{0} does not keep a hash-based key on this device.")]
    NotHeldHere(String),
    #[error("A next key for {0} is already waiting. It is used once the account rotates into it.")]
    AlreadyPrepared(String),
    #[error("No next key made on this device is waiting for {0}.")]
    NothingPending(String),
    #[error("The account of {0} does not hold the next key yet. It is installed once the transaction that replaces the key has landed.")]
    NotLanded(String),
    #[error("{0}")]
    Phrase(String),
    #[error("The account of {0} holds a key this device did not make, and its key here is not made from a recovery phrase.")]
    OtherKey(String),
}

impl From<VectorError> for RotationError {
    fn from(err: VectorError) -> Self {
        Self::Phrase(err.to_string())
    }
}

impl WalletRecord {
    /// A Vector wallet whose hash-based keys are made from `words`, holding
    /// key number 0, which makes the account. `restored` when the phrase was
    /// written down before and is typed in again here: see
    /// [`KeyPhrase::restored`].
    pub fn vector_from_phrase(
        label: impl Into<String>,
        scheme: SchemeId,
        program: solana_pubkey::Pubkey,
        words: &str,
        restored: bool,
    ) -> Result<Self, VectorError> {
        let words = normalize_key_phrase(words)?;
        let (public, seed) = phrase_key(&words, scheme, 0)?;
        let mut wallet = Self::vector(
            label,
            scheme,
            program,
            public,
            Authority::Held {
                secret: SecretBytes(seed.to_vec()),
                next_leaf: 0,
            },
        );
        if let WalletKind::Programmatic {
            protocol: Protocol::Vector { phrase, .. },
            ..
        } = &mut wallet.kind
        {
            *phrase = Some(KeyPhrase {
                words: wallet_vault::SecretString(words),
                index: 0,
                restored,
            });
        }
        Ok(wallet)
    }

    /// The recovery phrase this wallet's keys are made from, when they are.
    pub fn vector_phrase(&self) -> Option<&KeyPhrase> {
        match &self.kind {
            WalletKind::Programmatic {
                protocol:
                    Protocol::Vector {
                        phrase: Some(phrase),
                        ..
                    },
                ..
            } => Some(phrase),
            _ => None,
        }
    }

    /// Take `key` as the key the account holds now, with `floor` the lowest
    /// leaf it still accepts. A key made from this wallet's phrase is found
    /// by its number, and signs from then on. Answers the number when the
    /// key changed.
    pub fn adopt_vector_key(
        &mut self,
        key: &[u8],
        floor: u32,
    ) -> Result<Option<u32>, RotationError> {
        let label = self.label.clone();
        let WalletKind::Programmatic {
            protocol:
                Protocol::Vector {
                    scheme,
                    next_pubkey,
                    next_secret,
                    phrase,
                    ..
                },
            authority: Authority::Held { secret, next_leaf },
            ..
        } = &mut self.kind
        else {
            return Err(RotationError::NotHeldHere(label));
        };
        let Some(phrase) = phrase.as_mut() else {
            return Err(RotationError::OtherKey(label));
        };
        let (current, _) = phrase_key(&phrase.words.0, *scheme, phrase.index)?;
        if current == key {
            *next_leaf = (*next_leaf).max(floor);
            return Ok(None);
        }
        let upto = phrase.index.saturating_add(PHRASE_SEARCH_AHEAD);
        let index = find_phrase_key(&phrase.words.0, *scheme, key, upto)?;
        let (_, seed) = phrase_key(&phrase.words.0, *scheme, index)?;
        *secret = SecretBytes(seed.to_vec());
        *next_leaf = floor;
        *next_pubkey = None;
        *next_secret = None;
        phrase.index = index;
        Ok(Some(index))
    }

    /// The next public key a rotating `Advance` of this wallet names,
    /// when one was made here or given by the device that holds the key.
    pub fn vector_next_pubkey(&self) -> Option<&[u8]> {
        match &self.kind {
            WalletKind::Programmatic {
                protocol:
                    Protocol::Vector {
                        next_pubkey: Some(key),
                        ..
                    },
                ..
            } => Some(&key.0),
            _ => None,
        }
    }

    /// The next public key, when this device made it and holds its seed.
    /// Only a rotation into this key is signed here.
    pub fn vector_prepared_next_key(&self) -> Option<&[u8]> {
        match &self.kind {
            WalletKind::Programmatic {
                protocol:
                    Protocol::Vector {
                        next_pubkey: Some(key),
                        next_secret: Some(_),
                        ..
                    },
                authority: Authority::Held { .. },
                ..
            } => Some(&key.0),
            _ => None,
        }
    }

    /// Make the next key of a hash-based key kept here, and keep its seed
    /// until the rotation lands.
    pub fn prepare_vector_next_key(&mut self) -> Result<(), RotationError> {
        let label = self.label.clone();
        let WalletKind::Programmatic {
            protocol:
                Protocol::Vector {
                    scheme,
                    next_pubkey,
                    next_secret,
                    phrase,
                    ..
                },
            authority: Authority::Held { .. },
            ..
        } = &mut self.kind
        else {
            return Err(RotationError::NotHeldHere(label));
        };
        if next_secret.is_some() {
            return Err(RotationError::AlreadyPrepared(label));
        }
        // A key made from a phrase moves to the phrase's next key, so that it
        // can be made again.
        let (public, secret) = match phrase {
            Some(phrase) => {
                let (public, seed) =
                    phrase_key(&phrase.words.0, *scheme, phrase.index.saturating_add(1))?;
                (public, seed.to_vec())
            }
            None => new_rotating_key(*scheme).ok_or(RotationError::NotHeldHere(label))?,
        };
        *next_pubkey = Some(PublicBytes(public));
        *next_secret = Some(SecretBytes(secret));
        Ok(())
    }

    /// Take the next public key that the device holding this wallet's key
    /// made. A wallet whose key is kept here makes its own, and is refused.
    pub fn accept_vector_next_pubkey(&mut self, key: Vec<u8>) -> Result<(), RotationError> {
        let label = self.label.clone();
        match &mut self.kind {
            WalletKind::Programmatic {
                protocol: Protocol::Vector { next_pubkey, .. },
                authority: Authority::Outside,
                ..
            } => {
                *next_pubkey = Some(PublicBytes(key));
                Ok(())
            }
            _ => Err(RotationError::NotHeldHere(label)),
        }
    }

    /// Forget the next key, and its seed when this device made it.
    pub fn clear_vector_next_key(&mut self) {
        if let WalletKind::Programmatic {
            protocol:
                Protocol::Vector {
                    next_pubkey,
                    next_secret,
                    ..
                },
            ..
        } = &mut self.kind
        {
            *next_pubkey = None;
            *next_secret = None;
        }
    }

    /// Replace the key kept here by the next one, now that the account holds
    /// `current` on chain. For Winternitz, whose every transaction rotates,
    /// the key after it is made at once.
    pub fn install_vector_next_secret(&mut self, current: &[u8]) -> Result<(), RotationError> {
        let label = self.label.clone();
        let WalletKind::Programmatic {
            protocol:
                Protocol::Vector {
                    scheme,
                    next_pubkey,
                    next_secret,
                    retired_secret,
                    phrase,
                    ..
                },
            authority: Authority::Held { secret, next_leaf },
            ..
        } = &mut self.kind
        else {
            return Err(RotationError::NotHeldHere(label));
        };
        let (Some(public), Some(_)) = (next_pubkey.as_ref(), next_secret.as_ref()) else {
            return Err(RotationError::NothingPending(label));
        };
        if public.0 != current {
            return Err(RotationError::NotLanded(label));
        }
        let always = scheme.always_rotates();
        let next = next_secret.take().expect("checked above");
        let before = std::mem::replace(secret, next);
        match phrase {
            // The phrase makes the key before again, so nothing is kept.
            Some(phrase) => {
                phrase.index = phrase.index.saturating_add(1);
                phrase.restored = false;
            }
            None => *retired_secret = Some(before),
        }
        *next_pubkey = None;
        *next_leaf = 0;
        if always {
            self.prepare_vector_next_key()?;
        }
        Ok(())
    }
}

/// The wallet a next public key of scheme `scheme`, scanned from the device
/// that holds the key, belongs to: the only Vector wallet of that scheme
/// whose key is on another device, or the one the app acts as.
pub fn next_key_target(
    wallets: &[WalletRecord],
    selected: Option<WalletId>,
    scheme: u8,
    key: &[u8],
) -> Result<WalletId, RotationError> {
    let scheme = SchemeId::from_byte(scheme).ok_or(RotationError::UnknownScheme)?;
    if !scheme.is_stateful() {
        return Err(RotationError::KeepsItsKey(scheme.label()));
    }
    if key.len() != scheme.key_len() {
        return Err(RotationError::KeyLength {
            scheme: scheme.label(),
            expected: scheme.key_len(),
            found: key.len(),
        });
    }
    let candidates: Vec<WalletId> = wallets
        .iter()
        .filter(|wallet| {
            wallet.kind.vector_settings().is_some_and(|settings| {
                settings.scheme == scheme && matches!(settings.authority, Authority::Outside)
            })
        })
        .map(|wallet| wallet.id)
        .collect();
    match candidates.as_slice() {
        [] => Err(RotationError::NoWallet(scheme.label())),
        [one] => Ok(*one),
        _ => selected
            .filter(|id| candidates.contains(id))
            .ok_or(RotationError::Choose(scheme.label())),
    }
}

impl VaultData {
    /// Store a next public key scanned from the device that holds a wallet's
    /// key, on the wallet it belongs to. Answers that wallet's label.
    pub fn accept_next_public_key(
        &mut self,
        scheme: u8,
        key: Vec<u8>,
    ) -> Result<String, RotationError> {
        let id = next_key_target(&self.wallets, self.acting_wallet(), scheme, &key)?;
        let wallet = self.vector_wallet_mut(id)?;
        wallet.accept_vector_next_pubkey(key)?;
        Ok(wallet.label.clone())
    }

    /// The wallet `id`, to change.
    pub fn vector_wallet_mut(&mut self, id: WalletId) -> Result<&mut WalletRecord, RotationError> {
        self.wallets
            .iter_mut()
            .find(|wallet| wallet.id == id)
            .ok_or(RotationError::NoSuchWallet)
    }
}
