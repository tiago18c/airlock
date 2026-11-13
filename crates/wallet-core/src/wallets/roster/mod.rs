//! The rules of the wallet list: which wallets may be added, what removing
//! one clears, which one the app acts as, and how the screens list them.

use crate::book::address_book::{AddressBook, AddressKind, Origin, LOCAL_GROUP_ID};
use crate::book::workspace::Workspace;
use crate::ids::WalletId;
use crate::wallets::hardware::HardwareVendor;
use crate::wallets::import::NewSigner;
use crate::wallets::record::{
    Authority, OfflinePath, Protocol, WalletClass, WalletKind, WalletRecord,
};
use crate::wallets::vault::VaultData;
use crate::wallets::vector::{self, SchemeId};
use solana_pubkey::Pubkey;

/// A wallet as the screens list it. Never carries a secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletSummary {
    pub id: WalletId,
    pub label: String,
    pub tags: Vec<String>,
    pub class: WalletClass,
    /// "seed · m/44'/501'/0'/0'", "Squads v4 · 2 of 3", and so on.
    pub summary: String,
    pub address: Option<Pubkey>,
    /// The settings of a programmatic signer or a multisig. `None` for a
    /// wallet that holds a secret.
    pub contract: Option<WalletKind>,
    /// The hot wallet chosen to pay this wallet's fees.
    pub fee_payer: Option<WalletId>,
    /// Where a seed phrase wallet's key is derived, such as `m/44'/501'/0'/0'`.
    pub derivation_path: Option<String>,
    /// An offline wallet's durable nonce account.
    pub durable_nonce: Option<Pubkey>,
    /// The network `durable_nonce` was made on, when the record says.
    pub nonce_network: Option<String>,
    /// Which path an offline wallet takes when it is chosen.
    pub default_path: Option<OfflinePath>,
    /// A hardware wallet's vendor. Its path is in `derivation_path`.
    pub on_device: Option<HardwareVendor>,
    /// What names a hardware wallet's device: a Keystone's master
    /// fingerprint, a Trezor's device id.
    pub device: Option<String>,
}

/// An account a site can be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareableAccount {
    pub pubkey: Pubkey,
    pub label: String,
    /// A programmatic signer or a Squads vault: no key signs as this address,
    /// so what a site asks of it is rebuilt before it is signed.
    pub contract: bool,
    /// "seed · m/44'/501'/0'/0'", "Squads v4 · 2 of 3", and so on. Empty
    /// for an account the address book names before there is a vault.
    pub summary: String,
    /// A hardware wallet of this vault holds the key.
    pub on_device: Option<HardwareVendor>,
}

/// Why a wallet may not be added.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RosterError {
    #[error("{0} is already a signer on this device.")]
    Duplicate(Pubkey),
    #[error("{0} is already a wallet on this device.")]
    DuplicateWallet(Pubkey),
    #[error("Only a Vector wallet keeps its authority key in its own record.")]
    KeyInRecord,
    #[error("Every wallet needs a label.")]
    NoLabel,
    #[error("{0}")]
    BadKey(String),
    #[error("Only a hardware wallet is added this way.")]
    NotHardware,
}

impl VaultData {
    fn holds(&self, address: &Pubkey) -> bool {
        self.wallets
            .iter()
            .any(|wallet| wallet.address() == Some(*address))
    }

    /// The wallet that was chosen, while it is still here.
    pub fn chosen_wallet(&self) -> Option<WalletId> {
        self.selected
            .filter(|id| self.wallets.iter().any(|wallet| wallet.id == *id))
    }

    /// The wallet the app is acting as: the one chosen, or the first when
    /// none was chosen or the choice was removed.
    pub fn acting_wallet(&self) -> Option<WalletId> {
        self.chosen_wallet()
            .or_else(|| self.wallets.first().map(|wallet| wallet.id))
    }

    /// The address of the wallet the app is acting as.
    pub fn acting_address(&self) -> Option<Pubkey> {
        let id = self.acting_wallet()?;
        self.wallets
            .iter()
            .find(|wallet| wallet.id == id)?
            .address()
    }

