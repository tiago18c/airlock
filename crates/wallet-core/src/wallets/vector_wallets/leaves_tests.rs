//! Tests of what a hash-based key has signed.

use super::leaves::*;
use crate::fixtures::pk;
use crate::ids::WalletId;
use crate::wallets::record::WalletRecord;
use crate::wallets::record::{Authority, WalletKind};
use crate::wallets::vault::Vault;
use crate::wallets::vector::{
    winternitz_from_seed, SchemeId, VectorSigner, PROGRAM_ID, XMSS_LEAVES,
};
use solana_pubkey::Pubkey;
use std::sync::Arc;
use wallet_vault::store::{LoadProblem, VaultStore};
use wallet_vault::{MemoryStore, SecretBytes};

/// The key the wallet's account signs with now.
const KEY: [u8; 41] = [3; 41];

/// A Winternitz key made from `seed`.
fn one_time(seed: u8) -> VectorSigner {
    VectorSigner::Winternitz(Box::new(winternitz_from_seed(&[seed; 32]).unwrap()))
}

/// A device with one XMSS wallet, its vault and its marks, each in a
/// store that outlives a restart.
pub(super) struct Device {
    pub(super) vault_file: Arc<MemoryStore>,
    pub(super) marks_file: Arc<MemoryStore>,
    pub(super) vault: Vault<Arc<MemoryStore>>,
    pub(super) marks: LeafMarks<Arc<MemoryStore>>,
    pub(super) wallet: WalletId,
    pub(super) account: Pubkey,
}

impl Device {
    pub(super) fn new(next_leaf: u32) -> Self {
        Self::of(SchemeId::Xmss, next_leaf)
    }

    fn of(scheme: SchemeId, next_leaf: u32) -> Self {
        let vault_file = Arc::new(MemoryStore::new());
        let marks_file = Arc::new(MemoryStore::new());
        let mut vault = Vault::new(vault_file.clone());
        vault.create("pw").unwrap();
        let record = WalletRecord::vector(
            "Quantum",
            scheme,
            PROGRAM_ID,
            vec![7; scheme.key_len()],
            Authority::Held {
                secret: SecretBytes(vec![1; 32]),
                next_leaf,
            },
        );
        let (wallet, account) = (record.id, record.address().unwrap());
        vault.change(|data| data.wallets.push(record)).unwrap();
        Self {
            marks: LeafMarks::open(marks_file.clone()),
            vault_file,
            marks_file,
            vault,
            wallet,
            account,
        }
    }

    fn reserve(&mut self, chain_floor: Option<u32>) -> Result<Reserved, LeafError> {
        reserve_leaf(
            &mut self.vault,
            &mut self.marks,
            self.wallet,
            &KEY,
            chain_floor,
        )
    }

    /// Start again from what the stores hold.
    fn restart(&mut self) {
        self.vault = Vault::new(self.vault_file.clone());
        self.vault.unlock("pw").unwrap();
        self.marks = LeafMarks::open(self.marks_file.clone());
    }

    fn reserve_for(
        &mut self,
        signer: &VectorSigner,
        digest: [u8; 32],
    ) -> Result<Reserved, LeafError> {
        reserve(
            &mut self.vault,
            &mut self.marks,
            self.wallet,
            signer,
            &digest,
            None,
        )
    }

    /// Put the record's counter back to the first leaf, as installing a
    /// new key does.
    fn reset_record(&mut self) {
        if let WalletKind::Programmatic {
            authority: Authority::Held { next_leaf, .. },
            ..
        } = &mut self.vault.data_mut().unwrap().wallets[0].kind
        {
            *next_leaf = 0;
        }
        self.vault.save().unwrap();
    }

    pub(super) fn stored_next_leaf(&self) -> u32 {
        let mut vault = Vault::new(self.vault_file.clone());
        vault.unlock("pw").unwrap();
        match &vault.data().unwrap().wallets[0].kind {
            WalletKind::Programmatic {
                authority: Authority::Held { next_leaf, .. },
                ..
            } => *next_leaf,
            _ => unreachable!(),
        }
    }
}

