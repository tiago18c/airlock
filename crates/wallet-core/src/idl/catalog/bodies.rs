//! Where the IDL files themselves are kept, one per program.

use super::*;

/// Loads and saves one program's original IDL bytes.
pub trait IdlBodies: Send {
    fn load(&self, program_id: &Pubkey) -> Result<Option<Vec<u8>>, VaultError>;
    fn save(&self, program_id: &Pubkey, bytes: &[u8]) -> Result<(), VaultError>;
    fn remove(&self, program_id: &Pubkey) -> Result<(), VaultError>;
}

#[derive(Default)]
pub struct MemoryBodies {
    pub(super) inner: std::sync::Mutex<BTreeMap<String, Vec<u8>>>,
}

impl MemoryBodies {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdlBodies for MemoryBodies {
    fn load(&self, program_id: &Pubkey) -> Result<Option<Vec<u8>>, VaultError> {
        Ok(self
            .inner
            .lock()
            .expect("idl bodies")
            .get(&program_id.to_string())
            .cloned())
    }

    fn save(&self, program_id: &Pubkey, bytes: &[u8]) -> Result<(), VaultError> {
        self.inner
            .lock()
            .expect("idl bodies")
            .insert(program_id.to_string(), bytes.to_vec());
        Ok(())
    }

    fn remove(&self, program_id: &Pubkey) -> Result<(), VaultError> {
        self.inner
            .lock()
            .expect("idl bodies")
            .remove(&program_id.to_string());
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct DirIdlBodies {
    pub(super) dir: std::path::PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl DirIdlBodies {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub(super) fn path(&self, program_id: &Pubkey) -> std::path::PathBuf {
        self.dir.join(format!("{program_id}.json"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl IdlBodies for DirIdlBodies {
    fn load(&self, program_id: &Pubkey) -> Result<Option<Vec<u8>>, VaultError> {
        wallet_vault::store::FileStore::new(self.path(program_id)).load()
    }

    fn save(&self, program_id: &Pubkey, bytes: &[u8]) -> Result<(), VaultError> {
        wallet_vault::store::FileStore::new(self.path(program_id)).save(bytes)
    }

    fn remove(&self, program_id: &Pubkey) -> Result<(), VaultError> {
        let path = self.path(program_id);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|err| VaultError::Io(err.to_string()))?;
        }
        Ok(())
    }
}
