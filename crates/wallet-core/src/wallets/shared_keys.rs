//! Wallets moved between devices by their public keys alone.
//!
//! The device that holds a key exports what another device needs to act for
//! it: the address of a key, or the scheme and key a Vector account was made
//! from. The other device adds each one without its secret: an address as an
//! offline wallet, a Vector account with its key elsewhere. A Vector account
//! is initialized from there, since initializing needs a fee payer and a
//! recent blockhash, and the key never signs it.

use crate::ids::WalletId;
use crate::transport::protocol::{KeyRole, WalletKey};
use crate::wallets::record::{Authority, WalletClass, WalletKind, WalletRecord};
use crate::wallets::roster::{RosterError, WalletSummary};
use crate::wallets::vault::VaultData;
use crate::wallets::vector::{self, scheme_offered, SchemeId, PROGRAM_ID};
use solana_pubkey::Pubkey;

/// A wallet of this device that another device can add by its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exportable {
    pub id: WalletId,
    pub key: WalletKey,
    /// What the other device adds it as.
    pub becomes: String,
}

/// Every wallet of `wallets` that another device can add: a hot or offline
/// key as an offline wallet there, and a Vector wallet of an offered scheme
/// with its key elsewhere. Programmatic signers and multisigs are left out:
/// the other device sets those up from the key.
pub fn exportable(wallets: &[WalletSummary]) -> Vec<Exportable> {
    wallets.iter().filter_map(exportable_one).collect()
}

fn exportable_one(wallet: &WalletSummary) -> Option<Exportable> {
    let (role, key, becomes) = match (&wallet.class, &wallet.contract) {
        (WalletClass::Hot | WalletClass::Offline, _) => (
            KeyRole::Address,
            wallet.address?.to_bytes().to_vec(),
            "Offline wallet".to_string(),
        ),
        (WalletClass::SmartContract, Some(kind)) => {
            let settings = kind.vector_settings()?;
            if !scheme_offered(settings.scheme) {
                return None;
            }
            (
                KeyRole::Vector {
                    scheme: settings.scheme.byte(),
                },
                settings.key.to_vec(),
                format!("Vector wallet · {}", settings.scheme.label()),
            )
        }
        (WalletClass::SmartContract | WalletClass::Hardware, _) => return None,
    };
    Some(Exportable {
        id: wallet.id,
        key: WalletKey {
            role,
            label: wallet.label.clone(),
            key,
        },
        becomes,
    })
}

/// What adding one arriving key would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arrival {
    /// It would be a new wallet.
    New,
    /// A wallet here already has its address.
    AlreadyHere { label: String },
    /// It cannot be added, and why.
    Refused(String),
}

/// One key that arrived from another device, as this device reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrivingKey {
    pub key: WalletKey,
    /// Where its funds are. `None` when the key cannot be read.
    pub address: Option<Pubkey>,
    /// What it is added as.
    pub becomes: String,
    pub arrival: Arrival,
}

/// Read `keys` against the wallets already here. The same address twice in
/// `keys` is new once and then already here.
pub fn arriving(wallets: &[WalletSummary], keys: &[WalletKey]) -> Vec<ArrivingKey> {
    let mut seen: Vec<(Pubkey, String)> = wallets
        .iter()
        .filter_map(|wallet| Some((wallet.address?, wallet.label.clone())))
        .collect();
    keys.iter()
        .map(|key| {
            let read = read_key(key);
            let (address, becomes, arrival) = match read {
                Err((becomes, why)) => (None, becomes, Arrival::Refused(why)),
                Ok((address, becomes)) => {
                    let arrival = match seen.iter().find(|(known, _)| *known == address) {
                        Some((_, label)) => Arrival::AlreadyHere {
                            label: label.clone(),
                        },
                        None => {
                            seen.push((address, key.label.clone()));
                            Arrival::New
                        }
                    };
                    (Some(address), becomes, arrival)
                }
            };
            ArrivingKey {
                key: key.clone(),
                address,
                becomes,
                arrival,
            }
        })
        .collect()
}

