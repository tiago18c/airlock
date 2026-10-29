//! The key behind a wallet record, and the address the wallet is known by.

use crate::error::SignError;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::wallets::programmatic::programmatic_signer_address;
use crate::wallets::record::{Authority, KeyPhrase, Protocol, WalletKind, WalletRecord};
use crate::wallets::vector;
use solana_pubkey::Pubkey;
use wallet_vault::{SecretBytes, SecretString};

impl WalletRecord {
    pub fn signer(&self) -> Result<KeypairSigner, SignError> {
        match &self.kind {
            WalletKind::Keypair { secret_seed, .. } => KeypairSigner::from_seed(&secret_seed.0),
            WalletKind::Seed {
                mnemonic,
                passphrase,
                path,
                ..
            } => {
                let pass = passphrase
                    .as_ref()
                    .map(|value| value.0.as_str())
                    .unwrap_or("");
                let path = if path.is_empty() {
                    "m/44'/501'/0'/0'"
                } else {
                    path.as_str()
                };
                crate::keys::derive::keypair_at(&mnemonic.0, pass, path)
                    .map(KeypairSigner::from_keypair)
                    .map_err(|err| SignError::Message(err.to_string()))
            }
            WalletKind::Offline { .. }
            | WalletKind::Hardware { .. }
            | WalletKind::Programmatic { .. }
            | WalletKind::Squads { .. } => Err(SignError::CannotSign),
        }
    }

    /// The address this wallet is known by: where its funds are and what a
    /// program sees acting. A hardware wallet has none until it is read.
    pub fn address(&self) -> Option<Pubkey> {
        match &self.kind {
            WalletKind::Offline { pubkey, .. } => Some(*pubkey),
            WalletKind::Squads { vault, .. } => Some(*vault),
            WalletKind::Programmatic { protocol, .. } => match protocol {
                Protocol::Ed25519Signer { authority, .. } => {
                    Some(programmatic_signer_address(authority))
                }
                Protocol::Vector {
                    scheme,
                    program,
                    key,
                    ..
                } => vector::address(program, *scheme, &key.0).ok(),
            },
            WalletKind::Keypair { pubkey, .. } | WalletKind::Seed { pubkey, .. } => {
                pubkey.or_else(|| self.signer().ok().map(|signer| signer.pubkey()))
            }
            WalletKind::Hardware { .. } => None,
        }
    }

    /// The same record with no secret in it, for the list kept outside the
    /// vault: keys, seed phrases and prepared next keys are left empty, and
    /// a hot wallet's address is written in, since it can no longer be
    /// derived. It cannot sign.
    pub fn without_secrets(&self) -> WalletRecord {
        let address = self.address();
        let mut copy = self.clone();
        // Every field is named, with no `..`: a field added to any kind has
        // to be sorted here into secret or not before this builds.
        match &mut copy.kind {
            WalletKind::Keypair {
                secret_seed,
                pubkey,
            } => {
                *secret_seed = SecretBytes(Vec::new());
                *pubkey = address;
            }
            WalletKind::Seed {
                mnemonic,
                passphrase,
                path: _,
                pubkey,
            } => {
                *mnemonic = SecretString(String::new());
                *passphrase = None;
                *pubkey = address;
            }
            WalletKind::Programmatic {
                protocol,
                authority,
                fee_payer: _,
            } => {
                match protocol {
                    Protocol::Ed25519Signer {
                        authority: _,
                        nonce: _,
                    } => {}
                    Protocol::Vector {
                        scheme: _,
                        program: _,
                        key: _,
                        next_pubkey: _,
                        next_secret,
                        retired_secret,
                        phrase,
                    } => {
                        *next_secret = None;
                        *retired_secret = None;
                        // The number of the key and whether it was restored
                        // are no secret; the words are.
                        if let Some(KeyPhrase {
                            words,
                            index: _,
                            restored: _,
                        }) = phrase
                        {
                            *words = SecretString(String::new());
                        }
                    }
                }
                match authority {
                    Authority::Held {
                        secret,
                        next_leaf: _,
                    } => *secret = SecretBytes(Vec::new()),
                    Authority::Wallet { wallet: _ } | Authority::Outside => {}
                }
            }
            WalletKind::Offline {
                pubkey: _,
                fee_payer: _,
                durable_nonce: _,
                default_path: _,
            }
            | WalletKind::Hardware {
                vendor: _,
                paths: _,
            }
            | WalletKind::Squads {
                program: _,
                multisig: _,
                vault: _,
                members: _,
                threshold: _,
                member_wallet: _,
                approves_with: _,
                approval_nonce: _,
                fee_payer: _,
            } => {}
        }
        copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vector_wallet_is_known_by_its_account_and_holds_no_key_of_that_address() {
        use crate::wallets::record::Authority;
        use crate::wallets::vector::SchemeId;
        let program = Pubkey::new_from_array([2; 32]);
        let wallet = WalletRecord::vector(
            "Vector",
            SchemeId::Ed25519,
            program,
            vec![7; 32],
            Authority::Outside,
        );
        assert_eq!(
            wallet.address(),
            vector::address(&program, SchemeId::Ed25519, &[7; 32]).ok()
        );
        assert!(matches!(wallet.signer(), Err(SignError::CannotSign)));
        // A key of the wrong length names no account, so the record has no address.
        let broken = WalletRecord::vector(
            "Vector",
            SchemeId::Ed25519,
            program,
            vec![7; 31],
            Authority::Outside,
        );
        assert_eq!(broken.address(), None);
    }

    #[test]
    fn a_hot_wallet_signs_with_its_key_and_an_offline_one_cannot() {
        let signer = KeypairSigner::new_random();
        let wallet = WalletRecord::from_keypair_seed("hot", signer.seed_bytes());
        assert_eq!(wallet.signer().unwrap().pubkey(), signer.pubkey());
        assert_eq!(wallet.address(), Some(signer.pubkey()));
        assert!(matches!(
            WalletRecord::offline("cold", Pubkey::new_from_array([1; 32])).signer(),
            Err(SignError::CannotSign)
        ));
    }
}