    /// Addresses a key of this vault can sign for.
    pub fn local_signer_pubkeys(&self) -> Vec<Pubkey> {
        self.wallets
            .iter()
            .filter(|wallet| wallet.kind.can_sign())
            .filter_map(|wallet| wallet.address())
            .collect()
    }

    /// Keys on hardware wallets of this vault, with their vendor. They sign
    /// on the device, never with a vault key.
    pub fn local_device_keys(&self) -> Vec<(Pubkey, HardwareVendor)> {
        self.wallets
            .iter()
            .filter_map(|wallet| match &wallet.kind {
                WalletKind::Hardware { vendor, pubkey, .. } => Some((*pubkey, *vendor)),
                _ => None,
            })
            .collect()
    }

    /// Every wallet, as the screens list it.
    pub fn summaries(&self) -> Vec<WalletSummary> {
        self.wallets
            .iter()
            .map(|wallet| WalletSummary {
                id: wallet.id,
                label: wallet.label.clone(),
                tags: wallet.tags.iter().cloned().collect(),
                class: wallet.kind.class(),
                summary: wallet.summary(),
                address: wallet.address(),
                contract: matches!(
                    wallet.kind,
                    WalletKind::Programmatic { .. } | WalletKind::Squads { .. }
                )
                .then(|| wallet.without_secrets().kind),
                fee_payer: wallet.kind.fee_payer(),
                derivation_path: match &wallet.kind {
                    WalletKind::Seed { path, .. } | WalletKind::Hardware { path, .. }
                        if !path.is_empty() =>
                    {
                        Some(path.clone())
                    }
                    _ => None,
                },
                durable_nonce: wallet.kind.durable_nonce(),
                nonce_network: wallet.kind.nonce_network().map(str::to_string),
                default_path: wallet.kind.default_path(),
                on_device: wallet.kind.hardware_vendor(),
                device: wallet.kind.hardware_device().map(str::to_string),
            })
            .collect()
    }

    /// Store several signers. None is stored when one of them is already a
    /// wallet here, or is listed twice.
    pub fn add_signers(&mut self, signers: &[NewSigner]) -> Result<(), RosterError> {
        for (i, signer) in signers.iter().enumerate() {
            let repeated = signers[..i]
                .iter()
                .any(|other| other.pubkey == signer.pubkey);
            if repeated || self.holds(&signer.pubkey) {
                return Err(RosterError::Duplicate(signer.pubkey));
            }
        }
        self.wallets
            .extend(signers.iter().map(|signer| signer.wallet.clone()));
        Ok(())
    }

    /// Store a programmatic signer or a multisig. Answers its address, or
    /// `None` for a record without one, which is not stored.
    pub fn add_contract_wallet(
        &mut self,
        wallet: WalletRecord,
    ) -> Result<Option<Pubkey>, RosterError> {
        let Some(address) = wallet.address() else {
            return Ok(None);
        };
        if self.holds(&address) {
            return Err(RosterError::DuplicateWallet(address));
        }
        if matches!(
            wallet.kind,
            WalletKind::Programmatic {
                protocol: Protocol::Ed25519Signer { .. },
                authority: Authority::Held { .. },
                ..
            }
        ) {
            return Err(RosterError::KeyInRecord);
        }
        self.wallets.push(wallet);
        Ok(Some(address))
    }

    /// Store an offline wallet: an address whose key is kept on another
    /// device. A programmatic signer already here for that key is tied to it.
    pub fn add_offline_wallet(
        &mut self,
        label: impl Into<String>,
        pubkey: Pubkey,
        fee_payer: Option<WalletId>,
    ) -> Result<WalletId, RosterError> {
        if self.holds(&pubkey) {
            return Err(RosterError::DuplicateWallet(pubkey));
        }
        let mut wallet = WalletRecord::offline(label, pubkey);
        wallet.kind.set_fee_payer(fee_payer);
        let id = wallet.id;
        self.tie_authority(id, pubkey, true);
        self.wallets.push(wallet);
        Ok(id)
    }

