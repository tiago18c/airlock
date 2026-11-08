//! The Seed Vault of a Solana Mobile phone (Saga, Seeker): seeds kept in
//! the phone's secure hardware, which sign once the user confirms with the
//! phone's own screen and fingerprint. The app never sees a seed.
//!
//! The platform implements [`SeedVault`] (`app-mobile`, through Android's
//! Seed Vault SDK) and hands it over with its [`DeviceHub`](crate::link::DeviceHub).
//! Each call opens a screen of the Seed Vault's own and resolves when the
//! user is done with it.

use crate::link::LinkFuture;

/// How the app gets the use of a seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authorize {
    /// Pick one of the seeds already in the Seed Vault.
    Existing,
    /// Type a seed phrase into the Seed Vault's own screen.
    Import,
    /// Have the Seed Vault make a new seed and show its phrase.
    Create,
}

/// What the Seed Vault refused, or why it could not be asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeedVaultError {
    /// The user backed out of the Seed Vault's screen, or refused.
    Declined,
    /// The app was not allowed to use the Seed Vault.
    NoPermission,
    /// The seed this wallet was authorized for is gone, or its
    /// authorization was withdrawn in the Seed Vault's settings.
    NotAuthorized,
    /// Anything else the platform reported.
    Failed(String),
}

impl std::fmt::Display for SeedVaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declined => write!(f, "The Seed Vault request was declined."),
            Self::NoPermission => write!(
                f,
                "Airlock is not allowed to use the Seed Vault. Allow it in the app's permissions, then try again."
            ),
            Self::NotAuthorized => write!(
                f,
                "The Seed Vault no longer lets Airlock use this seed. Add the wallet again from Seed Vault."
            ),
            Self::Failed(why) => write!(f, "The Seed Vault failed: {why}"),
        }
    }
}

impl std::error::Error for SeedVaultError {}

/// What is signed, which the Seed Vault shows differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Payload {
    /// The bytes a transaction's signatures cover: its message.
    Transaction,
    /// An off-chain message.
    Message,
}

/// A Seed Vault the app may ask. Auth tokens name a seed the user let
/// this app use; they last until the user withdraws them.
pub trait SeedVault: Send + Sync {
    /// Whether the implementation is the real one, in secure hardware, and
    /// not Solana Mobile's simulator for development.
    fn secure(&self) -> bool;

    /// Get the use of a seed, and its auth token.
    fn authorize(&self, how: Authorize) -> LinkFuture<'_, Result<u64, SeedVaultError>>;

    /// The Ed25519 public keys at `paths` (such as `m/44'/501'/0'/0'`) of
    /// the seed `token` names.
    fn public_keys<'a>(
        &'a self,
        token: u64,
        paths: &'a [String],
    ) -> LinkFuture<'a, Result<Vec<[u8; 32]>, SeedVaultError>>;

    /// Sign `payload` with the key at `path`, once the user confirms.
    fn sign<'a>(
        &'a self,
        token: u64,
        path: &'a str,
        payload: &'a [u8],
        kind: Payload,
    ) -> LinkFuture<'a, Result<[u8; 64], SeedVaultError>>;
}
