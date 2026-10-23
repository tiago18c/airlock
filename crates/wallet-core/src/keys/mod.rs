//! Keys: deriving them from a seed phrase, reading a pasted one, and signing.

pub(crate) mod derive;
pub(crate) mod signer;

pub use derive::{
    concrete_path, derive_batch, new_phrase, path_has_wildcard, secret_seed_from_import,
    DerivedAccount, DERIVE_BATCH, SOLANA_PATH_TEMPLATE,
};
pub use signer::{KeypairSigner, WalletSigner};