    /// Store hardware wallets, each a key read from its device. All of them,
    /// or none when one is already here, is listed twice or is not a
    /// hardware wallet. A programmatic signer already here for one of the
    /// keys is tied to it, as for an offline wallet.
    pub fn add_hardware_wallets(&mut self, wallets: &[WalletRecord]) -> Result<(), RosterError> {
        for (i, wallet) in wallets.iter().enumerate() {
            let WalletKind::Hardware { pubkey, .. } = &wallet.kind else {
                return Err(RosterError::NotHardware);
            };
            if wallet.label.trim().is_empty() {
                return Err(RosterError::NoLabel);
            }
            let repeated = wallets[..i]
                .iter()
                .any(|other| other.address() == Some(*pubkey));
            if repeated || self.holds(pubkey) {
                return Err(RosterError::DuplicateWallet(*pubkey));
            }
        }
        for wallet in wallets {
            if let Some(pubkey) = wallet.address() {
                self.tie_authority(wallet.id, pubkey, false);
            }
            self.wallets.push(wallet.clone());
        }
        Ok(())
    }

    /// Tie each programmatic signer whose authority is `pubkey`, and is not
    /// a wallet here yet, to the wallet `id` that holds that key. A key on a
    /// device signs no Vector digest, so `vector` is false for one.
    fn tie_authority(&mut self, id: WalletId, pubkey: Pubkey, vector: bool) {
        for other in &mut self.wallets {
            let WalletKind::Programmatic {
                protocol,
                authority,
                ..
            } = &mut other.kind
            else {
                continue;
            };
            // Only an Ed25519 key is an address a wallet can stand for.
            let signs_for = match protocol {
                Protocol::Ed25519Signer { authority, .. } => *authority == pubkey,
                Protocol::Vector { scheme, key, .. } => {
                    vector && *scheme == SchemeId::Ed25519 && key.0 == pubkey.to_bytes()
                }
            };
            if signs_for && *authority == Authority::Outside {
                *authority = Authority::Wallet { wallet: id };
            }
        }
    }

    /// Remove a wallet, and clear every setting of another wallet that pointed at it.
    pub fn remove_wallet(&mut self, id: WalletId) {
        // An offline wallet whose programmatic signer goes no longer takes
        // that path.
        let signed_for = self
            .wallets
            .iter()
            .find(|wallet| wallet.id == id)
            .and_then(|wallet| wallet.kind.signer_authority());
        self.wallets.retain(|wallet| wallet.id != id);
        if self.selected == Some(id) {
            self.selected = None;
        }
        for wallet in &mut self.wallets {
            if wallet.kind.fee_payer() == Some(id) {
                wallet.kind.set_fee_payer(None);
            }
            if let WalletKind::Programmatic { authority, .. } = &mut wallet.kind {
                if authority.wallet() == Some(id) {
                    *authority = Authority::Outside;
                }
            }
            if let WalletKind::Squads { member_wallet, .. } = &mut wallet.kind {
                if *member_wallet == Some(id) {
                    *member_wallet = None;
                }
            }
            if let WalletKind::Squads { approves_with, .. } = &mut wallet.kind {
                approves_with.retain(|other| *other != id);
            }
            if signed_for.is_some()
                && wallet.address() == signed_for
                && wallet.kind.default_path() == Some(OfflinePath::Programmatic)
            {
                wallet.kind.set_default_path(None);
            }
        }
    }
}

/// The programmatic signer whose authority is `key`, when one is a wallet
/// of this device.
pub fn signer_of(wallets: &[WalletRecord], key: Pubkey) -> Option<&WalletRecord> {
    wallets
        .iter()
        .find(|wallet| wallet.kind.signer_authority() == Some(key))
}

