//! The intents prototype's methods from an app on the phone (`docs/intents.md`,
//! "Airlock's prototype on Android"): read as `MwaBridge` hands them over,
//! refused before an authorization, and a `resolve` that answers once the
//! person decided.

use super::*;
use crate::bridge::tests::{paid_by, with_hot_wallets};
use app_core::bridge::BridgeAnswer;
use app_core::testing::{kit, Kit};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;
use wallet_core::WalletSigner;

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
        uri: Some(SITE.into()),
        package: None,
    }
}

/// Wait up to five seconds for `done`.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The head of the queue, once something waits there.
fn waiting_id(kit: &Kit) -> String {
    eventually("something reaches the queue", || {
        !kit.core.slices().bridge.get().is_empty()
    });
    kit.core.slices().bridge.get().pending()[0].id.clone()
}

/// A session of the dapp, its authorization granted for `wallet`.
fn authorize(kit: &Kit, session: &Arc<AppSession>, wallet: Pubkey) {
    let asking = {
        let session = session.clone();
        std::thread::spawn(move || session.answer(AppCall::Authorize { sign_in: None }))
    };
    let id = waiting_id(kit);
    assert!(kit.core.bridge_connect(
        &id,
        vec![wallet_core::ShareableAccount {
            pubkey: wallet,
            label: "Hot".into(),
            contract: false,
            summary: String::new(),
            on_device: None,
        }],
        false,
    ));
    assert!(matches!(
        asking.join().unwrap(),
        AppAnswer::Authorized { .. }
    ));
}

/// A hot wallet of this device, and a session of the dapp authorized for it.
fn authorized(kit: &Kit, bridge: &Bridge) -> (Pubkey, Arc<AppSession>) {
    let wallet = with_hot_wallets(kit, &[7])[0];
    let session = Arc::new(bridge.app_session(dapp()));
    authorize(kit, &session, wallet);
    (wallet, session)
}

fn intent(name: &str, params: Value) -> AppCall {
    AppCall::Intent {
        name: name.into(),
        params,
    }
}

/// `airlock_resolve` acting as `wallet`, with a transfer it pays for.
fn resolve_params(wallet: Pubkey, id: &str) -> Value {
    json!({
        "id": id,
        "account": wallet.to_string(),
        "chain": "solana:devnet",
        "transactions": [b64(&paid_by(&wallet, &wallet))],
    })
}

fn result(answer: AppAnswer) -> Value {
    match answer {
        AppAnswer::Intent(Ok(result)) => result,
        other => panic!("a result: {other:?}"),
    }
}

fn error(answer: AppAnswer) -> IntentError {
    match answer {
        AppAnswer::Intent(Err(error)) => error,
        other => panic!("an error: {other:?}"),
    }
}

/// Ask `call` of `session` on a thread of its own, as `wallet_adapter` does.
fn asking(session: &Arc<AppSession>, call: AppCall) -> JoinHandle<AppAnswer> {
    let session = session.clone();
    std::thread::spawn(move || session.answer(call))
}

fn state(session: &AppSession, id: &str) -> Value {
    result(session.answer(intent("airlock_get_resolution", json!({ "id": id }))))["resolution"]
        ["state"]
        .clone()
}

/// The transaction of `wallet`'s key signed: what the window would send.
fn signed_by_hot_key(wire: &[u8]) -> Vec<u8> {
    let key = wallet_core::KeypairSigner::from_seed(&[7; 32]).unwrap();
    wallet_core::apply_local_signatures(wire, &[&key as &dyn WalletSigner]).unwrap()
}

