//! What a hash-based key has signed, kept so that no part of it signs
//! twice.
//!
//! An XMSS key signs once per leaf, and a Winternitz key signs once. Signing
//! two messages with one leaf, or with one Winternitz key, gives the key
//! away. The chain refuses an XMSS leaf below the account's `next_leaf`,
//! which protects what has landed, but not two signatures made off chain
//! before either lands. So what is about to sign is recorded before it signs:
//!
//! - XMSS: the leaf is reserved in a mark in a small file of its own, outside
//!   the vault, that only rises, and then in the record's `next_leaf`, in the
//!   vault. The mark is written and synced first, the vault second, and only
//!   then may the key sign. When a write fails nothing is signed, and the
//!   counters in memory stay where they were moved: a leaf can be skipped,
//!   never reused.
//! - Winternitz: the digest it signs and the salt it signs with go in the
//!   same file before it signs. The same digest signs again with the same
//!   salt, which gives the same signature. Another digest is refused until
//!   the account has moved to its next key.
//!
//! Each mark belongs to one key of one account. A key that replaces another
//! starts with no mark, at its own first leaf.
//!
//! The same file keeps, for each XMSS account, where its nonce may be. Only
//! this device signs for the account, so the nonce is the one it last signed
//! at, or one of the digests it signed there, whichever landed. That is what
//! a rotation is signed at when this device cannot read the chain, and a
//! rotation's own digests say when it has landed.
//!
//! What this cannot defend against is a copy of the whole data directory,
//! marks included, restored and used to sign again before the first
//! signature lands.

use super::held::local_vector_keys;
use crate::error::VaultError;
use crate::ids::WalletId;
use crate::keys::signer::WalletSigner;
use crate::transport::protocol::MAX_ROTATION_NONCES;
use crate::wallets::record::{Authority, Protocol, WalletKind};
use crate::wallets::relay::{
    refuse_handover, sign_authorizations_in, to_sign_now, AuthContext, VectorContext,
};
use crate::wallets::vault::Vault;
use crate::wallets::vector::{
    seed, SchemeId, VectorKey, VectorSigner, XmssRetryCache, XMSS_FORCE_ROTATE_USED, XMSS_LEAVES,
    XMSS_SALT_LEN,
};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;
use thiserror::Error;
use wallet_vault::store::{check_saveable, load_json_or_block, LoadProblem, VaultStore};

/// Leaves passed over when a key's mark is missing but its record says it
/// has signed. The record may be an older copy of the vault, so the leaves
/// just after it may have signed already.
pub const LEAVES_SKIPPED_WITHOUT_MARK: u32 = 16;

/// What a leaf is reserved to sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeafFor {
    /// A move of the account to the next key this device made.
    Rotation,
    /// Anything that keeps the key. Signed only below
    /// [`XMSS_FORCE_ROTATE_USED`]: the leaves above it are kept for the
    /// rotation, so a key can always move on, whatever the floor it is told.
    Other,
}

// A rotation may sign at every nonce it may be at, one leaf each, and the
// leaves kept for it have to cover that.
const _: () = assert!((XMSS_LEAVES - XMSS_FORCE_ROTATE_USED) as usize >= MAX_ROTATION_NONCES);

/// The marks as stored, each under its account and the hash of its key.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marks {
    /// XMSS: the lowest leaf not yet reserved.
    #[serde(default)]
    leaves: BTreeMap<String, u32>,
    /// Winternitz: what the key signed, in hex.
    #[serde(default)]
    one_time: BTreeMap<String, OneTimeUse>,
    /// XMSS: where each account's nonce may be, by address.
    #[serde(default)]
    trails: BTreeMap<String, Trail>,
    /// XMSS: a move to the next key that was signed and may not have
    /// landed, by address.
    #[serde(default)]
    rotations: BTreeMap<String, Rotation>,
}