/// The wallet that acts when `id` is chosen. An offline or hardware wallet
/// whose default path is its programmatic signer hands over to that signer,
/// a wallet of its own at another address. Any other wallet acts as itself.
pub fn acting_wallet(wallets: &[WalletRecord], id: WalletId) -> WalletId {
    let Some(wallet) = wallets.iter().find(|wallet| wallet.id == id) else {
        return id;
    };
    match (&wallet.kind, wallet.address()) {
        (
            WalletKind::Offline {
                default_path: Some(OfflinePath::Programmatic),
                ..
            }
            | WalletKind::Hardware {
                default_path: Some(OfflinePath::Programmatic),
                ..
            },
            Some(key),
        ) => signer_of(wallets, key).map_or(id, |signer| signer.id),
        _ => id,
    }
}

/// Name each new signer in the address book, tied to its wallet.
pub fn name_signers(book: &mut AddressBook, signers: &[NewSigner]) {
    for signer in signers {
        book.insert_account(
            signer.pubkey,
            signer.label.clone(),
            signer.origin,
            Some(signer.wallet.id),
        );
    }
}

/// Name an offline wallet in the address book, unless the address already
/// has an entry. No wallet id: the book ties an address to a wallet only
/// when this device holds its key.
pub fn name_offline_wallet(book: &mut AddressBook, pubkey: Pubkey, label: impl Into<String>) {
    if book.get(&pubkey).is_none() {
        book.insert_account(pubkey, label, Origin::Imported, None);
    }
}

/// Accounts this device can act as: the ones it holds a key for, and its
/// programmatic signers and Squads vaults, with its Vector wallets while they
/// are offered. With no vault data, which is a locked vault, the list comes
/// from the address-book entries tied to a wallet.
pub fn shareable_accounts(
    vault: Option<&VaultData>,
    workspace: &Workspace,
) -> Vec<ShareableAccount> {
    let book = &workspace.book;
    let mut out: Vec<ShareableAccount> = Vec::new();
    for wallet in vault
        .map(|data| data.wallets.as_slice())
        .unwrap_or_default()
    {
        let contract = wallet.kind.class() == WalletClass::SmartContract;
        let on_device = wallet.kind.hardware_vendor();
        if !wallet.kind.can_sign() && !contract && on_device.is_none() {
            continue;
        }
        if wallet.kind.vector_settings().is_some() && !vector::offered(workspace) {
            continue;
        }
        if let Some(pubkey) = wallet.address() {
            if !out.iter().any(|known| known.pubkey == pubkey) {
                out.push(ShareableAccount {
                    pubkey,
                    label: wallet.label.clone(),
                    contract,
                    summary: wallet.summary(),
                    on_device,
                });
            }
        }
    }
    for row in book.rows_in(LOCAL_GROUP_ID, "", Some(AddressKind::Account)) {
        let Some(wallet_id) = row.entry.wallet_id else {
            continue;
        };
        // An entry left tied to a wallet that is no longer listed names a
        // key this device does not hold.
        if vault.is_some_and(|data| !data.wallets.iter().any(|wallet| wallet.id == wallet_id)) {
            continue;
        }
        if !out.iter().any(|known| known.pubkey == row.entry.pubkey) {
            out.push(ShareableAccount {
                pubkey: row.entry.pubkey,
                label: row.entry.label,
                contract: false,
                summary: String::new(),
                on_device: None,
            });
        }
    }
    out
}

/// What a refresh reads from the chain: every owner and contact of the book
/// with every wallet's address, and the nonce accounts the book knows.
pub fn refresh_targets(
    vault: Option<&VaultData>,
    book: &AddressBook,
) -> (Vec<Pubkey>, Vec<Pubkey>) {
    let mut owners: Vec<Pubkey> = book
        .iter()
        .filter(|e| matches!(e.kind, AddressKind::Account))
        .map(|e| e.pubkey)
        .collect();
    for wallet in vault
        .map(|data| data.wallets.as_slice())
        .unwrap_or_default()
    {
        if let Some(pk) = wallet.address() {
            if !owners.contains(&pk) {
                owners.push(pk);
            }
        }
    }
    let nonces: Vec<Pubkey> = book
        .iter()
        .filter(|e| e.kind == AddressKind::NonceAccount)
        .map(|e| e.pubkey)
        .collect();
    (owners, nonces)
}

#[cfg(test)]
mod tests;
