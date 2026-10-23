//! What the vault holds: the wallets of this device and which one is in use.

use crate::ids::WalletId;
use crate::wallets::record::WalletRecord;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultData {
    pub wallets: Vec<WalletRecord>,
    /// The wallet the app is currently acting as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<WalletId>,
}

impl VaultData {
    /// Every wallet with no secret in it, and the one chosen.
    pub fn without_secrets(&self) -> VaultData {
        VaultData {
            wallets: self
                .wallets
                .iter()
                .map(WalletRecord::without_secrets)
                .collect(),
            selected: self.selected,
        }
    }
}

/// Format 1 held these fields bare, with a programmatic signer's settings
/// flat in its record and the offline wallet under another name. Format 2 is
/// the first with a number: it gave the programmatic signer a protocol.
/// Format 3 writes secret bytes as base64 instead of a list of numbers.
impl wallet_vault::Document for VaultData {
    const FORMAT: u32 = 3;
}

/// The encrypted store of this device's wallets.
pub type Vault<S> = wallet_vault::Vault<S, VaultData>;

#[cfg(test)]
mod tests {
    use super::*;
    use solana_pubkey::Pubkey;
    use wallet_vault::MemoryStore;

    #[test]
    fn wallets_survive_a_lock_and_nothing_else_is_stored() {
        let mut vault: Vault<_> = Vault::new(MemoryStore::new());
        vault.create("pw").unwrap();
        vault
            .data_mut()
            .unwrap()
            .wallets
            .push(WalletRecord::offline(
                "Bob",
                Pubkey::new_from_array([9; 32]),
            ));
        vault.save().unwrap();
        let json = serde_json::to_string(vault.data().unwrap()).unwrap();
        assert!(json.contains("wallets"));
        assert!(!json.contains("book"));
        vault.lock();
        vault.unlock("pw").unwrap();
        assert_eq!(vault.data().unwrap().wallets[0].label, "Bob");
    }
}