/// The address `key` stands for and what it is added as, or what it would
/// have been and why it cannot be.
fn read_key(key: &WalletKey) -> Result<(Pubkey, String), (String, String)> {
    match key.role {
        KeyRole::Address => {
            let bytes: [u8; 32] = key
                .key
                .as_slice()
                .try_into()
                .map_err(|_| ("Offline wallet".into(), "An address is 32 bytes.".into()))?;
            Ok((Pubkey::new_from_array(bytes), "Offline wallet".into()))
        }
        KeyRole::Vector { scheme } => {
            let Some(scheme) = SchemeId::from_byte(scheme) else {
                return Err((
                    "Vector wallet".into(),
                    format!("Scheme {scheme} is not one this build knows."),
                ));
            };
            let becomes = format!("Vector wallet · {}", scheme.label());
            if !scheme_offered(scheme) {
                return Err((
                    becomes,
                    format!("{} wallets are not offered.", scheme.label()),
                ));
            }
            vector::address(&PROGRAM_ID, scheme, &key.key)
                .map(|address| (address, becomes.clone()))
                .map_err(|err| (becomes, err.to_string()))
        }
    }
}

/// One arriving key to add, under the label the user gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyToAdd {
    pub key: WalletKey,
    pub label: String,
}

/// A wallet [`VaultData::add_wallet_keys`] stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddedKey {
    pub id: WalletId,
    pub address: Pubkey,
    pub label: String,
    /// An offline wallet, rather than a Vector wallet.
    pub offline: bool,
}

impl VaultData {
    /// Add every key of `keys`, each paid for by `fee_payer`. Addresses go
    /// in first, so a Vector account of an Ed25519 key added with it signs
    /// as that offline wallet. One that cannot be added refuses them all.
    pub fn add_wallet_keys(
        &mut self,
        keys: &[KeyToAdd],
        fee_payer: Option<WalletId>,
    ) -> Result<Vec<AddedKey>, RosterError> {
        let mut added = Vec::with_capacity(keys.len());
        let (addresses, accounts): (Vec<&KeyToAdd>, Vec<&KeyToAdd>) = keys
            .iter()
            .partition(|entry| entry.key.role == KeyRole::Address);
        for entry in addresses.into_iter().chain(accounts) {
            let label = entry.label.trim().to_string();
            if label.is_empty() {
                return Err(RosterError::NoLabel);
            }
            let (address, _) = read_key(&entry.key).map_err(|(_, why)| RosterError::BadKey(why))?;
            let id = match entry.key.role {
                KeyRole::Address => self.add_offline_wallet(label.clone(), address, fee_payer)?,
                KeyRole::Vector { scheme } => {
                    let scheme = SchemeId::from_byte(scheme).expect("read above");
                    let authority = self.authority_for(scheme, &entry.key.key);
                    let mut wallet = WalletRecord::vector(
                        label.clone(),
                        scheme,
                        PROGRAM_ID,
                        entry.key.key.clone(),
                        authority,
                    );
                    wallet.kind.set_fee_payer(fee_payer);
                    let id = wallet.id;
                    self.add_contract_wallet(wallet)?;
                    id
                }
            };
            added.push(AddedKey {
                id,
                address,
                label,
                offline: entry.key.role == KeyRole::Address,
            });
        }
        Ok(added)
    }

    /// Who signs for a Vector account of `key`: an Ed25519 key that is a
    /// wallet here signs as that wallet, and any other key is elsewhere.
    fn authority_for(&self, scheme: SchemeId, key: &[u8]) -> Authority {
        if scheme != SchemeId::Ed25519 {
            return Authority::Outside;
        }
        self.wallets
            .iter()
            .find(|wallet| {
                matches!(
                    wallet.kind,
                    WalletKind::Keypair { .. }
                        | WalletKind::Seed { .. }
                        | WalletKind::Offline { .. }
                ) && wallet.address().map(|address| address.to_bytes().to_vec())
                    == Some(key.to_vec())
            })
            .map(|wallet| Authority::Wallet { wallet: wallet.id })
            .unwrap_or(Authority::Outside)
    }
}

#[cfg(test)]
#[path = "shared_keys_tests.rs"]
mod tests;
