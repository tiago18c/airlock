//! The browser request queue: one queue in the core, answered by the window,
//! expired by the transport.

use std::sync::mpsc;

use app_core::bridge::{
    BridgeAnswer, BridgeRefused, BridgeReply, BridgeRequest, SiteCall, SiteTransaction, SiteWire,
};
use app_core::testing::{kit, kit_on, settle, MemoryStorage};
use app_core::{BridgeRequestOutcome, Event, NoticeKind};
use serde_json::Value;

fn unsigned() -> SiteTransaction {
    SiteTransaction {
        account: solana_pubkey::Pubkey::new_from_array([1; 32]),
        wire: SiteWire::Unreadable(1),
        chain: None,
        commitment: None,
    }
}

fn request(id: &str, method: &str) -> BridgeRequest {
    let call = match method {
        "connect" => SiteCall::Connect,
        "signIn" => SiteCall::SignIn(Default::default()),
        "signMessage" => SiteCall::SignMessage {
            account: solana_pubkey::Pubkey::new_from_array([1; 32]),
            message: vec![1],
        },
        "signTransaction" => SiteCall::SignTransactions(vec![unsigned()]),
        other => panic!("no such request: {other}"),
    };
    BridgeRequest {
        id: id.into(),
        origin: "https://example.org".into(),
        call,
    }
}

/// A reply that hands its bytes to the returned receiver.
fn reply() -> (BridgeReply, mpsc::Receiver<Vec<u8>>) {
    let (tx, rx) = mpsc::channel();
    (
        Box::new(move |bytes| {
            let _ = tx.send(bytes);
        }),
        rx,
    )
}

fn read(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

fn outcome_of(kit: &app_core::testing::Kit, id: &str) -> Option<BridgeRequestOutcome> {
    kit.core
        .slices()
        .notices
        .get()
        .items()
        .iter()
        .find_map(|notice| match &notice.kind {
            NoticeKind::BridgeRequest {
                id: notice_id,
                outcome,
                ..
            } if notice_id == id => Some(*outcome),
            _ => None,
        })
}

#[tokio::test]
async fn a_service_queues_requests_through_public_core() {
    let kit = kit();
    let public = kit.core.public();
    let mut events = kit.core.events();
    let mut queue = kit.core.slices().bridge.subscribe();

    let (send, _answer) = reply();
    public
        .bridge_push(request("7f3ac0de-11", "connect"), send)
        .unwrap();

    assert!(queue.has_changed().unwrap());
    let queue = queue.borrow_and_update().clone();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.head().unwrap().code(), "7F3A");
    assert_eq!(
        events.try_recv().unwrap(),
        Event::RequestArrived {
            id: "7f3ac0de-11".into()
        }
    );
}

#[tokio::test]
async fn an_answer_reaches_the_site_and_leaves_the_queue() {
    let kit = kit();
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request("c1", "connect"), send)
        .unwrap();

    assert!(kit.core.bridge_answer(
        "c1",
        &BridgeAnswer::Connect(vec![bridge_protocol::reply::SiteAccount {
            pubkey: [1; 32],
            label: "Treasury".into(),
            signs_messages: true,
        }])
    ));

    let reply = read(&answer.try_recv().unwrap());
    assert_eq!(reply["id"], "c1");
    assert_eq!(reply["result"]["accounts"][0]["label"], "Treasury");
    assert!(kit.core.slices().bridge.get().is_empty());
    assert_eq!(outcome_of(&kit, "c1"), Some(BridgeRequestOutcome::Answered));

    // Answered once: a second answer finds nothing waiting.
    assert!(!kit.core.bridge_answer("c1", &BridgeAnswer::Rejected));
}

