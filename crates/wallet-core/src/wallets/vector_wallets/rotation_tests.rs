//! Tests of moving a hash-based account to its next key.

use super::{next_key_target, RotationError};
use crate::ids::WalletId;
use crate::wallets::record::{Authority, WalletRecord};
use crate::wallets::vector::{SchemeId, PROGRAM_ID};
use wallet_vault::SecretBytes;

fn held(scheme: SchemeId) -> WalletRecord {
    WalletRecord::vector(
        "Here",
        scheme,
        PROGRAM_ID,
        vec![5; scheme.key_len()],
        Authority::Held {
            secret: SecretBytes(vec![1; 32]),
            next_leaf: 7,
        },
    )
}

fn outside(label: &str, scheme: SchemeId) -> WalletRecord {
    WalletRecord::vector(
        label,
        scheme,
        PROGRAM_ID,
        vec![6; scheme.key_len()],
        Authority::Outside,
    )
}

fn held_secret(wallet: &WalletRecord) -> (Vec<u8>, u32) {
    match wallet.kind.vector_settings().unwrap().authority {
        Authority::Held { secret, next_leaf } => (secret.0.clone(), *next_leaf),
        _ => unreachable!(),
    }
}

#[test]
fn the_next_key_is_installed_only_once_the_account_holds_it() {
    let mut wallet = held(SchemeId::Xmss);
    assert_eq!(
        wallet.install_vector_next_secret(&[0; 41]),
        Err(RotationError::NothingPending("Here".into()))
    );
    wallet.prepare_vector_next_key().unwrap();
    let next = wallet.vector_prepared_next_key().unwrap().to_vec();
    assert_eq!(
        wallet.prepare_vector_next_key(),
        Err(RotationError::AlreadyPrepared("Here".into()))
    );

    // The chain still holds the current key: the seed that controls the
    // account stays.
    let before = held_secret(&wallet);
    assert_eq!(
        wallet.install_vector_next_secret(&[5; 41]),
        Err(RotationError::NotLanded("Here".into()))
    );
    assert_eq!(held_secret(&wallet), before);

    wallet.install_vector_next_secret(&next).unwrap();
    let (secret, next_leaf) = held_secret(&wallet);
    assert_ne!(secret, before.0);
    assert_eq!(next_leaf, 0);
    // XMSS keeps signing with the new key until it runs low.
    assert!(wallet.vector_next_pubkey().is_none());
}

#[test]
fn a_winternitz_key_makes_the_key_after_it_when_its_next_is_installed() {
    let mut wallet = held(SchemeId::Winternitz);
    wallet.prepare_vector_next_key().unwrap();
    let next = wallet.vector_prepared_next_key().unwrap().to_vec();
    wallet.install_vector_next_secret(&next).unwrap();
    let after = wallet.vector_prepared_next_key().unwrap();
    assert_ne!(after, next.as_slice());
}

#[test]
fn a_scanned_next_key_goes_only_to_a_wallet_whose_key_is_elsewhere() {
    let mut here = held(SchemeId::Xmss);
    here.prepare_vector_next_key().unwrap();
    let prepared = here.vector_prepared_next_key().unwrap().to_vec();
    assert_eq!(
        here.accept_vector_next_pubkey(vec![9; 41]),
        Err(RotationError::NotHeldHere("Here".into()))
    );
    // What this device made is untouched.
    assert_eq!(here.vector_prepared_next_key(), Some(prepared.as_slice()));

    let there = outside("There", SchemeId::Xmss);
    let wallets = vec![here.clone(), there.clone()];
    let byte = SchemeId::Xmss.byte();
    assert_eq!(
        next_key_target(&wallets, None, byte, &[9; 41]),
        Ok(there.id)
    );
    // Only the wallet kept here has that scheme.
    assert_eq!(
        next_key_target(&[here.clone()], Some(here.id), byte, &[9; 41]),
        Err(RotationError::NoWallet("XMSS"))
    );
}

#[test]
fn a_scanned_next_key_names_its_wallet_or_is_refused() {
    let (a, b) = (
        outside("A", SchemeId::Winternitz),
        outside("B", SchemeId::Winternitz),
    );
    let wallets = vec![a.clone(), b.clone()];
    let byte = SchemeId::Winternitz.byte();
    assert_eq!(
        next_key_target(&wallets, None, byte, &[9; 41]),
        Err(RotationError::Choose("Winternitz"))
    );
    assert_eq!(
        next_key_target(&wallets, Some(WalletId::new()), byte, &[9; 41]),
        Err(RotationError::Choose("Winternitz"))
    );
    assert_eq!(
        next_key_target(&wallets, Some(b.id), byte, &[9; 41]),
        Ok(b.id)
    );
    assert_eq!(
        next_key_target(&wallets, Some(b.id), byte, &[9; 40]),
        Err(RotationError::KeyLength {
            scheme: "Winternitz",
            expected: 41,
            found: 40
        })
    );
    assert_eq!(
        next_key_target(&wallets, None, SchemeId::Ed25519.byte(), &[9; 32]),
        Err(RotationError::KeepsItsKey("Ed25519"))
    );
    assert_eq!(
        next_key_target(&wallets, None, 99, &[9; 41]),
        Err(RotationError::UnknownScheme)
    );
}
