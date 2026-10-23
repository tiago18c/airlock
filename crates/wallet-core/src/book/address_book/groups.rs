//! Groups: this device's own addresses, and one group per imported file.

use super::*;

impl AddressBook {
    pub fn groups(&self) -> Vec<BookGroupInfo> {
        let mut out = vec![BookGroupInfo {
            id: self.local.id.clone(),
            label: None,
            imported: false,
            entries: self.local.entries.len(),
        }];
        for group in &self.imported {
            out.push(BookGroupInfo {
                id: group.id.clone(),
                label: Some(group.label.clone()),
                imported: true,
                entries: group.entries.len(),
            });
        }
        out
    }

    pub fn place(&self, pubkey: &Pubkey) -> Option<AddressPlace> {
        let key = pubkey.to_string();
        if let Some(entry) = self.local.entries.get(&key) {
            return Some(AddressPlace {
                group_id: self.local.id.clone(),
                imported: false,
                group_label: None,
                entry: entry.clone(),
            });
        }
        for group in &self.imported {
            if let Some(entry) = group.entries.get(&key) {
                return Some(AddressPlace {
                    group_id: group.id.clone(),
                    imported: true,
                    group_label: Some(group.label.clone()),
                    entry: entry.clone(),
                });
            }
        }
        None
    }

    /// Rename an imported group. The local group has no label.
    pub fn rename_group(&mut self, id: &str, label: impl Into<String>) -> bool {
        if id == self.local.id {
            return false;
        }
        let label = label.into();
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return false;
        }
        let Some(group) = self.imported.iter_mut().find(|group| group.id == id) else {
            return false;
        };
        group.label = trimmed.to_string();
        true
    }

    pub(super) fn group(&self, id: &str) -> Option<&BookGroup> {
        if self.local.id == id {
            return Some(&self.local);
        }
        self.imported.iter().find(|group| group.id == id)
    }

    pub(super) fn group_mut(&mut self, id: &str) -> Option<&mut BookGroup> {
        if self.local.id == id {
            return Some(&mut self.local);
        }
        self.imported.iter_mut().find(|group| group.id == id)
    }

    pub(super) fn group_id_of(&self, pubkey: &Pubkey) -> Option<String> {
        let key = pubkey.to_string();
        if self.local.entries.contains_key(&key) {
            return Some(self.local.id.clone());
        }
        self.imported
            .iter()
            .find(|group| group.entries.contains_key(&key))
            .map(|group| group.id.clone())
    }

    /// Drop an imported group. The local group stays.
    pub fn remove_group(&mut self, id: &str) -> bool {
        if id == self.local.id {
            return false;
        }
        let before = self.imported.len();
        self.imported.retain(|group| group.id != id);
        self.imported.len() != before
    }
}

pub(super) fn new_group_id() -> String {
    let mut bytes = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