#[tokio::test]
async fn a_deferred_save_can_start_on_a_thread_with_no_runtime() {
    use app_core::runtime::{TokioClock, TokioSpawner};
    let testing = kit();
    let core = app_core::Core::open(app_core::ports::Ports {
        spawner: std::sync::Arc::new(TokioSpawner::current()),
        clock: std::sync::Arc::new(TokioClock),
        notifier: testing.notifier.clone(),
        storage: std::sync::Arc::new(MemoryStorage::default()),
        links: testing.links.clone(),
    });
    let public = core.public();
    // How a request that expires reaches the notices, on the bridge's thread.
    std::thread::spawn(move || {
        public.notices_set_bridge_outcome("x", ORIGIN, "connect", BridgeRequestOutcome::Expired);
    })
    .join()
    .unwrap();
}

const ORIGIN: &str = "https://example.org";

fn account(byte: u8) -> wallet_core::ShareableAccount {
    wallet_core::ShareableAccount {
        pubkey: solana_pubkey::Pubkey::new_from_array([byte; 32]),
        label: format!("Wallet {byte}"),
        contract: false,
        summary: String::new(),
    }
}

/// A `connect` the window answers with `accounts`.
fn approve(
    kit: &app_core::testing::Kit,
    id: &str,
    accounts: Vec<wallet_core::ShareableAccount>,
    remember: bool,
) -> Value {
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request(id, "connect"), send)
        .unwrap();
    assert!(kit.core.bridge_connect(id, accounts, remember));
    read(&answer.try_recv().unwrap())
}

fn grant_of(kit: &app_core::testing::Kit) -> Option<wallet_core::SiteGrant> {
    kit.core
        .with_workspace(|workspace| workspace.connections.grant(ORIGIN).cloned())
}

#[tokio::test]
async fn approving_sends_the_accounts_in_the_order_chosen() {
    let kit = kit();
    let reply = approve(&kit, "c", vec![account(2), account(1)], false);
    assert_eq!(reply["result"]["accounts"][0]["label"], "Wallet 2");
    assert_eq!(reply["result"]["accounts"][1]["label"], "Wallet 1");
}

/// A `signIn` the window answers with what `signer` signed.
fn sign_in(
    kit: &app_core::testing::Kit,
    id: &str,
    signer: &wallet_core::ShareableAccount,
    remember: bool,
) -> Value {
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request(id, "signIn"), send)
        .unwrap();
    assert!(kit
        .core
        .bridge_sign_in(id, signer.pubkey, b"signed".to_vec(), [9; 64], remember));
    read(&answer.try_recv().unwrap())
}

#[tokio::test]
async fn signing_in_grants_the_account_and_puts_it_first() {
    let kit = kit();
    approve(&kit, "c", vec![account(1)], true);
    let reply = sign_in(&kit, "s", &account(2), true);
    let accounts = reply["result"]["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(
        reply["result"]["accounts"][0]["publicKey"],
        reply["result"]["account"]
    );
    assert_eq!(
        grant_of(&kit).unwrap().accounts,
        vec![account(2).pubkey, account(1).pubkey]
    );
    assert_eq!(
        kit.core.public().site_sign(ORIGIN, &[account(2).pubkey]),
        app_core::bridge::SiteSign::Allowed
    );
    assert!(reply["result"]["signature"].is_string());
}

#[tokio::test]
async fn signing_in_without_remembering_lasts_this_run() {
    let kit = kit();
    sign_in(&kit, "s", &account(1), false);
    assert!(grant_of(&kit).is_none());
    assert_eq!(
        kit.core.public().site_sign(ORIGIN, &[account(1).pubkey]),
        app_core::bridge::SiteSign::Allowed
    );
}

#[tokio::test]
async fn a_message_answer_fits_only_its_method() {
    let kit = kit();
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request("c", "connect"), send)
        .unwrap();
    let signed = BridgeAnswer::SignedMessage {
        signed: b"hi".to_vec(),
        signature: [1; 64],
    };
    assert!(!kit.core.bridge_answer("c", &signed));
    assert!(kit.core.bridge_answer("c", &BridgeAnswer::Rejected));
    drop(answer);

    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request("m", "signMessage"), send)
        .unwrap();
    assert!(kit.core.bridge_answer("m", &signed));
    let reply = read(&answer.try_recv().unwrap());
    assert_eq!(reply["result"]["signedMessage"], "aGk=");
}

