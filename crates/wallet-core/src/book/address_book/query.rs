//! Finding entries: by wallet, by owner, or by what was typed.

use super::*;

impl AddressBook {
    pub fn iter(&self) -> impl Iterator<Item = &AddressEntry> {
        self.local.entries.values().chain(
            self.imported
                .iter()
                .flat_map(|group| group.entries.values()),
        )
    }

    pub fn by_wallet(&self, wallet_id: WalletId) -> impl Iterator<Item = &AddressEntry> {
        self.iter()
            .filter(move |entry| entry.wallet_id == Some(wallet_id))
    }

    pub fn children_of(&self, owner: &Pubkey) -> impl Iterator<Item = &AddressEntry> {
        let owner = *owner;
        self.iter().filter(move |entry| entry.owner == Some(owner))
    }

    /// Filter by substring of address, label, or tag. The local group wins when
    /// the same address appears in an import, so pickers stay unambiguous.
    pub fn query<'a>(&'a self, q: &'a str) -> impl Iterator<Item = &'a AddressEntry> + 'a {
        let needle = q.trim().to_ascii_lowercase();
        let mut seen = BTreeSet::new();
        self.iter().filter(move |entry| {
            if !entry_matches(entry, &needle) {
                return false;
            }
            seen.insert(entry.pubkey)
        })
    }

    /// Every matching address, still grouped. The same pubkey can appear twice.
    pub fn rows(&self, q: &str, kind: Option<AddressKind>) -> Vec<BookRow> {
        let needle = q.trim().to_ascii_lowercase();
        let mut out = Vec::new();
        push_rows(&self.local, false, &needle, kind, &mut out);
        for group in &self.imported {
            push_rows(group, true, &needle, kind, &mut out);
        }
        out
    }

    /// Every matching address in one group.
    pub fn rows_in(&self, group_id: &str, q: &str, kind: Option<AddressKind>) -> Vec<BookRow> {
        let Some(group) = self.group(group_id) else {
            return Vec::new();
        };
        let needle = q.trim().to_ascii_lowercase();
        let mut out = Vec::new();
        let imported = group.id != LOCAL_ID;
        push_rows(group, imported, &needle, kind, &mut out);
        out
    }
}

pub(super) fn entry_matches(entry: &AddressEntry, needle: &str) -> bool {
    if needle.is_empty() {
        return !entry.hidden;
    }
    if entry.hidden {
        return false;
    }
    entry
        .pubkey
        .to_string()
        .to_ascii_lowercase()
        .contains(needle)
        || entry.label.to_ascii_lowercase().contains(needle)
        || entry
            .tags
            .iter()
            .any(|tag| tag.to_ascii_lowercase().contains(needle))
}

pub(super) fn push_rows(
    group: &BookGroup,
    imported: bool,
    needle: &str,
    kind: Option<AddressKind>,
    out: &mut Vec<BookRow>,
) {
    for entry in group.entries.values() {
        if !entry_matches(entry, needle) {
            continue;
        }
        if kind.is_some_and(|kind| entry.kind != kind) {
            continue;
        }
        out.push(BookRow {
            group_id: group.id.clone(),
            group_label: group.label.clone(),
            imported,
            entry: entry.clone(),
        });
    }
}
