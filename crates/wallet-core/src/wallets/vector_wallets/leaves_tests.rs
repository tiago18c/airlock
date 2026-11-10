//! Tests of what a hash-based key has signed.

use super::leaves::*;
use crate::fixtures::pk;
use crate::ids::WalletId;
use crate::wallets::record::WalletRecord;
use crate::wallets::record::{Authority, WalletKind};
use crate::wallets::vault::Vault;
use crate::wallets::vector::{
    winternitz_from_seed, SchemeId, VectorSigner, PROGRAM_ID, XMSS_FORCE_ROTATE_USED, XMSS_LEAVES,
    XMSS_RECOVERY_FROM, XMSS_RECOVERY_LEAVES,
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
            LeafFor::Other,
        )
    }

    /// A leaf for a move to the next key, which may use the last leaves.
    fn reserve_rotation(&mut self, chain_floor: Option<u32>) -> Result<Reserved, LeafError> {
        reserve_leaf(
            &mut self.vault,
            &mut self.marks,
            self.wallet,
            &KEY,
            chain_floor,
            LeafFor::Rotation,
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
            LeafFor::Other,
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
fn the_last_leaf_below_those_kept_for_recovery_signs_and_then_nothing_does() {
    let last = XMSS_RECOVERY_FROM - 1;
    let mut device = Device::new(last);
    let account = device.account;
    device.marks.raise(&account, &KEY, last).unwrap();
    assert_eq!(device.reserve_rotation(None).unwrap().leaf, last);
    let before = device.vault_file.load().unwrap();
    let marks_before = device.marks_file.load().unwrap();
    // Not even a rotation gets one of the leaves kept for recovery, on a
    // device that kept its key.
    for _ in 0..2 {
        assert!(matches!(
            device.reserve_rotation(None),
            Err(LeafError::KeptForRecovery(label)) if label == "Quantum"
        ));
    }
    // Nothing was written by the refusal.
    assert_eq!(device.vault_file.load().unwrap(), before);
    assert_eq!(device.marks_file.load().unwrap(), marks_before);
    // Nor when the chain's floor is among them.
    let mut told = Device::new(0);
    assert!(matches!(
        told.reserve_rotation(Some(XMSS_RECOVERY_FROM)),
        Err(LeafError::KeptForRecovery(_))
    ));
    assert_eq!(told.marks.next_leaf(&told.account, &KEY), None);
    // A floor past the end is refused the same way.
    let mut fresh = Device::new(0);
    assert!(matches!(
        fresh.reserve_rotation(Some(XMSS_LEAVES)),
        Err(LeafError::Exhausted(_))
    ));
}

#[test]
fn the_last_leaves_are_kept_for_the_move_to_the_next_key() {
    let mut device = Device::new(XMSS_FORCE_ROTATE_USED - 1);
    let account = device.account;
    device
        .marks
        .raise(&account, &KEY, XMSS_FORCE_ROTATE_USED - 1)
        .unwrap();
    assert_eq!(
        device.reserve(None).unwrap().leaf,
        XMSS_FORCE_ROTATE_USED - 1
    );
    let before = (
        device.vault_file.load().unwrap(),
        device.marks_file.load().unwrap(),
    );
    assert!(matches!(
        device.reserve(None),
        Err(LeafError::MustRotate(label)) if label == "Quantum"
    ));
    // The refusal recorded nothing, so the leaves are still there.
    assert_eq!(
        (
            device.vault_file.load().unwrap(),
            device.marks_file.load().unwrap()
        ),
        before
    );
    assert_eq!(
        device.reserve_rotation(None).unwrap().leaf,
        XMSS_FORCE_ROTATE_USED
    );

    // A floor told by another device cannot use them up either: past the
    // kept leaves it is refused, and the rotation still has every one.
    let mut told = Device::new(0);
    assert!(matches!(
        told.reserve(Some(XMSS_LEAVES - 1)),
        Err(LeafError::MustRotate(_))
    ));
    assert_eq!(told.stored_next_leaf(), 0);
    assert_eq!(told.reserve_rotation(None).unwrap().leaf, 0);
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
            reserve_leaf(
                &mut device.vault,
                &mut device.marks,
                id,
                &KEY,
                None,
                LeafFor::Other
            ),
            Err(LeafError::NotStateful(_))
        ));
    }
    assert!(matches!(
        reserve_leaf(
            &mut device.vault,
            &mut device.marks,
            WalletId::new(),
            &KEY,
            None,
            LeafFor::Other
        ),
        Err(LeafError::NoWallet)
    ));
    device.vault.lock();
    assert!(matches!(device.reserve(None), Err(LeafError::Locked)));
}