#[tokio::test]
async fn a_remembered_site_is_saved_and_survives_a_restart() {
    let storage = MemoryStorage::default();
    {
        let kit = kit_on(storage.clone());
        approve(&kit, "c", vec![account(1)], true);
        assert!(grant_of(&kit).is_some());
    }
    let kit = kit_on(storage);
    let grant = grant_of(&kit).expect("the grant was saved");
    assert_eq!(grant.accounts, vec![account(1).pubkey]);
}

#[tokio::test]
async fn a_site_not_remembered_lasts_only_this_run() {
    let storage = MemoryStorage::default();
    {
        let kit = kit_on(storage.clone());
        approve(&kit, "c", vec![account(1)], false);
        assert!(grant_of(&kit).is_none());
        assert_eq!(
            kit.core.public().site_sign(ORIGIN, &[account(1).pubkey]),
            app_core::bridge::SiteSign::Allowed
        );
    }
    let kit = kit_on(storage);
    assert_eq!(
        kit.core.public().site_sign(ORIGIN, &[account(1).pubkey]),
        app_core::bridge::SiteSign::NotConnected
    );
}

#[tokio::test]
async fn unticking_remember_forgets_a_site() {
    let kit = kit();
    approve(&kit, "c", vec![account(1)], true);
    approve(&kit, "d", vec![account(1)], false);
    assert!(grant_of(&kit).is_none());
    assert!(!kit.core.site_choice(ORIGIN).remembered);
}

#[tokio::test]
async fn a_failed_save_still_connects_the_site_until_the_app_closes() {
    let kit = kit();
    kit.storage.refuse_saves(true);
    let mut events = kit.core.events();
    approve(&kit, "c", vec![account(1)], true);
    assert!(grant_of(&kit).is_none());
    assert_eq!(
        kit.core.public().site_sign(ORIGIN, &[account(1).pubkey]),
        app_core::bridge::SiteSign::Allowed
    );
    let mut failed = false;
    while let Ok(event) = events.try_recv() {
        failed |= matches!(event, Event::SaveFailed { .. });
    }
    assert!(failed);
}

#[tokio::test]
async fn an_answer_must_fit_the_request() {
    let kit = kit();
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request("s1", "connect"), send)
        .unwrap();

    // A signed transaction does not answer a connect.
    assert!(!kit
        .core
        .bridge_answer("s1", &BridgeAnswer::Signed(vec![vec![0x81]])));
    assert_eq!(kit.core.slices().bridge.get().len(), 1);
    assert!(answer.try_recv().is_err());

    assert!(kit.core.bridge_answer("s1", &BridgeAnswer::Rejected));
    assert_eq!(
        read(&answer.try_recv().unwrap())["error"]["code"],
        "rejected"
    );
    assert_eq!(outcome_of(&kit, "s1"), Some(BridgeRequestOutcome::Rejected));
}

#[tokio::test]
async fn a_batch_goes_back_only_whole() {
    let kit = kit();
    let (send, answer) = reply();
    let mut batch = request("b1", "signTransaction");
    batch.call = SiteCall::SignTransactions(vec![unsigned(); 3]);
    kit.core.public().bridge_push(batch, send).unwrap();
    assert_eq!(
        kit.core
            .slices()
            .bridge
            .get()
            .head()
            .unwrap()
            .transaction_count(),
        3
    );

    // Two of three signed is not an answer, and the request keeps waiting.
    let two = BridgeAnswer::Signed(vec![vec![0x81, 1], vec![0x81, 2]]);
    assert!(!kit.core.bridge_answer("b1", &two));
    assert!(answer.try_recv().is_err());
    assert_eq!(kit.core.slices().bridge.get().len(), 1);

    let all = BridgeAnswer::Signed(vec![vec![0x81, 1], vec![0x81, 2], vec![0x81, 3]]);
    assert!(kit.core.bridge_answer("b1", &all));
    let reply = read(&answer.try_recv().unwrap());
    let signed = reply["result"]["signedTransactions"].as_array().unwrap();
    assert_eq!(signed.len(), 3);
    assert_eq!(signed[2], "gQM=");
    assert_eq!(outcome_of(&kit, "b1"), Some(BridgeRequestOutcome::Answered));
}

