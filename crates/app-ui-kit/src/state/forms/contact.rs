//! The address book: its filter, and the entry being added or edited.

use super::*;

#[derive(Clone, Copy)]
pub struct ContactForm {
    pub filter: Signal<String>,
    pub kind: Signal<Option<wallet_core::Group>>,
    /// What an address being added is.
    pub new_kind: Signal<AddressKind>,
    pub pubkey: Signal<String>,
    pub label: Signal<String>,
    pub tags: Signal<String>,
    pub error: Signal<Option<String>>,
    pub modal_open: Signal<bool>,
    pub editing: Signal<Option<Pubkey>>,
    pub editing_group: Signal<String>,
    pub group: Signal<String>,
    pub page: Signal<usize>,
}

impl ContactForm {
    pub fn clear_draft(&self) {
        let mut pubkey = self.pubkey;
        let mut label = self.label;
        let mut tags = self.tags;
        let mut error = self.error;
        let mut editing = self.editing;
        let mut editing_group = self.editing_group;
        let mut new_kind = self.new_kind;
        new_kind.set(AddressKind::Account);
        pubkey.set(String::new());
        label.set(String::new());
        tags.set(String::new());
        error.set(None);
        editing.set(None);
        editing_group.set(String::new());
    }
}
