use super::*;
use app_core::bridge::BridgeAnswer;
use app_core::testing::{kit, Kit};
use std::time::Duration;

const SITE: &str = "https://dapp.example";

fn attached() -> (Kit, Bridge) {
    let kit = kit();
    let bridge = Bridge::for_apps();
    bridge.attach(kit.core.public());
    (kit, bridge)
}

fn dapp() -> AppIdentity {
    AppIdentity {
        name: Some("Dapp".into()),
        uri: Some(format!("{SITE}/app?x=1")),
        package: Some("example.dapp".into()),
    }
}

fn account() -> Pubkey {
    Pubkey::new_from_array([7; 32])
}

fn shareable(pubkey: Pubkey) -> wallet_core::ShareableAccount {
    wallet_core::ShareableAccount {
        pubkey,
        label: "Main".into(),
        contract: false,
        summary: String::new(),
        on_device: None,
    }
}

/// The head of the queue, once something waits there.
fn waiting_id(kit: &Kit) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(head) = kit.core.slices().bridge.get().pending().first() {
            return head.id.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "nothing reached the queue"
        );
        std::thread::yield_now();
    }
}

/// Authorize `account()` for the dapp the way the connect modal does.
fn authorized(kit: &Kit, bridge: &Bridge, remember: bool) -> AppAnswer {
    let session = bridge.app_session(dapp());
    let asking = std::thread::spawn(move || session.answer(AppCall::Authorize { sign_in: None }));
    let id = waiting_id(kit);
    assert!(kit
        .core
        .bridge_connect(&id, vec![shareable(account())], remember));
    asking.join().unwrap()
}

fn legacy_transfer(payer: &Pubkey) -> Vec<u8> {
    let mut message = solana_message::Message::new(
        &[solana_system_interface::instruction::transfer(
            payer,
            &Pubkey::new_from_array([2; 32]),
            1,
        )],
        Some(payer),
    );
    message.recent_blockhash = solana_hash::Hash::new_from_array([1; 32]);
    wincode::serialize(&solana_transaction::versioned::VersionedTransaction {
        signatures: vec![solana_signature::Signature::default()],
        message: solana_message::VersionedMessage::Legacy(message),
    })
    .unwrap()
}

#[test]
fn an_app_is_filed_under_the_site_it_gives_or_else_its_package() {
    assert_eq!(dapp().origin().as_deref(), Some(SITE));
    let upper = AppIdentity {
        uri: Some("HTTPS://Dapp.Example:8443".into()),
        ..AppIdentity::default()
    };
    assert_eq!(upper.origin().as_deref(), Some("https://dapp.example:8443"));
    let native = AppIdentity {
        uri: Some("dapp://home".into()),
        package: Some("example.dapp".into()),
        ..AppIdentity::default()
    };
    assert_eq!(
        native.origin().as_deref(),
        Some("android-app://example.dapp")
    );
    // Something odd in the authority, and no package to fall back on.
    let odd = AppIdentity {
        uri: Some("https://user@dapp.example".into()),
        ..AppIdentity::default()
    };
    assert_eq!(odd.origin(), None);
    let junk = AppIdentity {
        package: Some("a/b".into()),
        ..AppIdentity::default()
    };
    assert_eq!(junk.origin(), None);
}