#[test]
fn each_reservation_is_a_new_leaf_and_both_counters_move_past_it() {
    let mut device = Device::new(0);
    assert_eq!(device.reserve(None).unwrap().leaf, 0);
    assert_eq!(device.reserve(None).unwrap().leaf, 1);
    assert_eq!(device.marks.next_leaf(&device.account, &KEY), Some(2));
    assert_eq!(device.stored_next_leaf(), 2);
    // The floor on chain wins when it is higher, and is passed when lower.
    assert_eq!(device.reserve(Some(10)).unwrap().leaf, 10);
    assert_eq!(device.reserve(Some(3)).unwrap().leaf, 11);
}

#[test]
fn when_the_mark_cannot_be_saved_nothing_signs_and_the_leaf_is_not_given_again() {
    let mut device = Device::new(0);
    device.reserve(None).unwrap();
    device.marks_file.refuse_saves(true);
    assert!(matches!(device.reserve(None), Err(LeafError::Mark(_))));
    // The vault was not touched.
    assert_eq!(device.stored_next_leaf(), 1);
    device.marks_file.refuse_saves(false);
    // Leaf 1 may have been about to sign, so it is passed over.
    assert_eq!(device.reserve(None).unwrap().leaf, 2);
}

#[test]
fn when_the_vault_cannot_be_saved_after_the_mark_the_mark_still_holds_after_a_restart() {
    let mut device = Device::new(0);
    device.vault_file.refuse_saves(true);
    assert!(matches!(device.reserve(None), Err(LeafError::Vault(_))));
    device.vault_file.refuse_saves(false);
    device.restart();
    // The vault on disk still says 0. The mark says 1.
    assert_eq!(device.stored_next_leaf(), 0);
    assert_eq!(device.reserve(None).unwrap().leaf, 1);
}

#[test]
fn a_leaf_reserved_before_a_crash_is_not_given_again_after_it() {
    // Both saved, then the app stopped before the signature was shown.
    let mut device = Device::new(0);
    assert_eq!(device.reserve(None).unwrap().leaf, 0);
    device.restart();
    assert_eq!(device.reserve(None).unwrap().leaf, 1);
}

#[test]
fn an_older_vault_is_overruled_by_the_mark() {
    let mut device = Device::new(0);
    let older = device.vault_file.load().unwrap().unwrap();
    for _ in 0..5 {
        device.reserve(None).unwrap();
    }
    // The vault comes back from a copy taken before those five.
    device.vault_file.save(&older).unwrap();
    device.restart();
    assert_eq!(device.stored_next_leaf(), 0);
    let next = device.reserve(None).unwrap();
    assert_eq!(
        next,
        Reserved {
            leaf: 5,
            salt: None,
            skipped: false
        }
    );
}

#[test]
fn a_missing_mark_skips_leaves_after_what_the_record_says() {
    let mut device = Device::new(3);
    // No mark was ever written for a record that has signed.
    let next = device.reserve(None).unwrap();
    assert_eq!(
        next,
        Reserved {
            leaf: 3 + LEAVES_SKIPPED_WITHOUT_MARK,
            salt: None,
            skipped: true
        }
    );
    // The mark now exists, so nothing more is skipped.
    assert_eq!(
        device.reserve(None).unwrap(),
        Reserved {
            leaf: 4 + LEAVES_SKIPPED_WITHOUT_MARK,
            salt: None,
            skipped: false
        }
    );
    // A key that never signed starts at its first leaf.
    assert_eq!(
        Device::new(0).reserve(None).unwrap(),
        Reserved {
            leaf: 0,
            salt: None,
            skipped: false
        }
    );
}

