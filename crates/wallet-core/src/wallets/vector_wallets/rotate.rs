//! Moving an XMSS account to its next key from the device that holds the
//! key, without a node.
//!
//! A rotation is an `Advance` and nothing else, so what the key signs does
//! not depend on who pays or on the blockhash (`rotation_message`). Only the
//! nonce is missing, and this device has signed everything the account ever
//! did: its nonce is the one it last signed at, or a digest it signed there.
//! So the rotation is signed once for each, and the device that sends it
//! picks the one the account holds. Each signature spends a leaf of the key
//! that is being retired anyway.

use super::leaves::{install_landed_rotations, reserve_leaf, LeafError, LeafFor, LeafMarks};
use crate::ids::WalletId;
use crate::transport::protocol::AccountState;
use crate::transport::protocol::{KeyRotation, RotationSignature, MAX_ROTATION_NONCES};
use crate::wallets::record::Authority;
use crate::wallets::relay::VectorContext;
use crate::wallets::vault::Vault;
use crate::wallets::vector::{
    rotation_message, signer_digest, SchemeId, VectorAccount, VectorSigner, XMSS_LEAVES,
};
use solana_pubkey::Pubkey;
use wallet_vault::store::VaultStore;

/// Sign the move of `wallet`'s XMSS account to its next key, making that
/// key first when there is none. What is about to be signed is recorded
/// before it is signed: the leaves, and the digests that say later that the
/// rotation landed.
pub fn sign_rotation<V: VaultStore, M: VaultStore>(
    vault: &mut Vault<V>,
    marks: &mut LeafMarks<M>,
    wallet: WalletId,
) -> Result<KeyRotation, LeafError> {
    let data = vault.data().map_err(|_| LeafError::Locked)?;
    let record = data
        .wallets
        .iter()
        .find(|record| record.id == wallet)
        .ok_or(LeafError::NoWallet)?;
    let label = record.label.clone();
    let account = record.address().ok_or(LeafError::NoWallet)?;
    let settings = record
        .kind
        .vector_settings()
        .ok_or_else(|| LeafError::NotStateful(label.clone()))?;
    if settings.scheme != SchemeId::Xmss || !matches!(settings.authority, Authority::Held { .. }) {
        return Err(LeafError::NotStateful(label));
    }
    let created_with = settings.key.to_vec();
    // Its own digests are where the account is once it has moved: the key
    // that signs this cannot sign there.
    let pending = marks.rotation_digests(&account);
    let mut nonces = marks.possible_nonces(&account);
    nonces.retain(|nonce| !pending.contains(nonce));
    if nonces.is_empty() {
        return Err(LeafError::NoTrail(label));
    }
    let restored = record.vector_phrase().is_some_and(|phrase| phrase.restored);
    if restored {
        // The state the online device read: one nonce, and one leaf.
        nonces.truncate(1);
    }
    nonces.truncate(MAX_ROTATION_NONCES);

    if record.vector_prepared_next_key().is_none() {
        let mut refused = None;
        vault
            .change(|data| {
                if let Some(record) = data.wallets.iter_mut().find(|record| record.id == wallet) {
                    refused = record.prepare_vector_next_key().err();
                }
            })
            .map_err(LeafError::Vault)?;
        if let Some(err) = refused {
            return Err(LeafError::Rotation(err.to_string()));
        }
    }
    let data = vault.data().map_err(|_| LeafError::Locked)?;
    let next_key = data
        .wallets
        .iter()
        .find(|record| record.id == wallet)
        .and_then(|record| record.vector_prepared_next_key())
        .ok_or(LeafError::NoWallet)?
        .to_vec();
    let signer = VectorSigner::from_record(&data.wallets, wallet)
        .map_err(|err| LeafError::Rotation(err.to_string()))?;
    let key = signer.key();

    let message = rotation_message(SchemeId::Xmss, &created_with, &next_key)
        .map_err(|err| LeafError::Rotation(err.to_string()))?;
    let digests: Vec<[u8; 32]> = nonces
        .iter()
        .map(|nonce| signer_digest(&message, nonce, &account, &next_key))
        .collect();
    marks
        .note_rotation(&account, &next_key, &digests)
        .map_err(LeafError::Mark)?;

    if restored {
        // A lost copy took leaves in rising order, so the last one is the
        // one it cannot have used.
        marks
            .raise(&account, &key, XMSS_LEAVES - 1)
            .map_err(LeafError::Mark)?;
    }
    let mut signed = Vec::with_capacity(nonces.len());
    for (nonce, digest) in nonces.iter().zip(&digests) {
        let reserved = reserve_leaf(vault, marks, wallet, &key, None, LeafFor::Rotation)?;
        let signature = signer
            .sign_leaf(reserved.leaf, digest)
            .map_err(|err| LeafError::Rotation(err.to_string()))?;
        signed.push(RotationSignature {
            nonce: *nonce,
            signature,
        });
    }
    Ok(KeyRotation {
        account: account.to_bytes(),
        scheme: SchemeId::Xmss.byte(),
        next_key,
        signed,
    })
}

