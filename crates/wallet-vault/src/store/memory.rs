//! A store that forgets everything when it is dropped.

use super::VaultStore;
use crate::VaultError;

#[derive(Default)]
pub struct MemoryStore {
    inner: std::sync::Mutex<Option<Vec<u8>>>,
    backup: std::sync::Mutex<Option<Vec<u8>>>,
    aside: std::sync::Mutex<Option<Vec<u8>>>,
    refusing: std::sync::atomic::AtomicBool,
    refusing_reads: std::sync::atomic::AtomicBool,
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

    /// Fail every read from now on, or stop failing them. For testing what
    /// happens when what is stored cannot be read for a while.
    pub fn refuse_reads(&self, refuse: bool) {
        self.refusing_reads
            .store(refuse, std::sync::atomic::Ordering::SeqCst);
    }

    /// Replace what is stored and leave the backup alone, as damage on disk
    /// would.
    pub fn damage(&self, bytes: &[u8]) {
        *self.inner.lock().expect("memory store") = Some(bytes.to_vec());
    }

    /// What was set aside last.
    pub fn set_aside_bytes(&self) -> Option<Vec<u8>> {
        self.aside.lock().expect("memory store").clone()
    }

    fn check_read(&self) -> Result<(), VaultError> {
        if self
            .refusing_reads
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(VaultError::Io("the store refused the read".into()));
        }
        Ok(())
    }
}

impl VaultStore for MemoryStore {
    fn load(&self) -> Result<Option<Vec<u8>>, VaultError> {
        self.check_read()?;
        Ok(self.inner.lock().expect("memory store").clone())
    }

    fn save(&self, bytes: &[u8]) -> Result<(), VaultError> {
        if self.refusing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(VaultError::Io("the store refused the save".into()));
        }
        let previous = self
            .inner
            .lock()
            .expect("memory store")
            .replace(bytes.to_vec());
        *self.backup.lock().expect("memory store") = previous;
        Ok(())
    }

    fn set_aside(&self, bytes: &[u8]) -> Result<(), VaultError> {
        *self.aside.lock().expect("memory store") = Some(bytes.to_vec());
        Ok(())
    }

    fn load_backup(&self) -> Result<Option<Vec<u8>>, VaultError> {
        self.check_read()?;
        Ok(self.backup.lock().expect("memory store").clone())
    }

    fn discard_backup(&self) -> Result<(), VaultError> {
        *self.backup.lock().expect("memory store") = None;
        Ok(())
    }
}
