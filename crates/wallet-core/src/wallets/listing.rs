//! The wallets as the screens show them while the vault is locked: every
//! record with its secrets left out, in a plain file beside the vault.
//!
//! It is a copy. Each time the vault is open and changes, the copy is
//! written again from it. Nothing signs from it, and nothing in it goes back
//! into the vault but the wallet chosen while locked.

use crate::error::VaultError;
use crate::ids::WalletId;
use crate::wallets::vault::VaultData;
use wallet_vault::store::VaultStore;

pub struct WalletListStore<S: VaultStore> {
    store: S,
    data: Option<VaultData>,
}

impl<S: VaultStore> WalletListStore<S> {
    /// A missing or unreadable copy is no copy: the next unlock writes it.
    pub fn open(store: S) -> Self {
        let data = store
            .load()
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Self { store, data }
    }

    /// The wallets as last copied from the vault, when they were.
    pub fn data(&self) -> Option<&VaultData> {
        self.data.as_ref()
    }

    /// Copy `vault` without its secrets, and save it when it changed.
    pub fn keep(&mut self, vault: &VaultData) -> Result<(), VaultError> {
        let copy = vault.without_secrets();
        let same = self
            .data
            .as_ref()
            .is_some_and(|kept| kept.wallets == copy.wallets && kept.selected == copy.selected);
        if same {
            return Ok(());
        }
        self.data = Some(copy);
        self.save()
    }

    /// Act as `id` while the vault is locked. The vault takes the choice
    /// when it is unlocked next.
    pub fn select(&mut self, id: WalletId) -> Result<(), VaultError> {
        let Some(data) = self.data.as_mut() else {
            return Ok(());
        };
        if !data.wallets.iter().any(|wallet| wallet.id == id) {
            return Ok(());
        }
        let previous = data.selected.replace(id);
        let saved = self.save();
        if saved.is_err() {
            if let Some(data) = self.data.as_mut() {
                data.selected = previous;
            }
        }
        saved
    }

    fn save(&self) -> Result<(), VaultError> {
        let Some(data) = &self.data else {
            return Ok(());
        };
        let json = serde_json::to_vec_pretty(data).map_err(|e| VaultError::Serde(e.to_string()))?;
        self.store.save(&json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallets::record::{Authority, WalletRecord};
    use crate::wallets::vector::SchemeId;
    use solana_pubkey::Pubkey;
    use std::sync::Arc;
    use wallet_vault::{MemoryStore, SecretBytes};

    const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn vault() -> VaultData {
        let hot = WalletRecord::from_keypair_seed("Hot", [0xab; 32]);
        let seed = WalletRecord::from_seed_phrase(
            "Seed",
            PHRASE,
            Some("pass".into()),
            "m/44'/501'/0'/0'",
            Pubkey::new_from_array([3; 32]),
        );
        let vector = WalletRecord::vector(
            "Quantum",
            SchemeId::MlDsa44,
            Pubkey::new_from_array([2; 32]),
            vec![9; 1312],
            Authority::Held {
                secret: SecretBytes(vec![0xcd; 32]),
                next_leaf: 0,
            },
        );
        VaultData {
            selected: Some(seed.id),
            wallets: vec![hot, seed, vector],
        }
    }

    #[test]
    fn the_copy_has_every_wallet_and_address_and_no_secret() {
        let data = vault();
        let store = Arc::new(MemoryStore::new());
        let mut list = WalletListStore::open(store.clone());
        list.keep(&data).unwrap();

        let text = String::from_utf8(store.load().unwrap().unwrap()).unwrap();
        assert!(
            !text.contains("abandon") && !text.contains("pass"),
            "{text}"
        );
        let reopened = WalletListStore::open(store);
        let copy = reopened.data().unwrap();
        assert_eq!(copy.summaries(), data.summaries());
        assert_eq!(copy.local_signer_pubkeys(), data.local_signer_pubkeys());
        assert_eq!(copy.selected, data.selected);
        for wallet in &copy.wallets {
            assert!(wallet.signer().is_err());
        }
        let held = copy.wallets[2].kind.vector_settings().unwrap().authority;
        assert!(matches!(held, Authority::Held { secret, .. } if secret.0.is_empty()));
    }

    #[test]
    fn a_choice_made_while_locked_is_kept_for_a_wallet_in_the_copy() {
        let data = vault();
        let store = Arc::new(MemoryStore::new());
        let mut list = WalletListStore::open(store.clone());
        list.keep(&data).unwrap();
        let hot = data.wallets[0].id;
        list.select(hot).unwrap();
        list.select(WalletId::new()).unwrap();
        assert_eq!(
            WalletListStore::open(store).data().unwrap().selected,
            Some(hot)
        );
    }

    #[test]
    fn an_unreadable_copy_is_no_copy() {
        let store = MemoryStore::new();
        store.save(b"not json").unwrap();
        assert!(WalletListStore::open(store).data().is_none());
    }
}