/// What the chain says of the Vector account at `address`, for a device
/// that cannot read it.
pub fn account_state(address: &Pubkey, read: &VectorAccount, created_with: &[u8]) -> AccountState {
    AccountState {
        account: address.to_bytes(),
        scheme: read.scheme.byte(),
        nonce: read.nonce,
        leaf_floor: read.next_leaf().unwrap_or(0),
        key: read
            .current_key()
            .map_or_else(|| created_with.to_vec(), <[u8]>::to_vec),
    }
}

/// What taking an account state changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adopted {
    pub wallet: WalletId,
    pub label: String,
    /// The number of the key made from the phrase that signs now, when that
    /// changed.
    pub index: Option<u32>,
}

/// Take what another device read of an account whose key is kept here: a
/// rotation it shows landed is installed, a key made from the wallet's
/// phrase that the account holds is found and signs from then on, and the
/// account's nonce and leaf floor are where signing starts.
///
/// The state is only as good as the device that read it. A wrong key or
/// nonce makes this device sign what fails on chain. It cannot make it use
/// a leaf twice, and a key made from a phrase can always be made again.
pub fn adopt_account_state<V: VaultStore, M: VaultStore>(
    vault: &mut Vault<V>,
    marks: &mut LeafMarks<M>,
    state: &AccountState,
) -> Result<Adopted, LeafError> {
    let account = Pubkey::new_from_array(state.account);
    let data = vault.data().map_err(|_| LeafError::Locked)?;
    let record = data
        .wallets
        .iter()
        .find(|record| record.address() == Some(account))
        .ok_or(LeafError::NoWallet)?;
    let (wallet, label) = (record.id, record.label.clone());
    let scheme = record
        .kind
        .vector_settings()
        .map(|settings| settings.scheme)
        .filter(|scheme| scheme.byte() == state.scheme && *scheme == SchemeId::Xmss)
        .ok_or_else(|| LeafError::NotStateful(label.clone()))?;
    install_landed_rotations(
        vault,
        marks,
        &[VectorContext {
            account,
            scheme,
            nonce: Some(state.nonce),
            key: Some(state.key.clone()),
            leaf_floor: state.leaf_floor,
        }],
    )?;

    let data = vault.data().map_err(|_| LeafError::Locked)?;
    let record = data
        .wallets
        .iter()
        .find(|record| record.id == wallet)
        .ok_or(LeafError::NoWallet)?;
    let index = if record.vector_phrase().is_some() {
        let mut adopted = Ok(None);
        vault
            .change(|data| {
                if let Some(record) = data.wallets.iter_mut().find(|record| record.id == wallet) {
                    adopted = record.adopt_vector_key(&state.key, state.leaf_floor);
                }
            })
            .map_err(LeafError::Vault)?;
        adopted.map_err(|err| LeafError::Rotation(err.to_string()))?
    } else {
        let held = VectorSigner::from_record(&data.wallets, wallet)
            .map_err(|err| LeafError::Rotation(err.to_string()))?
            .key();
        if held != state.key {
            return Err(LeafError::Rotation(format!(
                "The account of {label} holds a key this device does not have."
            )));
        }
        None
    };
    marks
        .reset_trail(&account, &state.nonce)
        .map_err(LeafError::Mark)?;
    marks
        .raise(&account, &state.key, state.leaf_floor)
        .map_err(LeafError::Mark)?;
    Ok(Adopted {
        wallet,
        label,
        index,
    })
}
