//! A store that forgets everything when it is dropped.

use super::VaultStore;
use crate::VaultError;

#[derive(Default)]
pub struct MemoryStore {
    inner: std::sync::Mutex<Option<Vec<u8>>>,
    aside: std::sync::Mutex<Option<Vec<u8>>>,
    refusing: std::sync::atomic::AtomicBool,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fail every save from now on, or stop failing them. For testing what
    /// is kept when a save does not happen.
    pub fn refuse_saves(&self, refuse: bool) {
        self.refusing
            .store(refuse, std::sync::atomic::Ordering::SeqCst);
    }
}

impl VaultStore for MemoryStore {
    fn load(&self) -> Result<Option<Vec<u8>>, VaultError> {
        Ok(self.inner.lock().expect("memory store").clone())
    }

    fn save(&self, bytes: &[u8]) -> Result<(), VaultError> {
        if self.refusing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(VaultError::Io("the store refused the save".into()));
        }
        *self.inner.lock().expect("memory store") = Some(bytes.to_vec());
        Ok(())
    }

    fn set_aside(&self, bytes: &[u8]) -> Result<(), VaultError> {
        *self.aside.lock().expect("memory store") = Some(bytes.to_vec());
        Ok(())
    }
}