fn from_site(id: &str, origin: &str) -> BridgeRequest {
    BridgeRequest {
        origin: origin.into(),
        ..request(id, "connect")
    }
}

#[tokio::test]
async fn the_queue_limits_a_site_and_every_site_together() {
    use bridge_protocol::{MAX_PENDING, MAX_PENDING_PER_ORIGIN};
    let kit = kit();
    let public = kit.core.public();
    let mut replies = Vec::new();
    let mut push = |id: String, origin: &str| {
        let (send, answer) = reply();
        replies.push(answer);
        public.bridge_push(from_site(&id, origin), send)
    };

    for n in 0..MAX_PENDING_PER_ORIGIN {
        push(format!("a{n}"), "https://a.test").unwrap();
    }
    assert_eq!(
        push("a-over".into(), "https://a.test"),
        Err(BridgeRefused::Full)
    );
    // Sites that have room take the queue up to its own limit.
    let mut queued = MAX_PENDING_PER_ORIGIN;
    let mut site = 0;
    while queued < MAX_PENDING {
        for n in 0..MAX_PENDING_PER_ORIGIN.min(MAX_PENDING - queued) {
            push(format!("s{site}-{n}"), &format!("https://s{site}.test")).unwrap();
            queued += 1;
        }
        site += 1;
    }
    assert_eq!(
        push("late".into(), "https://new.test"),
        Err(BridgeRefused::Full)
    );
    assert_eq!(kit.core.slices().bridge.get().len(), MAX_PENDING);

    // Room again once one is answered, for its site or any other.
    assert!(kit.core.bridge_answer("a0", &BridgeAnswer::Rejected));
    push("a-next".into(), "https://a.test").unwrap();
}

#[tokio::test]
async fn an_id_already_waiting_is_refused_and_keeps_its_reply() {
    let kit = kit();
    let public = kit.core.public();
    let (send, first) = reply();
    public
        .bridge_push(request("same", "connect"), send)
        .unwrap();
    let (send, _second) = reply();
    assert_eq!(
        public.bridge_push(from_site("same", "https://other.test"), send),
        Err(BridgeRefused::SameId)
    );
    assert_eq!(kit.core.slices().bridge.get().len(), 1);

    assert!(kit.core.bridge_answer("same", &BridgeAnswer::Rejected));
    assert_eq!(
        read(&first.try_recv().unwrap())["error"]["code"],
        "rejected"
    );
}

#[tokio::test]
async fn the_published_queue_shares_each_request() {
    let kit = kit();
    let public = kit.core.public();
    let (send, _first) = reply();
    public.bridge_push(request("a", "connect"), send).unwrap();
    let before = kit.core.slices().bridge.get();
    let (send, _second) = reply();
    public.bridge_push(request("b", "connect"), send).unwrap();
    let after = kit.core.slices().bridge.get();
    assert!(std::sync::Arc::ptr_eq(
        &before.pending()[0],
        &after.pending()[0]
    ));
}

#[tokio::test]
async fn the_transport_expires_a_request_from_its_own_thread() {
    let kit = kit();
    let public = kit.core.public();
    let (send, answer) = reply();
    public
        .bridge_push(request("late", "signTransaction"), send)
        .unwrap();

    // The bridge serves its socket on a plain thread, outside the runtime.
    // Expiring records a notice, which schedules a save on the spawner.
    let expired = std::thread::spawn(move || public.bridge_expire("late"))
        .join()
        .unwrap();
    assert!(expired);
    settle().await;

    assert!(kit.core.slices().bridge.get().is_empty());
    // The reply was dropped unsent; the transport answers the site itself.
    assert!(matches!(
        answer.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    assert_eq!(
        outcome_of(&kit, "late"),
        Some(BridgeRequestOutcome::Expired)
    );
}
