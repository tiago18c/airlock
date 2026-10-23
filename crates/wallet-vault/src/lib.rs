//! Keeping secrets at rest.
//!
//! [`Vault`] seals one [`Document`] under a password. A [`store`] is where the
//! sealed bytes, and the app's plain JSON files, are kept. [`SecretBytes`] and
//! [`SecretString`] hold a secret in memory. Nothing here knows what a wallet
//! is: the document a vault holds is the caller's type.

mod error;
mod seal;
mod secret;
pub mod store;
mod vault;

#[cfg(feature = "test-hook")]
pub mod testing {
    //! What tests of a caller can look at after a lock. Off in the app.

    use std::cell::Cell;

    thread_local! {
        static WIPED: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn key_wiped() {
        WIPED.with(|count| count.set(count.get() + 1));
    }

    /// How many open vaults' keys were dropped, and so wiped, on this thread.
    pub fn wiped_keys() -> usize {
        WIPED.with(Cell::get)
    }
}

pub use error::VaultError;
pub use secret::{SecretBytes, SecretString};
#[cfg(not(target_arch = "wasm32"))]
pub use store::FileStore;
pub use store::{LoadProblem, MemoryStore, VaultStore};
pub use vault::{Document, Vault};
