//! Derived public views of the stores. The types live in `app-core`, which
//! builds and publishes them; the pump hands them to screens as read-only
//! signals.

use wallet_core::{VaultData, WalletId};

pub use app_core::slices::{NetworkStatus, SyncStatus, VaultState, WalletsView};

/// Next Vector public key stored on wallet `id`, when the vault is unlocked.
pub fn vector_next_pubkey(data: &VaultData, id: WalletId) -> Option<Vec<u8>> {
    data.wallets
        .iter()
        .find(|record| record.id == id)
        .and_then(|record| record.vector_next_pubkey().map(<[u8]>::to_vec))
}

/// Prepared next Vector key on wallet `id`, when the vault is unlocked.
pub fn vector_prepared_next_key(data: &VaultData, id: WalletId) -> Option<Vec<u8>> {
    data.wallets
        .iter()
        .find(|record| record.id == id)
        .and_then(|record| record.vector_prepared_next_key().map(<[u8]>::to_vec))
}
