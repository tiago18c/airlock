use super::*;
use crate::fixtures::pk;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::wallets::squads::Squad;

fn hot(label: &str) -> (WalletRecord, Pubkey) {
    let key = KeypairSigner::new_random();
    (
        WalletRecord::from_keypair_seed(label, key.seed_bytes()),
        key.pubkey(),
    )
}

#[test]
fn a_hot_wallet_signs_and_pays_for_itself() {
    let (wallet, pubkey) = hot("Hot");
    let route = signing_route(std::slice::from_ref(&wallet), wallet.id).unwrap();
    assert_eq!(route.address, pubkey);
    assert_eq!(
        route.steps,
        vec![RouteStep::Key {
            wallet: wallet.id,
            pubkey
        }]
    );
    assert_eq!(route.fee_payer, Some((wallet.id, pubkey)));
    assert!(route.is_local());
}

#[test]
fn squads_through_a_programmatic_signer_through_a_hot_key() {
    let (payer, payer_key) = hot("Fees");
    let (key, authority) = hot("Authority");
    let mut signer = WalletRecord::programmatic("Signer", authority, Some(key.id));
    let member = signer.address().unwrap();
    assert_eq!(member, programmatic_signer_address(&authority));
    signer.kind.set_fee_payer(Some(payer.id));

    let squad = Squad::v4(&pk(9));
    let mut treasury = WalletRecord::squads("Treasury", squad, vec![member, pk(8)], 2);
    if let WalletKind::Squads { member_wallet, .. } = &mut treasury.kind {
        *member_wallet = Some(signer.id);
    }
    let wallets = vec![payer.clone(), key.clone(), signer.clone(), treasury.clone()];

    let route = signing_route(&wallets, treasury.id).unwrap();
    assert_eq!(route.address, squad.vault(0));
    assert_eq!(
        route.steps,
        vec![
            RouteStep::Squads {
                wallet: treasury.id,
                squad,
                vault: squad.vault(0),
                member,
            },
            RouteStep::Programmatic {
                wallet: signer.id,
                authority,
                address: member,
                nonce: None,
            },
            RouteStep::Key {
                wallet: key.id,
                pubkey: authority,
            },
        ]
    );
    // The multisig sets no fee payer, so the signer's choice applies.
    assert_eq!(route.fee_payer, Some((payer.id, payer_key)));
    assert_eq!(route.signing_key(), Some(authority));

    // Its own choice wins once set.
    let mut wallets = wallets;
    wallets[3].kind.set_fee_payer(Some(key.id));
    let route = signing_route(&wallets, treasury.id).unwrap();
    assert_eq!(route.fee_payer, Some((key.id, authority)));
}

#[test]
fn an_authority_kept_elsewhere_ends_the_chain_and_cannot_pay() {
    let signer = WalletRecord::programmatic("Cold signer", pk(5), None);
    let route = signing_route(std::slice::from_ref(&signer), signer.id).unwrap();
    assert_eq!(
        route.steps.last(),
        Some(&RouteStep::OutsideKey { pubkey: pk(5) })
    );
    assert_eq!(route.fee_payer, None);
    assert!(!route.is_local());

    let (payer, payer_key) = hot("Fees");
    let mut signer = signer;
    signer.kind.set_fee_payer(Some(payer.id));
    let route = signing_route(&[payer.clone(), signer.clone()], signer.id).unwrap();
    assert_eq!(route.fee_payer, Some((payer.id, payer_key)));
}

