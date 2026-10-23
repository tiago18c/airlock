//! Turn what the user pasted into signers ready to store.

use crate::book::address_book::Origin;
use crate::error::DeriveError;
use crate::keys::derive::{secret_seed_from_import, DerivedAccount};
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::wallets::record::WalletRecord;
use solana_pubkey::Pubkey;

/// A signer to store: the vault record and the address-book entry that names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSigner {
    pub wallet: WalletRecord,
    pub pubkey: Pubkey,
    pub label: String,
    pub origin: Origin,
}

impl NewSigner {
    /// A pasted private key: base58, hex, or a JSON keypair array.
    pub fn from_private_key(label: &str, raw: &str) -> Result<Self, DeriveError> {
        let seed = secret_seed_from_import(raw)?;
        let signer =
            KeypairSigner::from_seed(&seed).map_err(|err| DeriveError::Key(err.to_string()))?;
        Ok(Self {
            wallet: WalletRecord::from_keypair_seed(label, seed),
            pubkey: signer.pubkey(),
            label: label.to_string(),
            origin: Origin::Imported,
        })
    }

    /// One signer per chosen account of a seed phrase. A single account keeps
    /// `label`; several get their index appended.
    pub fn from_seed_accounts(
        label: &str,
        phrase: &str,
        passphrase: &str,
        accounts: &[DerivedAccount],
    ) -> Vec<Self> {
        accounts
            .iter()
            .map(|account| {
                let label = if accounts.len() == 1 {
                    label.to_string()
                } else {
                    format!("{label} {}", account.index)
                };
                Self {
                    wallet: WalletRecord::from_seed_phrase(
                        label.clone(),
                        phrase,
                        Some(passphrase.to_string()),
                        account.path.clone(),
                        account.pubkey,
                    ),
                    pubkey: account.pubkey,
                    label,
                    origin: Origin::Derived,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::derive::{derive_batch, SOLANA_PATH_TEMPLATE};

    const ABANDON_24: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

    #[test]
    fn a_private_key_becomes_one_signer_that_signs_for_its_address() {
        let key = KeypairSigner::new_random();
        let hex = hex::encode(key.seed_bytes());
        let signer = NewSigner::from_private_key("Hot", &hex).unwrap();
        assert_eq!(signer.pubkey, key.pubkey());
        assert_eq!(signer.label, "Hot");
        assert_eq!(signer.origin, Origin::Imported);
        assert_eq!(signer.wallet.signer().unwrap().pubkey(), key.pubkey());
        assert!(NewSigner::from_private_key("Hot", "not a key").is_err());
    }

    #[test]
    fn seed_accounts_are_labelled_by_index_only_when_there_are_several() {
        let accounts = derive_batch(ABANDON_24, "", SOLANA_PATH_TEMPLATE, 0, 3).unwrap();

        let one = NewSigner::from_seed_accounts("Cold", ABANDON_24, "", &accounts[..1]);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].label, "Cold");

        let several = NewSigner::from_seed_accounts("Cold", ABANDON_24, "", &accounts[1..]);
        let labels: Vec<&str> = several.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Cold 1", "Cold 2"]);
        for (signer, account) in several.iter().zip(&accounts[1..]) {
            assert_eq!(signer.origin, Origin::Derived);
            assert_eq!(signer.wallet.signer().unwrap().pubkey(), account.pubkey);
        }
    }
}
