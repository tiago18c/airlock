//! `localStorage` in a browser.

use solana_pubkey::Pubkey;
use wallet_core::{IdlBodies, IdlNetwork, VaultError, VaultStore};

use super::Storage;

pub struct LocalStorage;

impl Storage for LocalStorage {
    fn open(&self, name: &str) -> Box<dyn VaultStore> {
        Box::new(LocalStorageStore::new(format!("airlock.{name}")))
    }
}

/// One `localStorage` key. Everything the app keeps is JSON, the vault too,
/// so it is stored as the text it is and can be read in the browser's tools.
struct LocalStorageStore {
    key: String,
}

impl LocalStorageStore {
    fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }

    fn storage() -> Result<web_sys::Storage, VaultError> {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .ok_or_else(|| VaultError::Io("localStorage is not available".into()))
    }

    fn remove(&self) -> Result<(), VaultError> {
        Self::storage()?
            .remove_item(&self.key)
            .map_err(|err| VaultError::Io(format!("localStorage remove: {err:?}")))
    }
}

impl VaultStore for LocalStorageStore {
    fn load(&self) -> Result<Option<Vec<u8>>, VaultError> {
        match Self::storage()?.get_item(&self.key) {
            Ok(Some(text)) => Ok(Some(text.into_bytes())),
            Ok(None) => Ok(None),
            Err(err) => Err(VaultError::Io(format!("localStorage read: {err:?}"))),
        }
    }

    fn save(&self, bytes: &[u8]) -> Result<(), VaultError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| VaultError::Io("localStorage holds text, and this is not".into()))?;
        Self::storage()?
            .set_item(&self.key, text)
            .map_err(|e| VaultError::Io(format!("localStorage write: {e:?}")))
    }

    fn set_aside(&self, bytes: &[u8]) -> Result<(), VaultError> {
        LocalStorageStore::new(format!("{}.unreadable", self.key)).save(bytes)
    }
}

/// `airlock.idl.<network>.<program id>`. Files from before IDLs were kept
/// per network are `airlock.idl.<program id>`.
pub struct WebIdlBodies;

impl WebIdlBodies {
    fn store(network: IdlNetwork, program_id: &Pubkey) -> LocalStorageStore {
        LocalStorageStore::new(format!("airlock.idl.{network}.{program_id}"))
    }

    fn unscoped(program_id: &Pubkey) -> LocalStorageStore {
        LocalStorageStore::new(format!("airlock.idl.{program_id}"))
    }
}

impl IdlBodies for WebIdlBodies {
    fn load(
        &self,
        network: IdlNetwork,
        program_id: &Pubkey,
    ) -> Result<Option<Vec<u8>>, VaultError> {
        Self::store(network, program_id).load()
    }

    fn save(
        &self,
        network: IdlNetwork,
        program_id: &Pubkey,
        bytes: &[u8],
    ) -> Result<(), VaultError> {
        Self::store(network, program_id).save(bytes)
    }

    fn remove(&self, network: IdlNetwork, program_id: &Pubkey) -> Result<(), VaultError> {
        Self::store(network, program_id).remove()
    }

    fn load_unscoped(&self, program_id: &Pubkey) -> Result<Option<Vec<u8>>, VaultError> {
        Self::unscoped(program_id).load()
    }

    fn remove_unscoped(&self, program_id: &Pubkey) -> Result<(), VaultError> {
        Self::unscoped(program_id).remove()
    }
}