#[test]
fn an_offline_wallet_is_signed_for_elsewhere_and_lends_its_fee_payer() {
    let (payer, payer_key) = hot("Fees");
    let mut cold = WalletRecord::offline("Cold", pk(5));
    let route = signing_route(std::slice::from_ref(&cold), cold.id).unwrap();
    assert_eq!(route.address, pk(5));
    assert_eq!(route.steps, vec![RouteStep::OutsideKey { pubkey: pk(5) }]);
    assert_eq!(route.fee_payer, None);
    assert!(!route.is_local());

    cold.kind.set_fee_payer(Some(payer.id));
    let route = signing_route(&[payer.clone(), cold.clone()], cold.id).unwrap();
    assert_eq!(route.fee_payer, Some((payer.id, payer_key)));

    // Its programmatic signer sets no fee payer, so the key's choice applies.
    let signer = WalletRecord::programmatic("Signer", pk(5), Some(cold.id));
    let wallets = [payer.clone(), cold.clone(), signer.clone()];
    let route = signing_route(&wallets, signer.id).unwrap();
    assert_eq!(route.address, programmatic_signer_address(&pk(5)));
    assert_eq!(
        route.steps.last(),
        Some(&RouteStep::OutsideKey { pubkey: pk(5) })
    );
    assert_eq!(route.fee_payer, Some((payer.id, payer_key)));
}

/// An offline wallet is stored under the name the screens give it. The name
/// it had in the first vault format is not read: that format is refused whole.
#[test]
fn an_offline_wallet_is_stored_as_one() {
    let stored = serde_json::to_value(WalletRecord::offline("Cold", pk(1))).unwrap();
    assert_eq!(stored["kind"]["type"], "offline");
    let old = serde_json::json!({
        "id": "00112233445566778899aabbccddeeff",
        "label": "Old",
        "tags": [],
        "kind": { "type": "watchOnly", "pubkey": pk(1) },
    });
    assert!(serde_json::from_value::<WalletRecord>(old).is_err());
    let json = stored.to_string();
    let wallet: WalletRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(
        wallet.kind.class(),
        crate::wallets::record::WalletClass::Offline
    );
    assert_eq!(wallet.kind.fee_payer(), None);
    assert_eq!(wallet.address(), Some(pk(1)));
}

#[test]
fn a_broken_chain_says_what_is_wrong() {
    let (key, authority) = hot("Key");
    let squad = Squad::smart_account(3);
    let mut treasury = WalletRecord::squads("Treasury", squad, vec![pk(8)], 1);
    let all = |treasury: &WalletRecord| vec![key.clone(), treasury.clone()];

    assert_eq!(
        signing_route(&all(&treasury), treasury.id),
        Err(RouteError::NoMember("Treasury".into()))
    );

    if let WalletKind::Squads { member_wallet, .. } = &mut treasury.kind {
        *member_wallet = Some(key.id);
    }
    assert_eq!(
        signing_route(&all(&treasury), treasury.id),
        Err(RouteError::NotAMember {
            squad: "Treasury".into(),
            member: "Key".into()
        })
    );

    if let WalletKind::Squads { members, .. } = &mut treasury.kind {
        members.push(authority);
    }
    assert!(signing_route(&all(&treasury), treasury.id).is_ok());

    // A fee payer has to be a key: an offline one pays, and its signature
    // is brought in from the device that holds it.
    let cold = WalletRecord::offline("Cold", pk(1));
    treasury.kind.set_fee_payer(Some(cold.id));
    let route = signing_route(&[key.clone(), cold.clone(), treasury.clone()], treasury.id);
    assert_eq!(route.unwrap().fee_payer, Some((cold.id, pk(1))));

    // A programmatic signer is no key, so it cannot pay.
    let signer = WalletRecord::programmatic("Signer", pk(2), None);
    treasury.kind.set_fee_payer(Some(signer.id));
    assert_eq!(
        signing_route(
            &[key.clone(), signer.clone(), treasury.clone()],
            treasury.id
        ),
        Err(RouteError::FeePayerNotAKey("Signer".into()))
    );
    assert_eq!(
        signing_route(&[], WalletId::new()),
        Err(RouteError::UnknownWallet)
    );
}

/// Wallets saved before these fields existed still load.
#[test]
fn an_older_squads_record_still_loads() {
    let json = serde_json::json!({
        "id": "00112233445566778899aabbccddeeff",
        "label": "Old",
        "tags": [],
        "kind": { "type": "squads", "multisig": pk(1), "vault": pk(2) },
    })
    .to_string();
    let wallet: WalletRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(wallet.kind.fee_payer(), None);
    assert_eq!(wallet.address(), Some(pk(2)));
}

