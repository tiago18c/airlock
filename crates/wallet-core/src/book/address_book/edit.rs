//! Adding, labelling, tagging and removing entries.

use super::*;

impl AddressBook {
    pub fn seed_programs(&mut self) {
        self.upsert_program(SYSTEM_PROGRAM_ID, "System Program", ["system"]);
        self.upsert_program(STAKE_PROGRAM_ID, "Stake Program", ["stake"]);
        self.upsert_program(TOKEN_PROGRAM_ID, "Token Program", ["token"]);
        self.upsert_program(TOKEN_2022_PROGRAM_ID, "Token-2022 Program", ["token"]);
        self.upsert_program(
            ASSOCIATED_TOKEN_PROGRAM_ID,
            "Associated Token Program",
            ["token", "ata"],
        );
    }

    pub(super) fn upsert_program(
        &mut self,
        pubkey: Pubkey,
        label: &str,
        tags: impl IntoIterator<Item = &'static str>,
    ) {
        let key = pubkey.to_string();
        self.local.entries.insert(
            key,
            AddressEntry {
                pubkey,
                label: label.to_string(),
                tags: tags.into_iter().map(str::to_string).collect(),
                kind: AddressKind::Program,
                origin: Origin::Imported,
                owner: None,
                mint: None,
                wallet_id: None,
                hidden: false,
                lamports: None,
                token_amount: None,
                decimals: None,
                nonce_hash: None,
                watched: false,
            },
        );
    }

    pub fn upsert(&mut self, entry: AddressEntry) -> &AddressEntry {
        let key = entry.pubkey.to_string();
        self.local.entries.insert(key.clone(), entry);
        self.local.entries.get(&key).expect("just inserted")
    }

    /// Name a plain account. `wallet_id` ties it to a wallet whose key this
    /// device holds.
    pub fn insert_account(
        &mut self,
        pubkey: Pubkey,
        label: impl Into<String>,
        origin: Origin,
        wallet_id: Option<WalletId>,
    ) -> &AddressEntry {
        self.insert_kind(pubkey, label, AddressKind::Account, origin, wallet_id)
    }

    /// Name an account typed in by hand.
    pub fn insert_contact(&mut self, pubkey: Pubkey, label: impl Into<String>) -> &AddressEntry {
        self.insert_account(pubkey, label, Origin::Imported, None)
    }

    /// Add an address read from the chain, unnamed, unless the book already
    /// has it. Says whether it was added.
    pub fn insert_discovered(&mut self, pubkey: Pubkey, kind: AddressKind) -> bool {
        if self.get(&pubkey).is_some() {
            return false;
        }
        self.insert_kind(pubkey, "", kind, Origin::Discovered, None);
        true
    }

    /// Name an address of any kind.
    pub fn insert_kind(
        &mut self,
        pubkey: Pubkey,
        label: impl Into<String>,
        kind: AddressKind,
        origin: Origin,
        wallet_id: Option<WalletId>,
    ) -> &AddressEntry {
        self.upsert(AddressEntry {
            pubkey,
            label: label.into(),
            tags: BTreeSet::new(),
            kind,
            origin,
            owner: None,
            mint: None,
            wallet_id,
            hidden: false,
            lamports: None,
            token_amount: None,
            decimals: None,
            nonce_hash: None,
            watched: false,
        })
    }

    pub fn set_label(&mut self, pubkey: &Pubkey, label: impl Into<String>) -> bool {
        let Some(group_id) = self.group_id_of(pubkey) else {
            return false;
        };
        self.set_label_in(&group_id, pubkey, label)
    }

    pub fn set_label_in(
        &mut self,
        group_id: &str,
        pubkey: &Pubkey,
        label: impl Into<String>,
    ) -> bool {
        let Some(entry) = self
            .group_mut(group_id)
            .and_then(|group| group.entries.get_mut(&pubkey.to_string()))
        else {
            return false;
        };
        entry.label = label.into();
        true
    }

    pub fn add_tag(&mut self, pubkey: &Pubkey, tag: impl Into<String>) -> bool {
        if let Some(entry) = self.get_mut(pubkey) {
            entry.tags.insert(tag.into());
            true
        } else {
            false
        }
    }

    pub fn remove_tag(&mut self, pubkey: &Pubkey, tag: &str) -> bool {
        if let Some(entry) = self.get_mut(pubkey) {
            entry.tags.remove(tag)
        } else {
            false
        }
    }

    pub fn set_tags(&mut self, pubkey: &Pubkey, tags: impl IntoIterator<Item = String>) -> bool {
        let Some(group_id) = self.group_id_of(pubkey) else {
            return false;
        };
        self.set_tags_in(&group_id, pubkey, tags)
    }

    pub fn set_tags_in(
        &mut self,
        group_id: &str,
        pubkey: &Pubkey,
        tags: impl IntoIterator<Item = String>,
    ) -> bool {
        let Some(entry) = self
            .group_mut(group_id)
            .and_then(|group| group.entries.get_mut(&pubkey.to_string()))
        else {
            return false;
        };
        entry.tags = tags
            .into_iter()
            .filter(|tag| !tag.trim().is_empty())
            .collect();
        true
    }

    pub fn remove(&mut self, pubkey: &Pubkey) -> Option<AddressEntry> {
        let group_id = self.group_id_of(pubkey)?;
        self.remove_in(&group_id, pubkey)
    }

    pub fn remove_in(&mut self, group_id: &str, pubkey: &Pubkey) -> Option<AddressEntry> {
        self.group_mut(group_id)
            .and_then(|group| group.entries.remove(&pubkey.to_string()))
    }
}
