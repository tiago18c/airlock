//! The wallets of this device: what each one is, what the vault holds, how
//! each signs, and the programs behind the ones that are more than a key.

pub mod adapt;
pub(crate) mod enroll;
pub(crate) mod import;
pub(crate) mod listing;
pub mod offline_nonce;
pub mod programmatic;
pub mod proposals;
pub(crate) mod record;
mod record_keys;
mod record_kind;
#[cfg(test)]
mod record_tests;
pub mod relay;
pub(crate) mod roster;
pub(crate) mod route;
pub mod shared_keys;
pub mod squads;
pub mod stake;
pub(crate) mod vault;
pub mod vector;
mod vector_wallets;

pub use enroll::{enroll, EnrollError};
pub use import::NewSigner;
pub use listing::WalletListStore;
pub use record::{
    Authority, HardwareVendor, KeyPhrase, OfflinePath, Protocol, PublicBytes, VectorSettings,
    WalletClass, WalletKind, WalletRecord,
};
pub use roster::{
    acting_wallet, name_offline_wallet, name_signers, refresh_targets, shareable_accounts,
    signer_of, RosterError, ShareableAccount, WalletSummary,
};
pub use route::{signing_route, RouteError, RouteStep, SigningRoute};
pub use vault::{Vault, VaultData};
