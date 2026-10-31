//! The Add wallet modal.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AddWalletMode {
    PrivateKey,
    Seed,
    Offline,
    Hardware,
    Programmatic,
    Vector,
    Squads,
    /// Wallets another device exported by their public keys.
    FromDevice,
}

impl AddWalletMode {
    /// The modal's own Label, secret and Save controls apply.
    pub fn imports_a_key(self) -> bool {
        matches!(self, Self::PrivateKey | Self::Seed)
    }

    /// The kinds of wallet the app can make from nothing.
    pub fn can_create(self) -> bool {
        matches!(
            self,
            Self::Seed | Self::Programmatic | Self::Vector | Self::Squads
        )
    }
}

#[derive(Clone, Copy)]
pub struct WalletForm {
    pub label: Signal<String>,
    pub secret: Signal<String>,
    pub passphrase: Signal<String>,
    pub path_template: Signal<String>,
    pub mode: Signal<AddWalletMode>,
    /// Make a new wallet, instead of bringing in one that exists.
    pub creating: Signal<bool>,
    pub error: Signal<Option<String>>,
    pub modal_open: Signal<bool>,
}

impl WalletForm {
    pub fn clear(&self) {
        let mut label = self.label;
        let mut secret = self.secret;
        let mut passphrase = self.passphrase;
        let mut path_template = self.path_template;
        let mut mode = self.mode;
        let mut creating = self.creating;
        let mut error = self.error;
        label.set(String::new());
        secret.write().zeroize();
        passphrase.write().zeroize();
        path_template.set(SOLANA_PATH_TEMPLATE.to_string());
        mode.set(AddWalletMode::PrivateKey);
        creating.set(false);
        error.set(None);
    }

    /// The vault locked: wipe what is being typed or shown, and close the
    /// modal, which wipes what its own forms hold.
    pub fn forget_secrets(&self) {
        self.clear();
        let mut modal_open = self.modal_open;
        modal_open.set(false);
    }
}