/// The nonces an account may be at: the one this device last signed at, and
/// every digest it signed there or since. In hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Trail {
    root: String,
    possible: Vec<String>,
}

/// A rotation signed here: the key it moves to, and the digest of each
/// signature, which becomes the account's nonce when that one lands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rotation {
    next_key: String,
    digests: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OneTimeUse {
    digest: String,
    salt: String,
}

/// Where the marks of `key`, the key that signs for `account` now, are kept.
fn mark_id(account: &Pubkey, key: &[u8]) -> String {
    format!("{account}:{}", hex::encode(seed(key)))
}

/// The marks of this device's hash-based keys, in a store of their own.
pub struct LeafMarks<S: VaultStore> {
    store: S,
    marks: Marks,
    problem: Option<LoadProblem>,
}

impl<S: VaultStore> LeafMarks<S> {
    /// Open the marks. Marks that cannot be read are not replaced: an
    /// empty record would let a used leaf sign again, so every save, and so
    /// every hash-based signature, is refused until the file reads again.
    pub fn open(store: S) -> Self {
        let (marks, problem) = load_json_or_block(&store, "record of used signing leaves");
        Self {
            store,
            marks,
            problem,
        }
    }

    /// Set when the stored marks could not be read at startup.
    pub fn load_problem(&self) -> Option<&LoadProblem> {
        self.problem.as_ref()
    }

    /// The lowest leaf of `key`, signing for `account`, not yet reserved,
    /// when there is a mark.
    pub fn next_leaf(&self, account: &Pubkey, key: &[u8]) -> Option<u32> {
        self.marks.leaves.get(&mark_id(account, key)).copied()
    }

    /// Move the mark of `key` up to `next`, and save. A mark never goes
    /// down. When the save fails the mark stays raised in memory.
    pub(super) fn raise(
        &mut self,
        account: &Pubkey,
        key: &[u8],
        next: u32,
    ) -> Result<(), VaultError> {
        let mark = self.marks.leaves.entry(mark_id(account, key)).or_insert(0);
        *mark = (*mark).max(next);
        self.save()
    }

    /// The salt `key` signs `digest` with: the one recorded, when it is this
    /// digest it signed, or a new one, recorded and saved first. `None` when
    /// the key already signed another digest.
    fn spend_once(
        &mut self,
        account: &Pubkey,
        key: &[u8],
        digest: &[u8; 32],
        choose: impl FnOnce() -> Result<[u8; XMSS_SALT_LEN], VaultError>,
    ) -> Result<Option<[u8; XMSS_SALT_LEN]>, VaultError> {
        let id = mark_id(account, key);
        if let Some(used) = self.marks.one_time.get(&id) {
            if used.digest != hex::encode(digest) {
                return Ok(None);
            }
            let salt = hex::decode(&used.salt)
                .ok()
                .and_then(|salt| salt.try_into().ok())
                .ok_or_else(|| VaultError::Serde("a recorded salt cannot be read".into()))?;
            // An earlier save may have failed: the file has to hold the
            // digest before the key signs it.
            self.save()?;
            return Ok(Some(salt));
        }
        let salt = choose()?;
        // Kept in memory when the save fails: the same digest may then sign
        // with this salt, and no other digest may.
        self.marks.one_time.insert(
            id,
            OneTimeUse {
                digest: hex::encode(digest),
                salt: hex::encode(salt),
            },
        );
        self.save()?;
        Ok(Some(salt))
    }

    /// The nonces `account` may be at, the last one it was seen at first.
    /// Empty when this device has not signed for it.
    pub fn possible_nonces(&self, account: &Pubkey) -> Vec<[u8; 32]> {
        self.marks
            .trails
            .get(&account.to_string())
            .map(|trail| trail.possible.iter().filter_map(|hex| unhex(hex)).collect())
            .unwrap_or_default()
    }