fn vector(label: &str, scheme: SchemeId, authority: Authority) -> WalletRecord {
    WalletRecord::vector(label, scheme, pk(70), vec![7; scheme.key_len()], authority)
}

#[test]
fn a_vector_account_is_signed_for_by_a_wallet_or_by_the_key_in_its_record() {
    let (payer, payer_key) = hot("Fees");
    let (key, _) = hot("Key");

    // An Ed25519 key that is a hot wallet here.
    let mut through = vector("Through", SchemeId::Ed25519, Authority::of(Some(key.id)));
    through.kind.set_fee_payer(Some(payer.id));
    let account = through.address().unwrap();
    let wallets = vec![payer.clone(), key.clone(), through.clone()];
    let route = signing_route(&wallets, through.id).unwrap();
    assert_eq!(route.address, account);
    assert_eq!(
        route.steps[0],
        RouteStep::Vector {
            wallet: through.id,
            scheme: SchemeId::Ed25519,
            account,
        }
    );
    assert!(matches!(route.steps[1], RouteStep::Key { wallet, .. } if wallet == key.id));
    assert!(route.is_local());
    assert_eq!(route.fee_payer, Some((payer.id, payer_key)));

    // A key of another scheme is kept in the record. It signs here, has no
    // address to name, and cannot pay.
    let held = vector(
        "Held",
        SchemeId::MlDsa44,
        Authority::Held {
            secret: crate::SecretBytes(vec![1; 32]),
            next_leaf: 0,
        },
    );
    let route = signing_route(std::slice::from_ref(&held), held.id).unwrap();
    assert_eq!(
        route.steps.last(),
        Some(&RouteStep::HeldKey { wallet: held.id })
    );
    assert!(route.is_local());
    assert_eq!((route.signing_key(), route.fee_payer), (None, None));

    // A key this app does not manage: the chain ends at the account.
    let outside = vector("Outside", SchemeId::Eip191, Authority::Outside);
    let route = signing_route(std::slice::from_ref(&outside), outside.id).unwrap();
    assert_eq!(route.steps.len(), 1);
    assert!(!route.is_local());
    assert_eq!(route.fee_payer, None);
}

#[test]
fn only_a_vector_account_keeps_its_authority_key_in_its_record() {
    let mut signer = WalletRecord::programmatic("Signer", pk(5), None);
    if let WalletKind::Programmatic { authority, .. } = &mut signer.kind {
        *authority = Authority::Held {
            secret: crate::SecretBytes(vec![1; 32]),
            next_leaf: 0,
        };
    }
    assert_eq!(
        signing_route(std::slice::from_ref(&signer), signer.id),
        Err(RouteError::HeldAuthority("Signer".into()))
    );
}

#[test]
fn an_offline_key_lives_on_its_durable_nonce_and_a_chain_through_it_does_not() {
    let (payer, payer_key) = hot("Fees");
    let mut cold = WalletRecord::offline("Cold", pk(1));
    cold.kind.set_fee_payer(Some(payer.id));
    let wallets = vec![payer.clone(), cold.clone()];
    assert_eq!(
        signing_route(&wallets, cold.id).unwrap().durable_nonce,
        None
    );

    cold.kind.set_durable_nonce(Some(pk(5)));
    let mut signer = WalletRecord::programmatic("Signer", pk(1), Some(cold.id));
    signer.kind.set_fee_payer(Some(payer.id));
    let wallets = vec![payer.clone(), cold.clone(), signer.clone()];
    let route = signing_route(&wallets, cold.id).unwrap();
    assert_eq!(route.durable_nonce, Some(pk(5)));
    assert_eq!(route.steps, vec![RouteStep::OutsideKey { pubkey: pk(1) }]);
    assert_eq!(route.fee_payer, Some((payer.id, payer_key)));

    // The signer has its own executor nonce: the key's durable nonce is not
    // what orders it.
    let through = signing_route(&wallets, signer.id).unwrap();
    assert_eq!(through.durable_nonce, None);
}
