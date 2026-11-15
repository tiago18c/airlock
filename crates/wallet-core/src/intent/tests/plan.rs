//! What the wallet decides to do with a request's notes.

use super::*;
use std::collections::BTreeMap;

fn hot(account: Pubkey) -> SigningProfile {
    SigningProfile {
        account,
        timing: Timing::Sync,
        execution: Execution::TopLevel,
        carrier: None,
        fee_payer: vec![Payer::Account, Payer::Wallet, Payer::App],
        new_accounts: vec![NewAccountKey::WalletKey, NewAccountKey::AppKey],
        delivery: vec![Delivery::Send, Delivery::Cosign, Delivery::Return],
        messages: vec![MessageMode::Signature, MessageMode::Memo],
        max_transactions: 16,
    }
}

fn squads(account: Pubkey) -> SigningProfile {
    SigningProfile {
        timing: Timing::Async,
        execution: Execution::Carrier,
        carrier: Some(crate::ids::SQUADS_V4_PROGRAM_ID),
        fee_payer: vec![Payer::Wallet],
        new_accounts: vec![NewAccountKey::Carrier],
        delivery: vec![Delivery::Send],
        messages: vec![MessageMode::Memo],
        ..hot(account)
    }
}

fn programmatic(account: Pubkey) -> SigningProfile {
    SigningProfile {
        new_accounts: vec![NewAccountKey::WalletKey],
        max_transactions: 1,
        ..squads(account)
    }
}

fn plan(
    notes: &Notes,
    profile: &SigningProfile,
    delivery: Delivery,
    payer: Option<Pubkey>,
) -> Result<Plan, Unsupported> {
    plan_resolution(notes, profile, delivery, Order::Serial, 1, payer)
}

#[test]
fn sent_by_the_wallet_the_mint_gets_a_key_here_and_the_wallet_pays() {
    let site = Site::new();
    let plan = plan(
        &site.notes(),
        &hot(site.owner),
        Delivery::Send,
        Some(site.owner),
    )
    .unwrap();
    assert_eq!(plan.new_keys, vec![site.mint]);
    assert_eq!(plan.fee_payer, Some((site.payer, site.owner)));
    assert!(!plan.needs_app());
    assert_eq!(
        plan.replaced(&[pk(40)], &[]),
        BTreeMap::from([(site.mint, pk(40)), (site.payer, site.owner)])
    );
}

#[test]
fn for_a_multisig_the_mint_is_carried_whether_it_waits_or_not() {
    let site = Site::new();
    for timing in [Timing::Sync, Timing::Async] {
        let profile = SigningProfile {
            timing,
            ..squads(site.owner)
        };
        let plan = plan(&site.notes(), &profile, Delivery::Send, Some(pk(30))).unwrap();
        assert!(plan.new_keys.is_empty());
        assert_eq!(plan.carried, vec![site.mint]);
        assert!(!plan.needs_app());
        assert_eq!(
            plan.replaced(&[], &[pk(41)]),
            BTreeMap::from([(site.mint, pk(41)), (site.payer, pk(30))])
        );
    }
    // A programmatic signer makes a key here instead.
    let plan = plan(
        &site.notes(),
        &programmatic(site.owner),
        Delivery::Send,
        Some(pk(30)),
    )
    .unwrap();
    assert_eq!((plan.new_keys, plan.carried), (vec![site.mint], vec![]));
}

#[test]
fn a_carried_account_signs_exactly_one_transaction() {
    let site = Site::new();
    let wire = site.wire(Format::Legacy);
    let other = wire_of(Format::V1, &site.instructions()[..1], &site.payer, &[]);
    let pay = solana_system_interface::instruction::transfer(&pk(1), &pk(2), 1);
    let unrelated = wire_of(Format::Legacy, &[pay], &pk(1), &[]);
    assert_eq!(
        carried_transactions(&[unrelated.clone(), wire.clone()], &[site.mint]),
        Ok(vec![1])
    );
    let twice = carried_transactions(&[wire.clone(), other], &[site.mint]).unwrap_err();
    assert_eq!(twice.requirement, Requirement::NewAccounts);
    assert!(twice.message.contains("more than one"), "{}", twice.message);
    let none = carried_transactions(&[unrelated], &[site.mint]).unwrap_err();
    assert_eq!(none.requirement, Requirement::NewAccounts);
}

#[test]
fn an_account_that_pays_for_itself_is_left_to_the_preparation() {
    // No note on the account acted as: the plan replaces no payer, and a
    // wallet without a key has another pay when the request is prepared.
    let site = Site::new();
    let mut notes = site.notes();
    notes
        .accounts
        .0
        .retain(|(address, _)| *address != site.payer);
    for payer in [Some(site.owner), Some(pk(30)), None] {
        let plan = plan(&notes, &squads(site.owner), Delivery::Send, payer).unwrap();
        assert_eq!(plan.fee_payer, None);
        assert_eq!(plan.carried, vec![site.mint]);
        assert!(!plan.needs_app());
    }
}

#[test]
fn cosigned_the_apps_sponsor_stays_and_signs() {
    let site = Site::new();
    for delivery in [Delivery::Cosign, Delivery::Return] {
        let plan = plan(&site.notes(), &hot(site.owner), delivery, Some(site.owner)).unwrap();
        assert_eq!(plan.new_keys, vec![site.mint]);
        assert_eq!(plan.fee_payer, None);
        assert_eq!(plan.app_signs, vec![site.payer]);
    }
}

