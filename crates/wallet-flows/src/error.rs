//! How a use case fails, and what it hands back when it does not.

use thiserror::Error;
use wallet_core::RpcError;

/// Why a use case did not finish.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FlowError {
    /// The node could not be asked, or did not answer.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error(
        "Nothing on this device can pay fees for this wallet. Pick a hot wallet for it on Wallets."
    )]
    NoFeePayer,
    /// What was asked for cannot be built. The text says why.
    #[error("{0}")]
    Refused(String),
}

impl FlowError {
    /// A failure of a step that has its own error type, in that type's words.
    pub fn refused(err: impl std::fmt::Display) -> Self {
        Self::Refused(err.to_string())
    }
}

/// A transaction ready for the Sign screen, and what to say it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub wire: Vec<u8>,
    /// What it does, naming the accounts it touches.
    pub note: wallet_core::Phrase,
}
