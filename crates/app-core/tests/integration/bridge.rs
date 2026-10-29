//! The browser request queue: one queue in the core, answered by the window,
//! expired by the transport.

use std::sync::mpsc;

use app_core::bridge::{BridgeAnswer, BridgeReply, BridgeRequest};
use app_core::testing::{kit, settle};
use app_core::{BridgeRequestOutcome, Event, NoticeKind};
use serde_json::Value;

fn request(id: &str, method: &str) -> BridgeRequest {
    BridgeRequest {
        id: id.into(),
        origin: "https://example.org".into(),
        method: method.into(),
        params: Value::Null,
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
    public.bridge_push(request("7f3ac0de-11", "connect"), send);

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
        .bridge_push(request("c1", "connect"), send);

    assert!(kit.core.bridge_answer(
        "c1",
        &BridgeAnswer::Connect(vec![([1; 32], "Treasury".into())])
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
async fn an_answer_must_fit_the_request() {
    let kit = kit();
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request("s1", "connect"), send);

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
    batch.params = serde_json::json!({
        "transactions": [{ "transaction": "gQ==" }, { "transaction": "gQ==" }, { "transaction": "gQ==" }]
    });
    kit.core.public().bridge_push(batch, send);
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

#[tokio::test]
async fn the_transport_expires_a_request_from_its_own_thread() {
    let kit = kit();
    let public = kit.core.public();
    let (send, answer) = reply();
    public.bridge_push(request("late", "signTransaction"), send);

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