#[test]
fn a_mark_file_that_cannot_be_read_stops_signing_and_is_left_as_it_is() {
    let mut device = Device::new(2);
    device.marks_file.save(b"not json").unwrap();
    device.restart();
    assert!(matches!(
        device.marks.load_problem(),
        Some(LoadProblem::Blocked(_))
    ));
    // Starting empty would let a used leaf sign again.
    assert!(matches!(device.reserve(None), Err(LeafError::Mark(_))));
    assert_eq!(device.stored_next_leaf(), 2);
    assert_eq!(device.marks_file.load().unwrap().unwrap(), b"not json");

    // Once it reads again, signing goes on from it.
    device.marks_file.save(b"{}").unwrap();
    device.restart();
    assert!(device.marks.load_problem().is_none());
    let next = device.reserve(None).unwrap();
    assert_eq!(next.leaf, 2 + LEAVES_SKIPPED_WITHOUT_MARK);
}

#[test]
fn the_last_leaf_signs_and_then_nothing_does() {
    let mut device = Device::new(XMSS_LEAVES - 1);
    let account = device.account;
    device.marks.raise(&account, &KEY, XMSS_LEAVES - 1).unwrap();
    assert_eq!(device.reserve(None).unwrap().leaf, XMSS_LEAVES - 1);
    let before = device.vault_file.load().unwrap();
    assert!(matches!(
        device.reserve(None),
        Err(LeafError::Exhausted(label)) if label == "Quantum"
    ));
    // Nothing was written by the refusal.
    assert_eq!(device.vault_file.load().unwrap(), before);
    // A floor past the end is refused the same way.
    let mut fresh = Device::new(0);
    assert!(matches!(
        fresh.reserve(Some(XMSS_LEAVES)),
        Err(LeafError::Exhausted(_))
    ));
}

#[test]
fn only_a_held_xmss_key_has_leaves() {
    let mut device = Device::new(0);
    let ed25519 = WalletRecord::vector(
        "Plain",
        SchemeId::Ed25519,
        pk(50),
        pk(5).to_bytes().to_vec(),
        Authority::Held {
            secret: SecretBytes(vec![1; 32]),
            next_leaf: 0,
        },
    );
    let outside = WalletRecord::vector(
        "Elsewhere",
        SchemeId::Xmss,
        pk(50),
        vec![8; SchemeId::Xmss.key_len()],
        Authority::Outside,
    );
    let ids = [ed25519.id, outside.id];
    device
        .vault
        .change(|data| data.wallets.extend([ed25519, outside]))
        .unwrap();
    for id in ids {
        assert!(matches!(
            reserve_leaf(&mut device.vault, &mut device.marks, id, &KEY, None),
            Err(LeafError::NotStateful(_))
        ));
    }
    assert!(matches!(
        reserve_leaf(
            &mut device.vault,
            &mut device.marks,
            WalletId::new(),
            &KEY,
            None
        ),
        Err(LeafError::NoWallet)
    ));
    device.vault.lock();
    assert!(matches!(device.reserve(None), Err(LeafError::Locked)));
}

#[test]
fn a_key_that_replaces_a_used_up_one_starts_at_its_first_leaf() {
    let mut device = Device::new(XMSS_LEAVES - 1);
    let account = device.account;
    device.marks.raise(&account, &KEY, XMSS_LEAVES - 1).unwrap();
    assert_eq!(device.reserve(None).unwrap().leaf, XMSS_LEAVES - 1);
    assert!(matches!(device.reserve(None), Err(LeafError::Exhausted(_))));

    // The account rotated to another key, and the record was reset with it.
    device.reset_record();
    let next = [4u8; 41];
    let first = reserve_leaf(
        &mut device.vault,
        &mut device.marks,
        device.wallet,
        &next,
        Some(0),
    )
    .unwrap();
    assert_eq!(
        first,
        Reserved {
            leaf: 0,
            salt: None,
            skipped: false
        }
    );
    device.restart();
    assert_eq!(device.marks.next_leaf(&account, &next), Some(1));
    // The old key's mark still holds.
    assert_eq!(device.marks.next_leaf(&account, &KEY), Some(XMSS_LEAVES));
}