    /// The digests of the rotation signed here for `account`, while it may
    /// not have landed.
    pub fn rotation_digests(&self, account: &Pubkey) -> Vec<[u8; 32]> {
        self.marks
            .rotations
            .get(&account.to_string())
            .map(|rotation| {
                rotation
                    .digests
                    .iter()
                    .filter_map(|hex| unhex(hex))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `account` is at `nonce`, as the chain says, so nothing signed at
    /// another nonce can land before it.
    pub(super) fn reset_trail(
        &mut self,
        account: &Pubkey,
        nonce: &[u8; 32],
    ) -> Result<(), VaultError> {
        let nonce = hex::encode(nonce);
        self.marks.trails.insert(
            account.to_string(),
            Trail {
                root: nonce.clone(),
                possible: vec![nonce],
            },
        );
        self.save()
    }

    /// The key a rotation signed here moves `account` to, while it may not
    /// have landed.
    pub fn pending_rotation(&self, account: &Pubkey) -> Option<Vec<u8>> {
        let rotation = self.marks.rotations.get(&account.to_string())?;
        hex::decode(&rotation.next_key).ok()
    }

    /// `account`'s key signed `digest` at `nonce`. Saved before it signs.
    pub(super) fn note_signed(
        &mut self,
        account: &Pubkey,
        nonce: &[u8; 32],
        digest: &[u8; 32],
    ) -> Result<(), VaultError> {
        let (nonce, digest) = (hex::encode(nonce), hex::encode(digest));
        let trail = self
            .marks
            .trails
            .entry(account.to_string())
            .or_insert_with(|| Trail {
                root: nonce.clone(),
                possible: vec![nonce.clone()],
            });
        // At a nonce after the last one, what was signed before it is void,
        // or has landed and led here.
        if trail.root != nonce {
            *trail = Trail {
                root: nonce.clone(),
                possible: vec![nonce],
            };
        }
        if !trail.possible.contains(&digest) {
            trail.possible.push(digest);
        }
        self.save()
    }

    /// A rotation of `account` into `next_key` is about to be signed, with
    /// these digests. Saved before it signs.
    pub(super) fn note_rotation(
        &mut self,
        account: &Pubkey,
        next_key: &[u8],
        digests: &[[u8; 32]],
    ) -> Result<(), VaultError> {
        let id = account.to_string();
        let next_key = hex::encode(next_key);
        let rotation = self
            .marks
            .rotations
            .entry(id.clone())
            .or_insert_with(|| Rotation {
                next_key: next_key.clone(),
                digests: Vec::new(),
            });
        if rotation.next_key != next_key {
            *rotation = Rotation {
                next_key,
                digests: Vec::new(),
            };
        }
        for digest in digests.iter().map(hex::encode) {
            if !rotation.digests.contains(&digest) {
                rotation.digests.push(digest);
            }
        }
        // A rotation may land too, so the account may be at its digests.
        if let Some(trail) = self.marks.trails.get_mut(&id) {
            for digest in digests.iter().map(hex::encode) {
                if !trail.possible.contains(&digest) {
                    trail.possible.push(digest);
                }
            }
        }
        self.save()
    }

    /// The key `account` moved to, when `nonce` is the digest of a rotation
    /// signed here: only that rotation landing leaves it there.
    pub fn rotation_landed(&self, account: &Pubkey, nonce: &[u8; 32]) -> Option<Vec<u8>> {
        let rotation = self.marks.rotations.get(&account.to_string())?;
        rotation
            .digests
            .contains(&hex::encode(nonce))
            .then(|| hex::decode(&rotation.next_key).ok())
            .flatten()
    }

    /// The rotation of `account` landed and left it at `nonce`.
    pub(super) fn settle_rotation(
        &mut self,
        account: &Pubkey,
        nonce: &[u8; 32],
    ) -> Result<(), VaultError> {
        let id = account.to_string();
        self.marks.rotations.remove(&id);
        let nonce = hex::encode(nonce);
        self.marks.trails.insert(
            id,
            Trail {
                root: nonce.clone(),
                possible: vec![nonce],
            },
        );
        self.save()
    }

    fn save(&mut self) -> Result<(), VaultError> {
        check_saveable(&self.problem)?;
        let json =
            serde_json::to_vec_pretty(&self.marks).map_err(|e| VaultError::Serde(e.to_string()))?;
        self.store.save(&json)
    }
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    hex::decode(text).ok()?.try_into().ok()
}

/// What a hash-based key may sign with now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reserved {
    pub leaf: u32,
    /// The salt to sign with, when it was chosen before signing.
    pub salt: Option<[u8; XMSS_SALT_LEN]>,
    /// The mark was missing, so leaves were passed over. Worth telling the
    /// user, since it uses up the key faster.
    pub skipped: bool,
}

#[derive(Debug, Error)]
pub enum LeafError {
    #[error("Unlock the vault first.")]
    Locked,
    #[error("That wallet is not on this device.")]
    NoWallet,
    #[error("{0} does not keep a hash-based key here.")]
    NotStateful(String),
    #[error("Every leaf of the key of {0} is used. It cannot sign again.")]
    Exhausted(String),
    #[error("The Winternitz key of {0} already signed another transaction, and a second signature would let anyone forge a third. Let the transaction it signed land, then install the next key.")]
    OneTimeSpent(String),
    #[error("The leaf to sign with could not be recorded, so nothing was signed. {0}")]
    Mark(VaultError),
    #[error("The vault could not be saved after a leaf was recorded, so nothing was signed. {0}")]
    Vault(VaultError),
    #[error("{0} has not signed a transaction on this device yet, so this device does not know its nonce. Scan its account state from the online device first, or sign one transaction for it, then rotate.")]
    NoTrail(String),
    #[error("{0} was restored from its recovery phrase. A lost copy may have signed with leaves of its current key that the chain has not seen, so it signs nothing until it has moved to its next key: scan its account state from the online device, then Rotate key.")]
    RestoredMustRotate(String),
    #[error("{0} has used all but the last few leaves of its key. Those are kept for moving to its next key, so it signs nothing else: Rotate key on its row.")]
    MustRotate(String),
    #[error("{0}")]
    Rotation(String),
}

/// Record what `wallet`'s hash-based key `signer` signs `digest` with,
/// before it signs. `chain_floor` is the account's `next_leaf` on chain,
/// when known, and `purpose` whether the digest moves the account to its
/// next key.
pub fn reserve<V: VaultStore, M: VaultStore>(
    vault: &mut Vault<V>,
    marks: &mut LeafMarks<M>,
    wallet: WalletId,
    signer: &VectorSigner,
    digest: &[u8; 32],
    chain_floor: Option<u32>,
    purpose: LeafFor,
) -> Result<Reserved, LeafError> {
    let key = signer.key();
    let data = vault.data().map_err(|_| LeafError::Locked)?;
    let record = data
        .wallets
        .iter()
        .find(|record| record.id == wallet)
        .ok_or(LeafError::NoWallet)?;
    match record
        .kind
        .vector_settings()
        .map(|settings| settings.scheme)
    {
        Some(SchemeId::Xmss) => reserve_leaf(vault, marks, wallet, &key, chain_floor, purpose),
        Some(SchemeId::Winternitz) => {
            let label = record.label.clone();
            let account = record.address().ok_or(LeafError::NoWallet)?;
            let salt = marks
                .spend_once(&account, &key, digest, || {
                    signer
                        .accepted_salt(0, digest)
                        .map_err(|err| VaultError::Crypto(err.to_string()))
                })
                .map_err(LeafError::Mark)?
                .ok_or(LeafError::OneTimeSpent(label))?;
            Ok(Reserved {
                leaf: 0,
                salt: Some(salt),
                skipped: false,
            })
        }
        _ => Err(LeafError::NotStateful(record.label.clone())),
    }
}

/// Reserve the next leaf of `wallet`'s XMSS key `key`, before it signs.
///
/// The leaf is the highest of the record's `next_leaf`, the key's mark, and
/// the account's `next_leaf` on chain when it is known. Both counters are
/// moved past it and saved before this returns it. Past
/// [`XMSS_FORCE_ROTATE_USED`] only a [`LeafFor::Rotation`] gets a leaf, and
/// a refusal records nothing.
pub fn reserve_leaf<V: VaultStore, M: VaultStore>(
    vault: &mut Vault<V>,
    marks: &mut LeafMarks<M>,
    wallet: WalletId,
    key: &[u8],
    chain_floor: Option<u32>,
    purpose: LeafFor,
) -> Result<Reserved, LeafError> {
    let data = vault.data_mut().map_err(|_| LeafError::Locked)?;
    let record = data
        .wallets
        .iter_mut()
        .find(|record| record.id == wallet)
        .ok_or(LeafError::NoWallet)?;
    let label = record.label.clone();
    let account = record.address();
    let (
        Some(account),
        WalletKind::Programmatic {
            protocol: Protocol::Vector { scheme, .. },
            authority: Authority::Held { next_leaf, .. },
            ..
        },
    ) = (account, &mut record.kind)
    else {
        return Err(LeafError::NotStateful(label));
    };
    if *scheme != SchemeId::Xmss {
        return Err(LeafError::NotStateful(label));
    }

    let mark = marks.next_leaf(&account, key);
    let skipped = mark.is_none() && *next_leaf > 0;
    let from_record = if skipped {
        next_leaf.saturating_add(LEAVES_SKIPPED_WITHOUT_MARK)
    } else {
        *next_leaf
    };
    let leaf = from_record
        .max(mark.unwrap_or(0))
        .max(chain_floor.unwrap_or(0));
    if leaf >= XMSS_LEAVES {
        return Err(LeafError::Exhausted(label));
    }
    if purpose == LeafFor::Other && leaf >= XMSS_FORCE_ROTATE_USED {
        return Err(LeafError::MustRotate(label));
    }

    marks
        .raise(&account, key, leaf + 1)
        .map_err(LeafError::Mark)?;
    // Not through `Vault::change`, which would put the counter back when the
    // save fails.
    *next_leaf = leaf + 1;
    vault.save().map_err(LeafError::Vault)?;
    Ok(Reserved {
        leaf,
        salt: None,
        skipped,
    })
}

/// The keys of this device that sign for Vector accounts in `wire`, with
/// what each hash-based one signs with recorded first.
///
/// Only an account that signs `wire` now records anything: one whose place
/// in the `Advance` is empty, whose nonce `known` holds, and that is not
/// waiting to sign last. An XMSS retry over the same digest takes its leaf
/// and salt from `cache` and records nothing new.
///
/// `signers` are the keys that sign in the same press. The Vector digest
/// covers each `Submit`'s signature, so the `Submit`s they are the authority
/// of are signed first, here too: an Ed25519 signature is the same every
/// time, so the digest recorded is the one the press signs.
pub fn keys_to_sign_with<V: VaultStore, M: VaultStore>(
    vault: &mut Vault<V>,
    marks: &mut LeafMarks<M>,
    cache: &XmssRetryCache,
    wire: &[u8],
    signers: &[&dyn WalletSigner],
    known: &[VectorContext],
) -> Result<Vec<VectorKey>, LeafError> {
    install_landed_rotations(vault, marks, known)?;
    let data = vault.data().map_err(|_| LeafError::Locked)?;
    let mut keys = local_vector_keys(&data.wallets);
    let held: Vec<Pubkey> = keys.iter().map(|key| key.account).collect();
    // A wire that cannot be signed records nothing here, and fails to sign.
    let submitted = sign_authorizations_in(wire, signers, &AuthContext { vector: known })
        .map_or_else(|_| wire.to_vec(), |authorized| authorized.wire);
    let now = to_sign_now(&submitted, &held, known);
    for key in &mut keys {
        if !key.signer.scheme().is_stateful() {
            continue;
        }
        let Some(to_sign) = now.get(&key.account) else {
            continue;
        };
        let digest = &to_sign.digest;
        let nonce = known
            .iter()
            .find(|known| known.account == key.account)
            .and_then(|known| known.nonce);
        if let Some(cached) = cache.get(&key.account, digest) {
            key.leaf = Some(cached.leaf);
            key.salt = Some(cached.salt);
            continue;
        }
        let record = vault
            .data()
            .map_err(|_| LeafError::Locked)?
            .wallets
            .iter()
            .find(|wallet| wallet.address() == Some(key.account))
            .ok_or(LeafError::NoWallet)?;
        if record.vector_phrase().is_some_and(|phrase| phrase.restored) {
            return Err(LeafError::RestoredMustRotate(record.label.clone()));
        }
        // What signing would refuse is refused before anything is recorded:
        // a Winternitz digest recorded and then not signed would leave the
        // key unable to sign any other.
        refuse_handover(&to_sign.next_key, key, &AuthContext { vector: known })
            .map_err(|err| LeafError::Rotation(err.to_string()))?;
        let purpose = if to_sign.next_key == key.signer.key() {
            LeafFor::Other
        } else {
            LeafFor::Rotation
        };
        let wallet = record.id;
        let floor = known
            .iter()
            .find(|known| known.account == key.account)
            .map(|known| known.leaf_floor);
        let reserved = reserve(vault, marks, wallet, &key.signer, digest, floor, purpose)?;
        if let Some(nonce) = nonce {
            marks
                .note_signed(&key.account, &nonce, digest)
                .map_err(LeafError::Mark)?;
        }
        key.leaf = Some(reserved.leaf);
        key.salt = reserved.salt;
    }
    Ok(keys)
}

/// Install the next key of each account of `known` whose nonce is the
/// digest of a rotation signed here: only that rotation landing leaves the
/// account there. Answers the wallets that moved on.
///
/// The nonce comes from the chain, or from the note of the device that read
/// it. A note that names a rotation's digest before it landed makes this
/// device sign with the next key too early, and those signatures fail on
/// chain. The key it held before is kept in the record for that case.
pub fn install_landed_rotations<V: VaultStore, M: VaultStore>(
    vault: &mut Vault<V>,
    marks: &mut LeafMarks<M>,
    known: &[VectorContext],
) -> Result<Vec<WalletId>, LeafError> {
    let mut installed = Vec::new();
    for context in known {
        let Some(nonce) = context.nonce else {
            continue;
        };
        let Some(next_key) = marks.rotation_landed(&context.account, &nonce) else {
            continue;
        };
        let data = vault.data().map_err(|_| LeafError::Locked)?;
        let Some(id) = data
            .wallets
            .iter()
            .find(|wallet| wallet.address() == Some(context.account))
            .map(|wallet| wallet.id)
        else {
            continue;
        };
        // Installed already, when only the marks could not be saved then.
        let current = VectorSigner::from_record(&data.wallets, id)
            .map(|signer| signer.key())
            .ok();
        if current.as_deref() != Some(next_key.as_slice()) {
            let mut refused = None;
            vault
                .change(|data| {
                    if let Some(wallet) = data.wallets.iter_mut().find(|wallet| wallet.id == id) {
                        refused = wallet.install_vector_next_secret(&next_key).err();
                    }
                })
                .map_err(LeafError::Vault)?;
            if let Some(err) = refused {
                return Err(LeafError::Rotation(err.to_string()));
            }
        }
        marks
            .settle_rotation(&context.account, &nonce)
            .map_err(LeafError::Mark)?;
        installed.push(id);
    }
    Ok(installed)
}
