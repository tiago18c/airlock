use super::*;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::transport::protocol::{decode_object, encode_object, encode_wallet_keys, QrObject};
use crate::wallets::vector::new_held_key;
use wallet_vault::SecretBytes;

/// The device that holds the keys: a hot wallet, and a Vector wallet of
/// each offered scheme with its key kept in the record.
fn airgap() -> (VaultData, Pubkey) {
    let mut data = VaultData::default();
    let key = KeypairSigner::new_random();
    let hot = WalletRecord::from_keypair_seed("Cold", key.seed_bytes());
    data.wallets.push(hot);
    for scheme in [
        SchemeId::MlDsa44,
        SchemeId::Falcon512,
        SchemeId::Winternitz,
        SchemeId::Xmss,
    ] {
        let (public, secret) = new_held_key(scheme);
        data.wallets.push(WalletRecord::vector(
            scheme.label(),
            scheme,
            PROGRAM_ID,
            public,
            Authority::Held {
                secret: SecretBytes(secret),
                next_leaf: 0,
            },
        ));
    }
    (data, key.pubkey())
}

fn to_add(keys: &[WalletKey]) -> Vec<KeyToAdd> {
    keys.iter()
        .map(|key| KeyToAdd {
            key: key.clone(),
            label: key.label.clone(),
        })
        .collect()
}

#[test]
fn every_key_and_vector_wallet_is_exported_and_lands_at_the_same_address() {
    let (airgap, cold) = airgap();
    let summaries = airgap.summaries();
    let exported = exportable(&summaries);
    assert_eq!(exported.len(), 5);
    assert_eq!(exported[0].key.role, KeyRole::Address);
    assert_eq!(exported[0].key.key, cold.to_bytes().to_vec());

    // Across a QR transfer, as one object or several.
    let keys: Vec<WalletKey> = exported.iter().map(|one| one.key.clone()).collect();
    let mut carried = Vec::new();
    for object in encode_wallet_keys(&keys).unwrap() {
        let payload = encode_object(&object).unwrap();
        let QrObject::WalletKeys(part) = decode_object(&payload).unwrap() else {
            panic!("wallet keys");
        };
        carried.extend(part);
    }
    assert_eq!(carried, keys);

    let mut online = VaultData::default();
    let seen = arriving(&online.summaries(), &carried);
    assert!(seen.iter().all(|key| key.arrival == Arrival::New));
    let added = online.add_wallet_keys(&to_add(&carried), None).unwrap();
    assert_eq!(added.len(), 5);
    for (here, there) in online.summaries().iter().zip(&summaries) {
        assert_eq!(here.address, there.address, "{}", there.label);
        assert_eq!(here.label, there.label);
    }
    assert_eq!(online.wallets[0].kind.class(), WalletClass::Offline);
    for wallet in &online.wallets[1..] {
        let settings = wallet.kind.vector_settings().unwrap();
        assert_eq!(*settings.authority, Authority::Outside);
    }

    // Read again, nothing is new.
    let again = arriving(&online.summaries(), &carried);
    assert!(again
        .iter()
        .all(|key| matches!(key.arrival, Arrival::AlreadyHere { .. })));
    assert!(online.add_wallet_keys(&to_add(&carried), None).is_err());
}

#[test]
fn one_wallet_exports_on_its_own() {
    let (airgap, _) = airgap();
    let exported = exportable(&airgap.summaries());
    let one = &exported[1];
    let objects = encode_wallet_keys(std::slice::from_ref(&one.key)).unwrap();
    assert_eq!(objects.len(), 1);
    let mut online = VaultData::default();
    let QrObject::WalletKeys(keys) = decode_object(&encode_object(&objects[0]).unwrap()).unwrap()
    else {
        panic!("wallet keys");
    };
    let added = online.add_wallet_keys(&to_add(&keys), None).unwrap();
    assert_eq!(added.len(), 1);
    assert!(!added[0].offline);
    assert_eq!(Some(added[0].address), airgap.wallets[1].address());
}

#[test]
fn an_ed25519_vector_account_signs_as_the_offline_wallet_added_with_it() {
    let key = KeypairSigner::new_random();
    let keys = vec![
        WalletKey {
            role: KeyRole::Vector {
                scheme: SchemeId::Ed25519.byte(),
            },
            label: "Vector".into(),
            key: key.pubkey().to_bytes().to_vec(),
        },
        WalletKey {
            role: KeyRole::Address,
            label: "Cold".into(),
            key: key.pubkey().to_bytes().to_vec(),
        },
    ];
    let mut online = VaultData::default();
    let added = online.add_wallet_keys(&to_add(&keys), None).unwrap();
    // The address goes in first, whatever the order it came in.
    assert!(added[0].offline);
    let vector = online
        .wallets
        .iter()
        .find_map(|wallet| wallet.kind.vector_settings())
        .unwrap();
    assert_eq!(
        *vector.authority,
        Authority::Wallet {
            wallet: added[0].id
        }
    );
}

#[test]
fn a_key_of_the_wrong_length_is_refused_and_one_listed_twice_is_new_once() {
    let short = WalletKey {
        role: KeyRole::Vector {
            scheme: SchemeId::MlDsa44.byte(),
        },
        label: "Short".into(),
        key: vec![1; 100],
    };
    let cold = WalletKey {
        role: KeyRole::Address,
        label: "Cold".into(),
        key: vec![7; 32],
    };
    let seen = arriving(&[], &[short, cold.clone(), cold]);
    assert!(matches!(seen[0].arrival, Arrival::Refused(_)));
    assert_eq!(seen[0].address, None);
    assert_eq!(seen[1].arrival, Arrival::New);
    assert_eq!(
        seen[2].arrival,
        Arrival::AlreadyHere {
            label: "Cold".into()
        }
    );
}

#[test]
fn a_key_needs_a_label() {
    let mut online = VaultData::default();
    let refused = online.add_wallet_keys(
        &[KeyToAdd {
            key: WalletKey {
                role: KeyRole::Address,
                label: String::new(),
                key: vec![7; 32],
            },
            label: "  ".into(),
        }],
        None,
    );
    assert_eq!(refused, Err(RosterError::NoLabel));
}