#[test]
fn requests_read_as_mwa_bridge_hands_them_over() {
    let call: AppCall = serde_json::from_str(
        r#"{"method":"signAndSend","accounts":["11111111111111111111111111111111"],"chain":"solana:devnet","payloads":["AQ=="],"commitment":"confirmed","serial":true}"#,
    )
    .unwrap();
    assert_eq!(
        call,
        AppCall::SignAndSend {
            accounts: vec!["11111111111111111111111111111111".into()],
            chain: "solana:devnet".into(),
            payloads: vec!["AQ==".into()],
            commitment: Some("confirmed".into()),
            serial: true,
        }
    );
    let call: AppCall =
        serde_json::from_str(r#"{"method":"authorize","signIn":{"domain":"dapp.example"}}"#)
            .unwrap();
    assert!(matches!(call, AppCall::Authorize { sign_in: Some(_) }));
    let identity: AppIdentity =
        serde_json::from_str(r#"{"name":"Dapp","uri":"https://dapp.example"}"#).unwrap();
    assert_eq!(identity.package, None);
}

#[test]
fn answers_read_as_mwa_bridge_takes_them() {
    let answer = AppAnswer::Authorized {
        accounts: vec![("a2V5".into(), "Main".into())],
        sign_in: None,
    };
    let value: Value = serde_json::from_str(&answer.to_json()).unwrap();
    assert_eq!(value["kind"], "authorized");
    assert_eq!(value["accounts"][0]["publicKey"], "a2V5");
    assert!(value.get("signIn").is_none());
    let value: Value = serde_json::from_str(
        &AppAnswer::NotSubmitted(vec!["c2ln".into(), String::new()]).to_json(),
    )
    .unwrap();
    assert_eq!(value["signatures"][1], "");
}

#[test]
fn a_send_answers_every_signature_or_says_what_went_out() {
    assert_eq!(
        sent(&json!([{ "signature": "YQ==" }, { "signature": "Yg==" }])),
        AppAnswer::Sent(vec!["YQ==".into(), "Yg==".into()])
    );
    assert_eq!(
        sent(&json!([{ "signature": "YQ==" }, { "error": { "code": "send_failed" } }])),
        AppAnswer::NotSubmitted(vec!["YQ==".into(), String::new()])
    );
    assert_eq!(sent(&json!([])), AppAnswer::Declined);
}

#[test]
fn a_transaction_is_asked_of_the_granted_account_that_signs_it() {
    let other = Pubkey::new_from_array([9; 32]);
    let wire = legacy_transfer(&account());
    let granted = [other, account()];
    assert_eq!(signer_of(&wire, &granted), Some(account()));
    assert_eq!(signer_of(&wire, &[other]), None);
    let params = transactions_params(
        &[other.to_string(), account().to_string()],
        "solana:devnet",
        &[b64(&wire)],
        Some("confirmed"),
    )
    .unwrap();
    assert_eq!(params["transactions"][0]["account"], account().to_string());
    assert_eq!(
        params["transactions"][0]["options"]["commitment"],
        "confirmed"
    );
    assert!(transactions_params(&[], "solana:devnet", &["not base64!".into()], None).is_none());
}

#[tokio::test]
async fn a_new_authorization_is_always_asked_and_shows_who_the_app_says_it_is() {
    let (kit, bridge) = attached();
    let first = authorized(&kit, &bridge, true);
    let AppAnswer::Authorized { accounts, sign_in } = first else {
        panic!("approved: {first:?}");
    };
    assert_eq!(accounts.len(), 1);
    assert_eq!(sign_in, None);

    // Remembered, and still asked: another app can give the same site.
    let session = bridge.app_session(dapp());
    let asking = std::thread::spawn(move || session.answer(AppCall::Authorize { sign_in: None }));
    let id = waiting_id(&kit);
    assert_eq!(
        bridge.app_claim(&id),
        Some(AppClaim {
            name: Some("Dapp".into()),
            package: Some("example.dapp".into()),
        })
    );
    assert!(kit.core.bridge_answer(&id, &BridgeAnswer::Rejected));
    assert_eq!(asking.join().unwrap(), AppAnswer::Declined);
    assert_eq!(bridge.app_claim(&id), None);
}

#[tokio::test]
async fn a_token_goes_on_only_while_its_accounts_are_granted() {
    let (kit, bridge) = attached();
    let session = bridge.app_session(dapp());
    let accounts = vec![account().to_string()];
    assert_eq!(
        session.answer(AppCall::Reauthorize {
            accounts: accounts.clone()
        }),
        AppAnswer::Declined
    );
    assert_eq!(
        session.answer(AppCall::Reauthorize { accounts: vec![] }),
        AppAnswer::Declined
    );
    authorized(&kit, &bridge, true);
    assert_eq!(
        session.answer(AppCall::Reauthorize {
            accounts: accounts.clone()
        }),
        AppAnswer::Reauthorized
    );
    assert_eq!(
        session.answer(AppCall::Reauthorize { accounts: vec![] }),
        AppAnswer::Reauthorized
    );
    let other = vec![Pubkey::new_from_array([9; 32]).to_string()];
    assert_eq!(
        session.answer(AppCall::Reauthorize { accounts: other }),
        AppAnswer::Declined
    );
    kit.core.site_revoke(SITE);
    assert_eq!(
        session.answer(AppCall::Reauthorize { accounts }),
        AppAnswer::Declined
    );
}

#[tokio::test]
async fn a_signature_goes_through_the_queue_and_comes_back_signed() {
    let (kit, bridge) = attached();
    let wire = legacy_transfer(&account());
    let call = AppCall::SignTransactions {
        accounts: vec![account().to_string()],
        chain: "solana:devnet".into(),
        payloads: vec![b64(&wire)],
    };
    // Not connected: turned away without asking.
    assert_eq!(
        bridge.app_session(dapp()).answer(call.clone()),
        AppAnswer::Declined
    );
    assert!(kit.core.slices().bridge.get().is_empty());

    authorized(&kit, &bridge, false);
    let session = bridge.app_session(dapp());
    let asking = std::thread::spawn(move || session.answer(call));
    let id = waiting_id(&kit);
    let mut signed = wire.clone();
    signed[1] = 0xAB;
    assert!(kit
        .core
        .bridge_answer(&id, &BridgeAnswer::Signed(vec![signed.clone()])));
    assert_eq!(
        asking.join().unwrap(),
        AppAnswer::Signed(vec![b64(&signed)])
    );
}

#[tokio::test]
async fn a_closed_session_takes_its_requests_and_its_site_with_it() {
    let (kit, bridge) = attached();
    let session = std::sync::Arc::new(bridge.app_session(dapp()));
    let open = |kit: &Kit| {
        kit.core
            .slices()
            .bridge
            .get()
            .pending()
            .iter()
            .any(|request| request.origin == SITE)
    };
    let asking = {
        let session = session.clone();
        std::thread::spawn(move || session.answer(AppCall::Authorize { sign_in: None }))
    };
    waiting_id(&kit);
    assert!(open(&kit));
    assert!(kit.core.slices().sites.get().connected.is_empty());
    session.close();
    assert_eq!(asking.join().unwrap(), AppAnswer::Declined);
    assert!(!open(&kit));
    assert!(bridge
        .shared
        .apps
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty());
}

#[tokio::test]
async fn an_app_with_no_site_and_no_package_is_turned_away() {
    let (_kit, bridge) = attached();
    let session = bridge.app_session(AppIdentity::default());
    assert_eq!(session.origin(), None);
    assert_eq!(
        session.answer(AppCall::Authorize { sign_in: None }),
        AppAnswer::Declined
    );
}
