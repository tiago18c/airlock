//! What the app does with a node and a set of wallets.
//!
//! Each function here is one use case: refresh what is known about some
//! addresses, simulate a transaction, prepare a stake operation, create a
//! multisig. They take a [`SolanaRpc`](wallet_core::SolanaRpc) and plain
//! data, and return plain data, so the same code runs behind a screen, in a
//! test with made-up answers, or against an in-process validator.
//!
//! Nothing here draws anything or keeps state between calls.

pub mod adapt;
pub mod cleanup;
pub mod compile;
pub mod device;
mod error;
pub mod history;
pub mod idl;
pub mod loader;
pub mod lookup;
pub mod market;
pub mod migrate;
pub mod offer;
pub mod offline_nonce;
pub mod programmatic;
pub mod proposal_checks;
pub mod refresh;
pub mod request;
pub mod send;
pub mod sign;
pub mod simulate;
pub mod squads;
pub mod stake;
pub mod template;
pub mod token_transfer;
pub mod track;
pub mod transfer;
pub mod vector;

pub mod testing;

pub use error::{FlowError, Prepared};
pub use template::template_facts;
