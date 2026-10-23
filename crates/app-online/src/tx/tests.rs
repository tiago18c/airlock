//! Tracker and submitter behaviour with a fake node and the manual clock.

use std::sync::Arc;
use std::time::Duration;

use app_core::testing::{kit, settle};
use app_core::{NoticeKind, TxStatus};
use solana_signature::Signature;
use wallet_core::{Cluster, ConfirmationLevel, SignatureStatus, SolanaRpc};
use wallet_flows::testing::{pk, transfer, FakeRpc};
use wallet_flows::track::{POLL_INTERVAL, TRACK_TIMEOUT};

use super::{Submitter, Tracker};

fn sig(n: u8) -> Signature {
    Signature::from([n; 64])
}

fn status_of(kit: &app_core::testing::Kit, signature: Signature) -> Option<TxStatus> {
    kit.core
        .slices()
        .notices
        .get()
        .items()
        .iter()
        .find_map(|n| {
            let NoticeKind::Transaction {
                signature: s,
                status,
                ..
            } = &n.kind
            else {
                return None;
            };
            (*s == signature).then(|| status.clone())
        })
}

async fn advance_polls(clock: &app_core::testing::ManualClock, n: u32) {
    for _ in 0..n {
        clock.advance(POLL_INTERVAL);
        settle().await;
    }
}

#[tokio::test]
async fn tracks_to_confirmed() {
    let kit = kit();
    let signature = sig(1);
    let rpc = Arc::new(FakeRpc::default());
    let tracker = Tracker::new(kit.core.public());
    kit.core.notices_push(NoticeKind::Transaction {
        flow: 1,
        signature,
        cluster: Cluster::Devnet,
        label: "Transfer".into(),
        status: TxStatus::Sent,
    });
    let deadline = tracker.fresh_deadline();
    tracker.follow(rpc.clone(), &Cluster::Devnet, signature, None, deadline);

    settle().await;
    rpc.set_signature_status(
        signature,
        Some(SignatureStatus {
            slot: 42,
            err: None,
            confirmation: Some(ConfirmationLevel::Confirmed),
        }),
    );
    advance_polls(&kit.clock, 2).await;

    assert_eq!(
        status_of(&kit, signature),
        Some(TxStatus::Confirmed { slot: 42 })
    );
}

#[tokio::test]
async fn tracks_to_failed() {
    let kit = kit();
    let signature = sig(2);
    let rpc = Arc::new(FakeRpc::default());
    let tracker = Tracker::new(kit.core.public());
    kit.core.notices_push(NoticeKind::Transaction {
        flow: 2,
        signature,
        cluster: Cluster::Devnet,
        label: "Transfer".into(),
        status: TxStatus::Sent,
    });
    let deadline = tracker.fresh_deadline();
    tracker.follow(rpc.clone(), &Cluster::Devnet, signature, None, deadline);
    settle().await;
    let error = "InstructionError(0, Custom(1))".to_string();
    rpc.set_signature_status(
        signature,
        Some(SignatureStatus {
            slot: 7,
            err: Some(error.clone()),
            confirmation: Some(ConfirmationLevel::Confirmed),
        }),
    );
    advance_polls(&kit.clock, 2).await;

    assert_eq!(status_of(&kit, signature), Some(TxStatus::Failed { error }));
}

#[tokio::test]
async fn a_landed_transaction_is_told_with_its_cluster_and_slot() {
    let kit = kit();
    let rpc = Arc::new(FakeRpc::default());
    let tracker = Tracker::new(kit.core.public());
    let (told, landed) = std::sync::mpsc::channel();
    let told = std::sync::Mutex::new(told);
    tracker.set_on_landed(move |cluster, slot| {
        let _ = told.lock().unwrap().send((cluster.clone(), slot));
    });
    let deadline = tracker.fresh_deadline();
    tracker.follow(rpc.clone(), &Cluster::Devnet, sig(3), None, deadline);
    tracker.follow(rpc.clone(), &Cluster::Devnet, sig(4), None, deadline);
    settle().await;
    rpc.set_signature_status(
        sig(3),
        Some(SignatureStatus {
            slot: 42,
            err: None,
            confirmation: Some(ConfirmationLevel::Confirmed),
        }),
    );
    rpc.set_signature_status(
        sig(4),
        Some(SignatureStatus {
            slot: 43,
            err: Some("InstructionError(0, Custom(1))".into()),
            confirmation: Some(ConfirmationLevel::Confirmed),
        }),
    );
    advance_polls(&kit.clock, 2).await;

    let mut landed: Vec<_> = landed.try_iter().collect();
    landed.sort_by_key(|(_, slot)| *slot);
    assert_eq!(
        landed,
        vec![(Cluster::Devnet, None), (Cluster::Devnet, Some(42))],
        "a failed transaction paid its fee too"
    );
}

#[tokio::test]
async fn expires_past_last_valid_height() {
    let kit = kit();
    let signature = sig(3);
    let rpc = Arc::new(FakeRpc::default().with_block_height(10));
    let tracker = Tracker::new(kit.core.public());
    kit.core.notices_push(NoticeKind::Transaction {
        flow: 3,
        signature,
        cluster: Cluster::Devnet,
        label: "Transfer".into(),
        status: TxStatus::Sent,
    });
    let deadline = tracker.fresh_deadline();
    tracker.follow(rpc.clone(), &Cluster::Devnet, signature, Some(50), deadline);
    settle().await;
    rpc.set_block_height(51);
    advance_polls(&kit.clock, 2).await;

    assert_eq!(status_of(&kit, signature), Some(TxStatus::Expired));
}