#[test]
fn a_key_that_replaces_a_used_up_one_starts_at_its_first_leaf() {
    let last = XMSS_RECOVERY_FROM - 1;
    let mut device = Device::new(last);
    let account = device.account;
    device.marks.raise(&account, &KEY, last).unwrap();
    assert_eq!(device.reserve_rotation(None).unwrap().leaf, last);
    assert!(matches!(
        device.reserve_rotation(None),
        Err(LeafError::KeptForRecovery(_))
    ));

    // The account rotated to another key, and the record was reset with it.
    device.reset_record();
    let next = [4u8; 41];
    let first = reserve_leaf(
        &mut device.vault,
        &mut device.marks,
        device.wallet,
        &next,
        Some(0),
        LeafFor::Other,
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
    assert_eq!(marks.pending_rotations(), [(account, vec![9; 41])]);
    assert_eq!(marks.rotation_landed(&account, &[4; 32]), None);
    assert_eq!(marks.rotation_landed(&account, &[5; 32]), Some(vec![9; 41]));

    marks.settle_rotation(&account, &[5; 32]).unwrap();
    assert_eq!(marks.pending_rotation(&account), None);
    assert!(marks.pending_rotations().is_empty());
    assert_eq!(marks.possible_nonces(&account), vec![[5; 32]]);
}

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// A device the wallet was typed into again from its recovery phrase.
fn restored_device() -> (Device, Vec<u8>) {
    let mut device = Device::new(0);
    let record =
        WalletRecord::vector_from_phrase("Tree", SchemeId::Xmss, PROGRAM_ID, PHRASE, true).unwrap();
    device.wallet = record.id;
    device.account = record.address().unwrap();
    device
        .vault
        .change(|data| data.wallets = vec![record])
        .unwrap();
    let key = VectorSigner::from_record(&device.vault.data().unwrap().wallets, device.wallet)
        .unwrap()
        .key();
    (device, key)
}

#[test]
fn a_restored_wallet_takes_the_leaves_kept_for_it_from_the_last_one_down() {
    let (mut device, key) = restored_device();
    let wallet = device.wallet;
    let left = |device: &Device| recovery_attempts_left(&device.vault, &device.marks, wallet);
    assert_eq!(left(&device), Some(XMSS_RECOVERY_LEAVES));
    let mut given = Vec::new();
    for _ in 0..XMSS_RECOVERY_LEAVES {
        let reserved =
            reserve_recovery_leaf(&device.vault, &mut device.marks, wallet, &key).unwrap();
        given.push(reserved.leaf);
        // What is counted is on disk before the leaf is given, and a
        // restart does not give it again.
        device.restart();
    }
    assert_eq!(given, [255, 254, 253, 252]);
    assert_eq!(given.last(), Some(&XMSS_RECOVERY_FROM));
    assert_eq!(left(&device), Some(0));
    let marks_before = device.marks_file.load().unwrap();
    assert!(matches!(
        reserve_recovery_leaf(&device.vault, &mut device.marks, wallet, &key),
        Err(LeafError::RecoverySpent(label)) if label == "Tree"
    ));
    assert_eq!(device.marks_file.load().unwrap(), marks_before);
    // The leaves it may not use otherwise stay refused too.
    assert!(matches!(
        reserve_leaf(
            &mut device.vault,
            &mut device.marks,
            wallet,
            &key,
            Some(XMSS_RECOVERY_FROM),
            LeafFor::Rotation
        ),
        Err(LeafError::KeptForRecovery(_))
    ));
}

#[test]
fn a_leaf_kept_for_recovery_is_given_only_above_what_the_chain_has_seen() {
    let (mut device, key) = restored_device();
    let (wallet, account) = (device.wallet, device.account);
    // The chain says leaf 253 has signed: only the two above it are left.
    device.marks.raise(&account, &key, 254).unwrap();
    assert_eq!(
        recovery_attempts_left(&device.vault, &device.marks, wallet),
        Some(2)
    );
    for expected in [255, 254] {
        let reserved =
            reserve_recovery_leaf(&device.vault, &mut device.marks, wallet, &key).unwrap();
        assert_eq!(reserved.leaf, expected);
    }
    assert_eq!(
        recovery_attempts_left(&device.vault, &device.marks, wallet),
        Some(0)
    );
    assert!(matches!(
        reserve_recovery_leaf(&device.vault, &mut device.marks, wallet, &key),
        Err(LeafError::RecoverySpent(_))
    ));
    // A build before this one signed a restored wallet's rotation with the
    // last leaf and marked the whole key used: nothing is given after it.
    let (mut earlier, key) = restored_device();
    let account = earlier.account;
    earlier.marks.raise(&account, &key, XMSS_LEAVES).unwrap();
    assert_eq!(
        recovery_attempts_left(&earlier.vault, &earlier.marks, earlier.wallet),
        Some(0)
    );
    assert!(matches!(
        reserve_recovery_leaf(&earlier.vault, &mut earlier.marks, earlier.wallet, &key),
        Err(LeafError::RecoverySpent(_))
    ));
}

#[test]
fn only_a_restored_wallet_gets_a_leaf_kept_for_recovery() {
    let mut device = Device::new(0);
    assert_eq!(
        recovery_attempts_left(&device.vault, &device.marks, device.wallet),
        None
    );
    let before = device.marks_file.load().unwrap();
    assert!(matches!(
        reserve_recovery_leaf(&device.vault, &mut device.marks, device.wallet, &KEY),
        Err(LeafError::NotStateful(_))
    ));
    assert_eq!(device.marks_file.load().unwrap(), before);
    assert!(matches!(
        reserve_recovery_leaf(&device.vault, &mut device.marks, WalletId::new(), &KEY),
        Err(LeafError::NoWallet)
    ));
}

#[test]
fn what_a_key_used_of_its_recovery_leaves_outlives_its_rotation() {
    let (mut device, key) = restored_device();
    let (wallet, account) = (device.wallet, device.account);
    reserve_recovery_leaf(&device.vault, &mut device.marks, wallet, &key).unwrap();
    // The rotation it signed is recorded, kept and then settled, as when a
    // note says it landed. Told later that the account holds this key
    // after all, the leaf it used is still counted.
    let (next, nonce, digest) = ([9u8; 41], [1u8; 32], [2u8; 32]);
    device
        .marks
        .note_rotation(&account, &next, &[digest])
        .unwrap();
    device
        .marks
        .keep_rotation_signature(&account, &next, &nonce, &[5; 8])
        .unwrap();
    let kept = device.marks.kept_rotation(&account).unwrap();
    assert_eq!(kept.next_key, next);
    assert_eq!(kept.signed.len(), 1);
    assert_eq!(kept.signed[0].nonce, nonce);
    assert_eq!(device.marks.kept_rotations(), vec![kept]);
    device.marks.settle_rotation(&account, &digest).unwrap();
    assert_eq!(device.marks.kept_rotation(&account), None);
    device.restart();
    assert_eq!(device.marks.recovery_used(&account, &key), 1);
    assert_eq!(
        reserve_recovery_leaf(&device.vault, &mut device.marks, wallet, &key)
            .unwrap()
            .leaf,
        XMSS_LEAVES - 2
    );
}
