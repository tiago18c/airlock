use crate::chain::{DiscoveredNonce, DiscoveredTokenAccount};
use crate::error::BookFileError;
use crate::ids::{
    WalletId, ASSOCIATED_TOKEN_PROGRAM_ID, STAKE_PROGRAM_ID, SYSTEM_PROGRAM_ID,
    TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet};

mod chain_state;
mod edit;
mod file;
mod groups;
mod query;
mod types;

#[cfg(test)]
mod tests;

use groups::*;
pub use types::*;

const LOCAL_ID: &str = "local";

/// Id of the unlabeled group on this device.
pub const LOCAL_GROUP_ID: &str = LOCAL_ID;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressBook {
    local: BookGroup,
    #[serde(default)]
    imported: Vec<BookGroup>,
}

impl Default for AddressBook {
    fn default() -> Self {
        Self::new()
    }
}

impl AddressBook {
    pub fn new() -> Self {
        let mut book = Self {
            local: BookGroup::this_device(),
            imported: Vec::new(),
        };
        book.seed_programs();
        book
    }

    pub fn local_id(&self) -> &str {
        &self.local.id
    }

    pub fn get(&self, pubkey: &Pubkey) -> Option<&AddressEntry> {
        let key = pubkey.to_string();
        self.local.entries.get(&key).or_else(|| {
            self.imported
                .iter()
                .find_map(|group| group.entries.get(&key))
        })
    }

    pub fn get_in(&self, group_id: &str, pubkey: &Pubkey) -> Option<&AddressEntry> {
        self.group(group_id)?.entries.get(&pubkey.to_string())
    }

    pub fn get_mut(&mut self, pubkey: &Pubkey) -> Option<&mut AddressEntry> {
        let key = pubkey.to_string();
        if self.local.entries.contains_key(&key) {
            return self.local.entries.get_mut(&key);
        }
        for group in &mut self.imported {
            if let Some(entry) = group.entries.get_mut(&key) {
                return Some(entry);
            }
        }
        None
    }

    fn touch(&mut self, key: &str, mut f: impl FnMut(&mut AddressEntry)) -> bool {
        let mut found = false;
        if let Some(entry) = self.local.entries.get_mut(key) {
            f(entry);
            found = true;
        }
        for group in &mut self.imported {
            if let Some(entry) = group.entries.get_mut(key) {
                f(entry);
                found = true;
            }
        }
        found
    }

    pub fn user_entry_count(&self) -> usize {
        self.iter()
            .filter(|e| e.kind != AddressKind::Program)
            .count()
    }
}