#[tokio::test]
async fn unknown_after_five_minutes() {
    let kit = kit();
    let signature = sig(4);
    let rpc = Arc::new(FakeRpc::default());
    let tracker = Tracker::new(kit.core.public());
    kit.core.notices_push(NoticeKind::Transaction {
        flow: 4,
        signature,
        cluster: Cluster::Devnet,
        label: "Transfer".into(),
        status: TxStatus::Sent,
    });
    let deadline = tracker.fresh_deadline();
    tracker.follow(rpc, &Cluster::Devnet, signature, None, deadline);
    settle().await;
    kit.clock.advance(TRACK_TIMEOUT + Duration::from_secs(1));
    settle().await;
    advance_polls(&kit.clock, 1).await;

    assert_eq!(status_of(&kit, signature), Some(TxStatus::Unknown));
}

#[tokio::test]
async fn tracks_on_the_cluster_it_was_sent_to() {
    let kit = kit();
    let signature = sig(5);
    let sent_on = Arc::new(FakeRpc::default());
    let other = Arc::new(FakeRpc::default());
    let tracker = Tracker::new(kit.core.public());
    kit.core.notices_push(NoticeKind::Transaction {
        flow: 5,
        signature,
        cluster: Cluster::Devnet,
        label: "Transfer".into(),
        status: TxStatus::Sent,
    });
    let deadline = tracker.fresh_deadline();
    tracker.follow(sent_on.clone(), &Cluster::Devnet, signature, None, deadline);
    settle().await;

    // App switches cluster; the other node would be what a fresh rpc() returns.
    let _switched: Arc<dyn SolanaRpc> = other.clone();
    sent_on.set_signature_status(
        signature,
        Some(SignatureStatus {
            slot: 99,
            err: None,
            confirmation: Some(ConfirmationLevel::Finalized),
        }),
    );
    other.set_signature_status(
        signature,
        Some(SignatureStatus {
            slot: 1,
            err: Some("wrong cluster".into()),
            confirmation: Some(ConfirmationLevel::Confirmed),
        }),
    );
    advance_polls(&kit.clock, 2).await;

    assert_eq!(
        status_of(&kit, signature),
        Some(TxStatus::Confirmed { slot: 99 })
    );
    assert_eq!(other.asked(), 0, "tracking must not ask the new cluster");
    assert!(sent_on.asked() > 0);
}

#[tokio::test]
async fn resumes_sent_notices_at_start() {
    let kit = kit();
    let signature = sig(6);
    let rpc = Arc::new(FakeRpc::default());
    kit.core.notices_push(NoticeKind::Transaction {
        flow: 6,
        signature,
        cluster: Cluster::Devnet,
        label: "Transfer".into(),
        status: TxStatus::Sent,
    });

    let tracker = Tracker::new(kit.core.public());
    let rpc_resume = rpc.clone();
    tracker.resume(move |_cluster| Some(rpc_resume.clone()));
    settle().await;

    rpc.set_signature_status(
        signature,
        Some(SignatureStatus {
            slot: 12,
            err: None,
            confirmation: Some(ConfirmationLevel::Confirmed),
        }),
    );
    advance_polls(&kit.clock, 2).await;

    assert_eq!(
        status_of(&kit, signature),
        Some(TxStatus::Confirmed { slot: 12 })
    );
}

#[tokio::test]
async fn submitter_is_the_only_send_path() {
    let kit = kit();
    let rpc = Arc::new(FakeRpc::default());
    // Make send return a distinct signature by using the transfer wire's first sig slot (zeros).
    let wire = transfer(pk(1), pk(2)).wire;
    let tracker = Tracker::new(kit.core.public());
    let submitter = Submitter::new(kit.core.public(), tracker);
    let signature = submitter
        .send(rpc, &wire, 7, "Sign", &Cluster::Devnet, None)
        .await
        .unwrap();
    settle().await;

    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), 1);
    match &notices.items()[0].kind {
        NoticeKind::Transaction {
            status: TxStatus::Sent,
            signature: s,
            label,
            ..
        } => {
            assert_eq!(*s, signature);
            assert_eq!(label, "Sign");
        }
        other => panic!("expected Sent notice, got {other:?}"),
    }
}

#[tokio::test]
async fn signatures_on_one_node_share_each_poll() {
    let kit = kit();
    let rpc = Arc::new(FakeRpc::default());
    let tracker = Tracker::new(kit.core.public());
    for n in [8u8, 9] {
        kit.core.notices_push(NoticeKind::Transaction {
            flow: u64::from(n),
            signature: sig(n),
            cluster: Cluster::Devnet,
            label: "Transfer".into(),
            status: TxStatus::Sent,
        });
        let deadline = tracker.fresh_deadline();
        tracker.follow(rpc.clone(), &Cluster::Devnet, sig(n), None, deadline);
    }
    settle().await;
    let first = rpc.asked();
    advance_polls(&kit.clock, 3).await;
    assert_eq!(
        rpc.asked() - first,
        3,
        "one status request per poll for both"
    );

    for n in [8u8, 9] {
        rpc.set_signature_status(
            sig(n),
            Some(SignatureStatus {
                slot: u64::from(n),
                err: None,
                confirmation: Some(ConfirmationLevel::Confirmed),
            }),
        );
    }
    advance_polls(&kit.clock, 1).await;
    assert_eq!(
        status_of(&kit, sig(8)),
        Some(TxStatus::Confirmed { slot: 8 })
    );
    assert_eq!(
        status_of(&kit, sig(9)),
        Some(TxStatus::Confirmed { slot: 9 })
    );
    let done = rpc.asked();
    advance_polls(&kit.clock, 3).await;
    assert_eq!(rpc.asked(), done, "nothing left, so the loop stopped");
}