#[test]
fn intent_calls_read_as_mwa_bridge_hands_them_over() {
    let call: AppCall = serde_json::from_str(
        r#"{"method":"intent","name":"airlock_resolve","params":{"id":"a","account":"x"}}"#,
    )
    .unwrap();
    assert_eq!(
        call,
        intent("airlock_resolve", json!({ "id": "a", "account": "x" }))
    );
    let bare: AppCall =
        serde_json::from_str(r#"{"method":"intent","name":"airlock_get_resolution"}"#).unwrap();
    assert_eq!(bare, intent("airlock_get_resolution", Value::Null));

    let names = [
        ("airlock_get_signing_profile", "getSigningProfile"),
        ("airlock_resolve", "resolve"),
        ("airlock_get_resolution", "getResolution"),
        ("airlock_provide_signatures", "provideSignatures"),
        ("airlock_cancel_resolution", "cancelResolution"),
        ("airlock_sign_message", "resolveMessage"),
    ];
    for (mwa, bridge) in names {
        assert_eq!(intent_method(mwa), Some(bridge));
    }
    assert_eq!(intent_method("airlock_resolveMessage"), None);
    assert_eq!(intent_method("sign_messages"), None);
}

#[test]
fn intent_answers_read_as_mwa_bridge_takes_them() {
    let ok: Value = serde_json::from_str(
        &AppAnswer::Intent(Ok(json!({ "resolution": { "id": "a" } }))).to_json(),
    )
    .unwrap();
    assert_eq!(
        ok,
        json!({ "kind": "result", "result": { "resolution": { "id": "a" } } })
    );
    let refused = AppAnswer::Intent(Err(IntentError {
        code: "unsupported".into(),
        message: "No.".into(),
        requirement: Some("order".into()),
    }));
    let value: Value = serde_json::from_str(&refused.to_json()).unwrap();
    assert_eq!(
        value,
        json!({ "kind": "error", "code": "unsupported", "message": "No.", "requirement": "order" })
    );
    let value: Value = serde_json::from_str(
        &AppAnswer::Intent(Err(IntentError::new("rejected", "No."))).to_json(),
    )
    .unwrap();
    assert!(value.get("requirement").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_intent_method_waits_for_the_sessions_authorization() {
    let (kit, bridge) = attached();
    let wallet = with_hot_wallets(&kit, &[7])[0];
    let session = Arc::new(bridge.app_session(dapp()));
    let profile = || {
        session.answer(intent(
            "airlock_get_signing_profile",
            json!({ "account": wallet.to_string() }),
        ))
    };

    let refused = error(profile());
    assert_eq!(refused.code, "rejected");
    assert_eq!(refused.message, NOT_AUTHORIZED);
    let refused = error(session.answer(intent("airlock_resolve", resolve_params(wallet, "early"))));
    assert_eq!(refused.code, "rejected");
    assert!(kit.core.slices().bridge.get().is_empty());

    // Another session of the same site authorized: still not this one.
    let other = Arc::new(bridge.app_session(dapp()));
    authorize(&kit, &other, wallet);
    assert_eq!(error(profile()).code, "rejected");

    authorize(&kit, &session, wallet);
    let answered = result(profile());
    assert_eq!(answered["profile"]["account"], wallet.to_string());
    assert_eq!(answered["profile"]["timing"], "sync");
    let unknown = error(session.answer(intent("airlock_resolve_later", json!({}))));
    assert_eq!(unknown.code, "not_implemented");

    // A reauthorization that is not granted ends it, as MWA's own does.
    kit.core.site_revoke(SITE);
    assert_eq!(
        session.answer(AppCall::Reauthorize { accounts: vec![] }),
        AppAnswer::Declined
    );
    assert_eq!(error(profile()).code, "rejected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deauthorized_session_takes_no_intent_method() {
    let (kit, bridge) = attached();
    let (wallet, session) = authorized(&kit, &bridge);
    let params = json!({ "account": wallet.to_string() });
    assert!(matches!(
        session.answer(intent("airlock_get_signing_profile", params.clone())),
        AppAnswer::Intent(Ok(_))
    ));
    assert_eq!(session.answer(AppCall::Deauthorize), AppAnswer::Done);
    assert_eq!(
        error(session.answer(intent("airlock_get_signing_profile", params))).code,
        "rejected"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resolve_answers_once_the_person_decides() {
    let (kit, bridge) = attached();
    let (wallet, session) = authorized(&kit, &bridge);
    let answer = asking(
        &session,
        intent("airlock_resolve", resolve_params(wallet, "pay-1")),
    );
    let id = waiting_id(&kit);
    // The modal says who the app says it is, and nobody has decided.
    assert_eq!(
        bridge.app_claim(&id),
        Some(AppClaim {
            name: Some("Dapp".into()),
            package: None,
        })
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(!answer.is_finished(), "it waits for the person");
    // What never waits for the person goes on meanwhile.
    assert_eq!(state(&session, "pay-1"), "awaiting-approval");

    let wire = paid_by(&wallet, &wallet);
    let sending = kit
        .core
        .bridge_begin_send(&id, std::slice::from_ref(&signed_by_hot_key(&wire)))
        .unwrap();
    // The screen sends it, and a phone stops the screen behind the
    // browser: the answer waits until it is out.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!answer.is_finished(), "it waits while sending");
    assert_eq!(state(&session, "pay-1"), "sending");
    kit.core.bridge_send_finished(sending, vec![Ok([3; 64])]);
    let resolution = result(answer.join().unwrap())["resolution"].clone();
    assert_eq!(resolution["id"], "pay-1");
    assert_eq!(resolution["state"], "done");
    assert_eq!(
        resolution["ran"][0]["signature"],
        solana_signature::Signature::from([3; 64]).to_string()
    );
    eventually("the claim goes with the request", || {
        bridge.app_claim(&id).is_none()
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approved_resolve_goes_on_after_its_session_closes() {
    let (kit, bridge) = attached();
    let (wallet, session) = authorized(&kit, &bridge);
    let answer = asking(
        &session,
        intent("airlock_resolve", resolve_params(wallet, "on")),
    );
    let id = waiting_id(&kit);
    let wire = paid_by(&wallet, &wallet);
    let sending = kit
        .core
        .bridge_begin_send(&id, std::slice::from_ref(&signed_by_hot_key(&wire)))
        .unwrap();
    session.close();
    assert_eq!(error(answer.join().unwrap()).code, "rejected");
    kit.core.bridge_send_finished(sending, vec![Ok([3; 64])]);
    let done = kit.core.public().resolution(SITE, "on").unwrap();
    assert_eq!(done.state, State::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declined_expired_and_cancelled_are_errors() {
    let (kit, bridge) = attached();
    let (wallet, session) = authorized(&kit, &bridge);

    let answer = asking(
        &session,
        intent("airlock_resolve", resolve_params(wallet, "no")),
    );
    let id = waiting_id(&kit);
    assert!(kit.core.bridge_answer(&id, &BridgeAnswer::Rejected));
    let declined = error(answer.join().unwrap());
    assert_eq!(declined.code, "rejected");
    assert_eq!(declined.message, "The request was declined in Airlock.");

    let answer = asking(
        &session,
        intent("airlock_resolve", resolve_params(wallet, "stop")),
    );
    waiting_id(&kit);
    let cancelled =
        result(session.answer(intent("airlock_cancel_resolution", json!({ "id": "stop" }))));
    assert_eq!(cancelled["resolution"]["state"], "cancelled");
    assert_eq!(error(answer.join().unwrap()).code, "cancelled");

    let answer = asking(
        &session,
        intent("airlock_resolve", resolve_params(wallet, "late")),
    );
    waiting_id(&kit);
    // Its window starts once the core has queued it.
    std::thread::sleep(Duration::from_millis(100));
    kit.clock.advance(Duration::from_secs(10 * 60));
    let expired = error(answer.join().unwrap());
    assert_eq!(expired.code, "expired");
    assert!(kit.core.slices().bridge.get().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resolve_outlives_its_session_until_the_person_decides() {
    let (kit, bridge) = attached();
    let (wallet, session) = authorized(&kit, &bridge);
    let answer = asking(
        &session,
        intent("airlock_resolve", resolve_params(wallet, "kept")),
    );
    let id = waiting_id(&kit);

    session.close();
    // Nobody hears the answer any more, and it stops waiting.
    assert_eq!(error(answer.join().unwrap()).code, "rejected");
    // The person can still decide, and the modal still names the app.
    assert_eq!(kit.core.slices().bridge.get().len(), 1);
    assert!(bridge.app_claim(&id).is_some());
    assert_eq!(
        kit.core.public().resolution(SITE, "kept").unwrap().state,
        State::AwaitingApproval
    );

    assert!(kit.core.bridge_answer(&id, &BridgeAnswer::Rejected));
    eventually("the claim goes with the request", || {
        bridge.app_claim(&id).is_none()
    });
    assert_eq!(
        kit.core.public().resolution(SITE, "kept").unwrap().state,
        State::Declined
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_keeps_the_bridges_code_message_and_requirement() {
    let (kit, bridge) = attached();
    let (wallet, session) = authorized(&kit, &bridge);

    let mut params = resolve_params(wallet, "elsewhere");
    params["chain"] = json!("solana:localnet");
    let refused = error(session.answer(intent("airlock_resolve", params)));
    assert_eq!(refused.code, "unsupported");
    assert_eq!(refused.requirement.as_deref(), Some("chain"));
    assert!(refused.message.contains("solana:localnet"), "{refused:?}");

    let mut params = resolve_params(wallet, "noted");
    let stranger = Pubkey::new_from_array([42; 32]);
    params["accounts"] = json!({ stranger.to_string(): { "kind": "new", "replaceable": true } });
    let refused = error(session.answer(intent("airlock_resolve", params)));
    assert_eq!(refused.code, "bad_notes");
    assert_eq!(refused.requirement, None);

    let missing =
        error(session.answer(intent("airlock_get_resolution", json!({ "id": "nothing" }))));
    assert_eq!(missing.code, "not_found");
    assert!(kit.core.slices().bridge.get().is_empty());
    assert!(bridge
        .shared
        .claims
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_signed_as_a_memo_answers_once_the_person_decides() {
    let (kit, bridge) = attached();
    let (wallet, session) = authorized(&kit, &bridge);
    let params = json!({
        "id": "memo-1",
        "account": wallet.to_string(),
        "message": b64(b"dapp.example wants proof, nonce 7"),
        "mode": "memo",
        "chain": "solana:devnet",
    });
    let answer = asking(&session, intent("airlock_sign_message", params));
    let id = waiting_id(&kit);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!answer.is_finished(), "it waits for the person");

    let queue = kit.core.slices().bridge.get();
    let memo = queue.pending()[0].resolve().unwrap().transactions[0]
        .wire
        .bytes()
        .unwrap()
        .to_vec();
    let sending = kit
        .core
        .bridge_begin_send(&id, std::slice::from_ref(&signed_by_hot_key(&memo)))
        .unwrap();
    kit.core.bridge_send_finished(sending, vec![Ok([4; 64])]);
    let answered = result(answer.join().unwrap());
    assert_eq!(answered["kind"], "memo");
    assert_eq!(answered["resolution"]["id"], "memo-1");
    assert_eq!(answered["resolution"]["state"], "done");
    assert_eq!(
        answered["resolution"]["message"]["memo"],
        "dapp.example wants proof, nonce 7"
    );
}
