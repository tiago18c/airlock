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
mod strength;
mod vault;

// With it a vault is sealed under a key anyone could guess. Tests want that, since
// they seal many side by side. Nothing that ships may.
#[cfg(all(feature = "test-hook", not(debug_assertions)))]
compile_error!("the test-hook feature seals vaults under a cheap key: it is not for a release");

#[cfg(feature = "test-hook")]
pub mod testing {
    //! What tests of a caller can look at, and a key cheap enough for them to
    //! make many of. Off in the app.

    use std::cell::{Cell, RefCell};

    thread_local! {
        static WIPED: Cell<usize> = const { Cell::new(0) };
        static ON_DERIVE: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
    }

    pub(crate) fn key_wiped() {
        WIPED.with(|count| count.set(count.get() + 1));
    }

    /// How many open vaults' keys were dropped, and so wiped, on this thread.
    pub fn wiped_keys() -> usize {
        WIPED.with(Cell::get)
    }

    /// Run `then` whenever a key is derived on this thread, while it is, to see
    /// what that holds up.
    pub fn on_derive(then: impl FnMut() + 'static) {
        ON_DERIVE.with(|slot| *slot.borrow_mut() = Some(Box::new(then)));
    }

    pub(crate) fn deriving() {
        let taken = ON_DERIVE.with(|slot| slot.borrow_mut().take());
        if let Some(mut then) = taken {
            then();
            ON_DERIVE.with(|slot| *slot.borrow_mut() = Some(then));
        }
    }
}

pub use error::VaultError;
pub use seal::SealKey;
pub use secret::{SecretBytes, SecretString};
#[cfg(not(target_arch = "wasm32"))]
pub use store::FileStore;
pub use store::{LoadProblem, MemoryStore, VaultStore};
pub use strength::password_problem;
pub use vault::{Document, PasswordChange, Unlocked, Unlocking, Vault};
