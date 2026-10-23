//! The book as a file, to hand to another device.

use super::*;

impl AddressBook {
    /// Pretty JSON of this device's addresses, without the seeded programs.
    /// `label` is the name the other machine will show; it is not stored here.
    pub fn export_json(&self, label: &str) -> Result<String, BookFileError> {
        let trimmed = label.trim();
        let label = if trimmed.is_empty() {
            "Imported".to_string()
        } else {
            trimmed.to_string()
        };
        let file = AddressBookFile {
            label,
            entries: self
                .local
                .entries
                .values()
                .filter(|entry| entry.kind != AddressKind::Program)
                .cloned()
                .collect(),
        };
        serde_json::to_string_pretty(&file).map_err(|err| BookFileError::Encode(err.to_string()))
    }

    /// Append `json` as its own group. Labels inside the file are kept.
    pub fn import_json(&mut self, json: &str) -> Result<String, BookFileError> {
        let file: AddressBookFile =
            serde_json::from_str(json).map_err(|err| BookFileError::NotABook(err.to_string()))?;
        let trimmed = file.label.trim();
        let label = if trimmed.is_empty() {
            "Imported".to_string()
        } else {
            trimmed.to_string()
        };
        let mut entries = BTreeMap::new();
        for entry in file.entries {
            if entry.kind == AddressKind::Program {
                continue;
            }
            entries.insert(entry.pubkey.to_string(), entry);
        }
        if entries.is_empty() {
            return Err(BookFileError::Empty);
        }
        self.imported.push(BookGroup {
            id: new_group_id(),
            label: label.clone(),
            entries,
        });
        Ok(label)
    }
}
