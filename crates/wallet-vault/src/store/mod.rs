//! Where bytes are kept: a file, a browser's storage, or memory in a test.

mod json;
mod memory;

#[cfg(not(target_arch = "wasm32"))]
mod file;

#[cfg(not(target_arch = "wasm32"))]
pub use file::FileStore;
pub use json::{check_saveable, load_json, load_json_or_block, LoadProblem};
pub use memory::MemoryStore;

use crate::VaultError;

pub trait VaultStore: Send + Sync {
    fn load(&self) -> Result<Option<Vec<u8>>, VaultError>;
    fn save(&self, bytes: &[u8]) -> Result<(), VaultError>;

    /// Keep a copy of bytes that could not be parsed, so the next save does not
    /// destroy them. A store that cannot do this leaves the data unsaveable.
    fn set_aside(&self, _bytes: &[u8]) -> Result<(), VaultError> {
        Err(VaultError::Io("this store cannot keep a copy".into()))
    }
}

/// A store shared with whoever else holds the handle.
impl<S: VaultStore + ?Sized> VaultStore for std::sync::Arc<S> {
    fn load(&self) -> Result<Option<Vec<u8>>, VaultError> {
        (**self).load()
    }

    fn save(&self, bytes: &[u8]) -> Result<(), VaultError> {
        (**self).save(bytes)
    }

    fn set_aside(&self, bytes: &[u8]) -> Result<(), VaultError> {
        (**self).set_aside(bytes)
    }
}
