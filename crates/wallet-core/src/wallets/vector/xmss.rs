//! XMSS leaf policy, rotation thresholds, and the retry cache so a second
//! Sign press over the same digest does not spend another leaf.

use super::SchemeId;
use solana_pubkey::Pubkey;
use std::collections::HashMap;

/// Leaves of one XMSS key.
pub const XMSS_LEAVES: u32 = 256;

/// Bytes of the salt inside a Construction 3 signature (`ρ`).
pub const XMSS_SALT_LEN: usize = 21;

/// Offset of the salt in an encoded XMSS signature (after the leaf index).
pub const XMSS_SALT_OFFSET: usize = 4;

/// Offset of the salt in an encoded Winternitz signature (no leaf index).
pub const WINTERNITZ_SALT_OFFSET: usize = 0;

/// Warn when this many leaves of [`XMSS_LEAVES`] have been used.
pub const XMSS_WARN_USED: u32 = 205; // 80% of 256

/// Past this many used leaves, every wrap must rotate to a new key.
pub const XMSS_FORCE_ROTATE_USED: u32 = 244; // 95% of 256

/// How many leaves of one key have already signed, given the chain's floor.
pub fn leaves_used(next_leaf: u32) -> u32 {
    next_leaf.min(XMSS_LEAVES)
}

/// Leaves still available after `next_leaf`.
pub fn leaves_left(next_leaf: u32) -> u32 {
    XMSS_LEAVES.saturating_sub(next_leaf)
}

/// Whether the row (or Sign note) should warn that leaves are running low.
pub fn warn_leaves(next_leaf: u32) -> bool {
    leaves_used(next_leaf) >= XMSS_WARN_USED
}

/// Whether a wrap for this account must carry the next public key.
pub fn must_rotate(next_leaf: u32) -> bool {
    leaves_used(next_leaf) >= XMSS_FORCE_ROTATE_USED || next_leaf + 1 >= XMSS_LEAVES
}

/// The salt bytes inside a hash-based signature encoding.
pub fn salt_of_signature(scheme: SchemeId, signature: &[u8]) -> Option<[u8; XMSS_SALT_LEN]> {
    let offset = match scheme {
        SchemeId::Xmss => XMSS_SALT_OFFSET,
        SchemeId::Winternitz => WINTERNITZ_SALT_OFFSET,
        _ => return None,
    };
    signature
        .get(offset..offset + XMSS_SALT_LEN)?
        .try_into()
        .ok()
}

/// One successful XMSS attempt, so a retry over the same digest reuses the
/// leaf and salt instead of reserving again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmssRetry {
    pub leaf: u32,
    pub digest: [u8; 32],
    pub salt: [u8; XMSS_SALT_LEN],
}

/// Last XMSS attempt per Vector account on this device.
#[derive(Clone, Debug, Default)]
pub struct XmssRetryCache {
    entries: HashMap<Pubkey, XmssRetry>,
}

impl XmssRetryCache {
    pub fn get(&self, account: &Pubkey, digest: &[u8; 32]) -> Option<&XmssRetry> {
        self.entries
            .get(account)
            .filter(|entry| entry.digest == *digest)
    }

    pub fn remember(
        &mut self,
        account: Pubkey,
        leaf: u32,
        digest: [u8; 32],
        salt: [u8; XMSS_SALT_LEN],
    ) {
        self.entries
            .insert(account, XmssRetry { leaf, digest, salt });
    }

    pub fn forget(&mut self, account: &Pubkey) {
        self.entries.remove(account);
    }
}

/// A short note for the wallet row when leaves are low or rotation is forced.
pub fn leaf_status_note(scheme: SchemeId, next_leaf: u32) -> Option<String> {
    if scheme != SchemeId::Xmss {
        return None;
    }
    let left = leaves_left(next_leaf);
    if must_rotate(next_leaf) {
        Some(format!(
            "{left} leaf(ves) left. Only a move to a new XMSS key can be sent now: Rotate key on the row of the device that holds the key."
        ))
    } else if warn_leaves(next_leaf) {
        Some(format!(
            "{left} of {} leaves left. Rotate the key soon, from the device that holds it.",
            XMSS_LEAVES
        ))
    } else {
        None
    }
}