#[test]
fn a_payer_that_is_already_the_routes_is_left_alone() {
    let site = Site::new();
    let plan = plan(
        &site.notes(),
        &hot(site.owner),
        Delivery::Send,
        Some(site.payer),
    )
    .unwrap();
    assert_eq!(plan.fee_payer, None);
}

#[test]
fn nothing_to_pay_with_is_unsupported() {
    let site = Site::new();
    let err = plan(&site.notes(), &squads(site.owner), Delivery::Send, None).unwrap_err();
    assert_eq!(err.requirement, Requirement::FeePayer);
}

#[test]
fn a_kept_key_takes_the_apps_signature_and_a_delivery_that_allows_it() {
    let site = Site::new();
    let mut notes = site.notes();
    notes.accounts.0[0].1 = AccountNote::New { replaceable: false };

    let err = plan(&notes, &hot(site.owner), Delivery::Send, Some(site.owner)).unwrap_err();
    assert_eq!(err.requirement, Requirement::NewAccounts);
    assert_eq!(err.requirement.name(), "newAccounts");
    assert_eq!(err.code(), "unsupported");

    let plan_ok = plan(&notes, &hot(site.owner), Delivery::Cosign, Some(site.owner)).unwrap();
    assert!(plan_ok.new_keys.is_empty());
    assert_eq!(plan_ok.app_signs, vec![site.mint, site.payer]);

    // A multisig keeps no key of the app's.
    let err = plan(&notes, &squads(site.owner), Delivery::Send, Some(pk(30))).unwrap_err();
    assert_eq!(err.requirement, Requirement::NewAccounts);
}

#[test]
fn an_apps_signer_needs_cosign_or_return() {
    let notes = Notes {
        accounts: AccountNotes(vec![(
            pk(9),
            AccountNote::Signer {
                signs: SignsBy::App,
            },
        )]),
        data: Vec::new(),
    };
    let err = plan(&notes, &hot(pk(2)), Delivery::Send, Some(pk(2))).unwrap_err();
    assert_eq!(err.requirement, Requirement::Signer);
    let ok = plan(&notes, &hot(pk(2)), Delivery::Return, Some(pk(2))).unwrap();
    assert_eq!(ok.app_signs, vec![pk(9)]);
}

#[test]
fn a_sponsor_kept_by_the_app_needs_a_profile_that_lets_the_app_pay() {
    let notes = Notes {
        accounts: AccountNotes(vec![(
            pk(1),
            AccountNote::FeePayer {
                replaceable: false,
                signs: Some(SignsBy::App),
            },
        )]),
        data: Vec::new(),
    };
    let err = plan(&notes, &squads(pk(2)), Delivery::Send, Some(pk(30))).unwrap_err();
    assert_eq!(err.requirement, Requirement::FeePayer);
    let err = plan(&notes, &hot(pk(2)), Delivery::Send, Some(pk(2))).unwrap_err();
    assert_eq!(err.requirement, Requirement::FeePayer);
    let ok = plan(&notes, &hot(pk(2)), Delivery::Cosign, Some(pk(2))).unwrap();
    assert_eq!(ok.app_signs, vec![pk(1)]);
}

#[test]
fn a_delivery_the_profile_does_not_list_is_unsupported() {
    let site = Site::new();
    let err = plan(
        &site.notes(),
        &squads(site.owner),
        Delivery::Cosign,
        Some(pk(30)),
    )
    .unwrap_err();
    assert_eq!(err.requirement, Requirement::Delivery);
}

#[test]
fn several_transactions_as_one_are_unsupported() {
    let site = Site::new();
    let err = plan_resolution(
        &site.notes(),
        &hot(site.owner),
        Delivery::Send,
        Order::Atomic,
        2,
        Some(site.owner),
    )
    .unwrap_err();
    assert_eq!(err.requirement, Requirement::Order);
    assert!(plan_resolution(
        &site.notes(),
        &hot(site.owner),
        Delivery::Send,
        Order::Atomic,
        1,
        Some(site.owner),
    )
    .is_ok());
}

#[test]
fn an_account_that_lists_no_way_to_sign_for_a_new_account_refuses_it() {
    let site = Site::new();
    let none = SigningProfile {
        new_accounts: Vec::new(),
        ..squads(site.owner)
    };
    let err = plan(&site.notes(), &none, Delivery::Send, Some(site.owner)).unwrap_err();
    assert_eq!(err.requirement, Requirement::NewAccounts);
}

#[test]
fn more_transactions_than_the_profile_takes_are_unsupported() {
    let site = Site::new();
    let one = SigningProfile {
        max_transactions: 1,
        ..squads(site.owner)
    };
    let err = plan_resolution(
        &Notes::default(),
        &one,
        Delivery::Send,
        Order::Serial,
        2,
        Some(site.owner),
    )
    .unwrap_err();
    assert_eq!(err.requirement, Requirement::Order);
    assert!(plan_resolution(
        &Notes::default(),
        &one,
        Delivery::Send,
        Order::Serial,
        1,
        Some(site.owner)
    )
    .is_ok());
}
