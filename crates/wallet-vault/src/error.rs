//! What can go wrong with a store or the vault.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("vault is locked")]
    Locked,
    #[error("wrong password or corrupt vault")]
    UnlockFailed,
    #[error("vault is not initialized")]
    NotInitialized,
    #[error("unsupported vault version")]
    UnsupportedVersion,
    #[error("This vault was written by another build of the app (format {found}; this build reads format {expected}) and cannot be opened here. The file has not been changed. Open it with the build that made it, or move it aside to start a new vault.")]
    OtherFormat { found: u32, expected: u32 },
    #[error("I/O error: {0}")]
    Io(String),
    #[error("serialize error: {0}")]
    Serde(String),
    #[error("crypto error: {0}")]
    Crypto(String),
}