#[test]
fn a_winternitz_key_signs_one_digest_with_one_salt_even_after_a_restart() {
    let mut device = Device::of(SchemeId::Winternitz, 0);
    let key = one_time(1);
    let first = device.reserve_for(&key, [1; 32]).unwrap();
    assert_eq!(first.leaf, 0);
    let salt = first.salt.expect("the salt is chosen before signing");
    // The same digest again: the same salt, so the same signature.
    assert_eq!(device.reserve_for(&key, [1; 32]).unwrap().salt, Some(salt));
    // A salt the key accepts: it signs.
    assert!(key.sign_leaf_with_salt(0, &[1; 32], &salt).is_ok());
    device.restart();
    assert_eq!(device.reserve_for(&key, [1; 32]).unwrap().salt, Some(salt));
    // Another digest would be a second signature by a one-time key.
    assert!(matches!(
        device.reserve_for(&key, [2; 32]),
        Err(LeafError::OneTimeSpent(label)) if label == "Quantum"
    ));
    // The key it rotates into is free.
    assert!(device
        .reserve_for(&one_time(4), [2; 32])
        .unwrap()
        .salt
        .is_some());
}

#[test]
fn a_winternitz_digest_that_could_not_be_recorded_is_not_signed() {
    let mut device = Device::of(SchemeId::Winternitz, 0);
    let key = one_time(1);
    device.marks_file.refuse_saves(true);
    assert!(matches!(
        device.reserve_for(&key, [1; 32]),
        Err(LeafError::Mark(_))
    ));
    device.marks_file.refuse_saves(false);
    // What was chosen in memory still binds the key to that digest.
    assert!(matches!(
        device.reserve_for(&key, [2; 32]),
        Err(LeafError::OneTimeSpent(_))
    ));
    assert!(device.reserve_for(&key, [1; 32]).is_ok());
}

#[test]
fn only_hash_based_keys_reserve() {
    let mut device = Device::of(SchemeId::Ed25519, 0);
    assert!(matches!(
        device.reserve_for(&one_time(1), [1; 32]),
        Err(LeafError::NotStateful(_))
    ));
}

#[test]
fn the_nonce_is_where_the_last_signature_was_made_or_one_of_its_digests() {
    let mut marks = LeafMarks::open(MemoryStore::new());
    let account = pk(7);
    assert!(marks.possible_nonces(&account).is_empty());

    // Two signatures at one nonce: the account is at it or at either digest.
    marks.note_signed(&account, &[1; 32], &[2; 32]).unwrap();
    marks.note_signed(&account, &[1; 32], &[3; 32]).unwrap();
    assert_eq!(
        marks.possible_nonces(&account),
        vec![[1; 32], [2; 32], [3; 32]]
    );

    // Signed at one of them, the others are void.
    marks.note_signed(&account, &[2; 32], &[4; 32]).unwrap();
    assert_eq!(marks.possible_nonces(&account), vec![[2; 32], [4; 32]]);

    // A rotation may land too, and only its digest says it did.
    marks.note_rotation(&account, &[9; 41], &[[5; 32]]).unwrap();
    assert_eq!(
        marks.possible_nonces(&account),
        vec![[2; 32], [4; 32], [5; 32]]
    );
    assert_eq!(marks.pending_rotation(&account), Some(vec![9; 41]));
    assert_eq!(marks.rotation_landed(&account, &[4; 32]), None);
    assert_eq!(marks.rotation_landed(&account, &[5; 32]), Some(vec![9; 41]));

    marks.settle_rotation(&account, &[5; 32]).unwrap();
    assert_eq!(marks.pending_rotation(&account), None);
    assert_eq!(marks.possible_nonces(&account), vec![[5; 32]]);
}
