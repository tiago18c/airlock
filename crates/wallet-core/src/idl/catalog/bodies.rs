//! Where the IDL files themselves are kept, one per program and network.

use super::*;

/// Loads and saves one program's original IDL bytes for one network.
pub trait IdlBodies: Send {
    fn load(&self, network: IdlNetwork, program_id: &Pubkey)
        -> Result<Option<Vec<u8>>, VaultError>;
    fn save(
        &self,
        network: IdlNetwork,
        program_id: &Pubkey,
        bytes: &[u8],
    ) -> Result<(), VaultError>;
    fn remove(&self, network: IdlNetwork, program_id: &Pubkey) -> Result<(), VaultError>;
    /// A file saved before IDLs were kept per network, under its program alone.
    fn load_unscoped(&self, program_id: &Pubkey) -> Result<Option<Vec<u8>>, VaultError>;
    fn remove_unscoped(&self, program_id: &Pubkey) -> Result<(), VaultError>;
}

#[derive(Default)]
pub struct MemoryBodies {
    pub(super) inner: std::sync::Mutex<BTreeMap<String, Vec<u8>>>,
}

impl MemoryBodies {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(network: IdlNetwork, program_id: &Pubkey) -> String {
        format!("{network}/{program_id}")
    }

    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.lock().expect("idl bodies").get(key).cloned()
    }

    fn drop_key(&self, key: &str) {
        self.inner.lock().expect("idl bodies").remove(key);
    }
}

impl IdlBodies for MemoryBodies {
    fn load(
        &self,
        network: IdlNetwork,
        program_id: &Pubkey,
    ) -> Result<Option<Vec<u8>>, VaultError> {
        Ok(self.get(&Self::key(network, program_id)))
    }

    fn save(
        &self,
        network: IdlNetwork,
        program_id: &Pubkey,
        bytes: &[u8],
    ) -> Result<(), VaultError> {
        self.inner
            .lock()
            .expect("idl bodies")
            .insert(Self::key(network, program_id), bytes.to_vec());
        Ok(())
    }

    fn remove(&self, network: IdlNetwork, program_id: &Pubkey) -> Result<(), VaultError> {
        self.drop_key(&Self::key(network, program_id));
        Ok(())
    }

    fn load_unscoped(&self, program_id: &Pubkey) -> Result<Option<Vec<u8>>, VaultError> {
        Ok(self.get(&program_id.to_string()))
    }

    fn remove_unscoped(&self, program_id: &Pubkey) -> Result<(), VaultError> {
        self.drop_key(&program_id.to_string());
        Ok(())
    }
}

/// `<dir>/<network>/<program id>.json`. Files from before IDLs were kept per
/// network are `<dir>/<program id>.json`.
#[cfg(not(target_arch = "wasm32"))]
pub struct DirIdlBodies {
    pub(super) dir: std::path::PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl DirIdlBodies {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub(super) fn path(&self, network: IdlNetwork, program_id: &Pubkey) -> std::path::PathBuf {
        self.dir
            .join(network.key())
            .join(format!("{program_id}.json"))
    }

    fn unscoped_path(&self, program_id: &Pubkey) -> std::path::PathBuf {
        self.dir.join(format!("{program_id}.json"))
    }

    fn remove_path(path: &std::path::Path) -> Result<(), VaultError> {
        if path.exists() {
            std::fs::remove_file(path).map_err(|err| VaultError::Io(err.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl IdlBodies for DirIdlBodies {
    fn load(
        &self,
        network: IdlNetwork,
        program_id: &Pubkey,
    ) -> Result<Option<Vec<u8>>, VaultError> {
        wallet_vault::store::FileStore::new(self.path(network, program_id)).load()
    }

    fn save(
        &self,
        network: IdlNetwork,
        program_id: &Pubkey,
        bytes: &[u8],
    ) -> Result<(), VaultError> {
        wallet_vault::store::FileStore::new(self.path(network, program_id)).save(bytes)
    }

    fn remove(&self, network: IdlNetwork, program_id: &Pubkey) -> Result<(), VaultError> {
        Self::remove_path(&self.path(network, program_id))
    }

    fn load_unscoped(&self, program_id: &Pubkey) -> Result<Option<Vec<u8>>, VaultError> {
        wallet_vault::store::FileStore::new(self.unscoped_path(program_id)).load()
    }

    fn remove_unscoped(&self, program_id: &Pubkey) -> Result<(), VaultError> {
        Self::remove_path(&self.unscoped_path(program_id))
    }
}
