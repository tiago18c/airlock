//! Which keys one press of Sign uses, and what it records for them: only an
//! account that signs the transaction now records a leaf.

use super::leaves_tests::Device;
use super::{keys_to_sign_with, FloorsFrom};
use crate::fixtures::{blockhash, compile, pk, transfer};
use crate::tx::decode::decode_v1_transaction;
use crate::wallets::relay::{wrap_for_vector, VectorContext, VectorWrap};
use crate::wallets::vector::{xmss_from_seed, SchemeId, VectorKey, XmssRetryCache};

trait Press {
    fn keys_for(&mut self, wire: &[u8], known: &[VectorContext]) -> Vec<VectorKey>;
    fn wrapped_transfer(&self) -> (Vec<u8>, Vec<VectorContext>);
}

impl Press for Device {
    /// The keys one press of Sign would use for `wire`.
    fn keys_for(&mut self, wire: &[u8], known: &[VectorContext]) -> Vec<VectorKey> {
        keys_to_sign_with(
            &mut self.vault,
            &mut self.marks,
            &XmssRetryCache::default(),
            wire,
            &[],
            known,
            FloorsFrom::Chain,
        )
        .unwrap()
    }

    /// A transfer out of this device's account, wrapped for it, and what
    /// signing it needs to know of the account.
    fn wrapped_transfer(&self) -> (Vec<u8>, Vec<VectorContext>) {
        let site = compile(pk(1), vec![transfer(self.account, pk(5), 1)]).wire;
        let current = xmss_from_seed(&[1; 32])
            .unwrap()
            .verifying_key()
            .to_bytes()
            .to_vec();
        // The Advance names the key the account holds, which keeps it.
        let wrap = VectorWrap {
            account: self.account,
            scheme: SchemeId::Xmss,
            key: vec![7; SchemeId::Xmss.key_len()],
            next_key: Some(current.clone()),
        };
        let wire = wrap_for_vector(
            &decode_v1_transaction(&site).unwrap(),
            &[wrap],
            pk(1),
            blockhash(2),
        )
        .unwrap();
        let known = vec![VectorContext {
            account: self.account,
            scheme: SchemeId::Xmss,
            nonce: Some([3; 32]),
            key: Some(current),
            leaf_floor: 0,
        }];
        (wire, known)
    }
}

#[test]
fn a_transaction_that_does_not_need_the_key_spends_no_leaf() {
    let mut device = Device::new(0);
    let plain = compile(pk(1), vec![transfer(pk(1), pk(5), 1)]).wire;
    let keys = device.keys_for(&plain, &[]);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].leaf, None);
    assert_eq!(device.stored_next_leaf(), 0);
    // Nor does one whose place for the account is already signed, or not
    // even a transaction.
    let _ = device.keys_for(&[1, 2, 3], &[]);
    assert_eq!(device.stored_next_leaf(), 0);
}

#[test]
fn a_transaction_the_key_signs_reserves_one_leaf() {
    let mut device = Device::new(0);
    let (wire, known) = device.wrapped_transfer();
    let keys = device.keys_for(&wire, &known);
    assert_eq!(keys[0].leaf, Some(0));
    assert_eq!(device.stored_next_leaf(), 1);
    // Without the nonce nothing could be signed, so nothing is reserved.
    let mut unknown = known.clone();
    unknown[0].nonce = None;
    assert_eq!(device.keys_for(&wire, &unknown)[0].leaf, None);
    assert_eq!(device.stored_next_leaf(), 1);
}

#[test]
fn a_floor_past_what_this_device_has_used_is_not_taken_from_a_note() {
    let mut device = Device::new(0);
    let (wire, mut known) = device.wrapped_transfer();
    known[0].leaf_floor = 200;
    let press = |device: &mut Device, known: &[VectorContext]| {
        keys_to_sign_with(
            &mut device.vault,
            &mut device.marks,
            &XmssRetryCache::default(),
            &wire,
            &[],
            known,
            FloorsFrom::Note,
        )
    };
    let refused = press(&mut device, &known);
    assert!(matches!(
        refused,
        Err(super::LeafError::FloorPastOwnCount {
            floor: 200,
            own: 0,
            ..
        })
    ));
    // Nothing was spent.
    assert_eq!(device.stored_next_leaf(), 0);
    assert_eq!(device.marks.next_leaf(&device.account, &device_key()), None);

    // A floor at or below this device's count is what signing leaves behind.
    known[0].leaf_floor = 0;
    let keys = press(&mut device, &known).unwrap();
    assert_eq!(keys[0].leaf, Some(0));

    // The account state says so on purpose, and then the floor is its own.
    device
        .marks
        .raise(&device.account, &device_key(), 200)
        .unwrap();
    known[0].leaf_floor = 200;
    let keys = press(&mut device, &known).unwrap();
    assert_eq!(keys[0].leaf, Some(200));
}

#[test]
fn a_floor_from_the_chain_does_not_spend_the_leaves_kept_for_rotating() {
    let mut device = Device::new(0);
    let (wire, mut known) = device.wrapped_transfer();
    // The chain accepts only the last leaf.
    known[0].leaf_floor = crate::wallets::vector::XMSS_LEAVES - 1;
    let refused = keys_to_sign_with(
        &mut device.vault,
        &mut device.marks,
        &XmssRetryCache::default(),
        &wire,
        &[],
        &known,
        FloorsFrom::Chain,
    );
    assert!(matches!(refused, Err(super::LeafError::MustRotate(_))));
    assert_eq!(device.stored_next_leaf(), 0);
    assert_eq!(device.marks.next_leaf(&device.account, &device_key()), None);
}

#[test]
fn a_hand_over_to_a_key_not_made_here_records_nothing() {
    let mut device = Device::new(0);
    let site = compile(pk(1), vec![transfer(device.account, pk(5), 1)]).wire;
    let wrap = VectorWrap {
        account: device.account,
        scheme: SchemeId::Xmss,
        key: vec![7; SchemeId::Xmss.key_len()],
        // Someone else's key.
        next_key: Some(vec![9; SchemeId::Xmss.key_len()]),
    };
    let wire = wrap_for_vector(
        &decode_v1_transaction(&site).unwrap(),
        &[wrap],
        pk(1),
        blockhash(2),
    )
    .unwrap();
    let (_, known) = device.wrapped_transfer();
    let refused = keys_to_sign_with(
        &mut device.vault,
        &mut device.marks,
        &XmssRetryCache::default(),
        &wire,
        &[],
        &known,
        FloorsFrom::Chain,
    );
    assert!(matches!(refused, Err(super::LeafError::Rotation(_))));
    assert_eq!(device.stored_next_leaf(), 0);
    assert!(device.marks.possible_nonces(&device.account).is_empty());
}

/// The key the device's wallet signs with: its seed is `[1; 32]`.
fn device_key() -> Vec<u8> {
    xmss_from_seed(&[1; 32])
        .unwrap()
        .verifying_key()
        .to_bytes()
        .to_vec()
}
