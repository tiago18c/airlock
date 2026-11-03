//! Tracker and submitter behaviour with a fake node and the manual clock.

use std::sync::Arc;
use std::time::Duration;

use app_core::testing::{kit, settle};
use app_core::{NoticeKind, TxStatus};
use app_ui_kit::TxSubmit;
use solana_signature::Signature;
use wallet_core::{
    apply_local_signatures, parse_wire, Cluster, ConfirmationLevel, KeypairSigner, RpcError,
    SignatureStatus, SolanaRpc, WalletSigner,
};
use wallet_flows::send::SendFailure;
use wallet_flows::testing::{legacy_transfer, pk, transfer, FakeRpc};
use wallet_flows::track::{POLL_INTERVAL, SITE_SEND_POLL, SITE_SEND_WINDOW, TRACK_TIMEOUT};

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

fn confirmed() -> Option<SignatureStatus> {
    Some(SignatureStatus {
        slot: 9,
        err: None,
        confirmation: Some(ConfirmationLevel::Confirmed),
    })
}

fn transaction_notices(kit: &app_core::testing::Kit) -> usize {
    kit.core
        .slices()
        .notices
        .get()
        .items()
        .iter()
        .filter(|n| matches!(n.kind, NoticeKind::Transaction { .. }))
        .count()
}

#[tokio::test]
async fn a_send_with_no_answer_is_followed_and_can_still_land() {
    for no_answer in [RpcError::Timeout, RpcError::NoAnswer("reset".into())] {
        let kit = kit();
        let rpc = Arc::new(FakeRpc::default());
        rpc.set_send_fails(Some(no_answer));
        let (wire, signature) = signed_legacy(&KeypairSigner::new_random());
        let tracker = Tracker::new(kit.core.public());
        let submitter = Submitter::new(kit.core.public(), tracker);

        let failure = submitter
            .send(rpc.clone(), &wire, 7, "Sign", &Cluster::Devnet, None)
            .await
            .unwrap_err();
        assert!(matches!(
            &failure,
            SendFailure::Unknown { signature: found, why }
                if *found == signature && why.contains("may still land")
        ));
        settle().await;
        assert_eq!(status_of(&kit, signature), Some(TxStatus::Sent));

        rpc.set_signature_status(signature, confirmed());
        advance_polls(&kit.clock, 2).await;
        assert_eq!(
            status_of(&kit, signature),
            Some(TxStatus::Confirmed { slot: 9 })
        );
    }
}

#[tokio::test]
async fn a_send_that_never_left_or_was_refused_is_final() {
    for refused in [
        RpcError::Unreachable("connection refused".into()),
        RpcError::Message("Transaction simulation failed".into()),
    ] {
        let kit = kit();
        let rpc = Arc::new(FakeRpc::default());
        rpc.set_send_fails(Some(refused));
        let (wire, signature) = signed_legacy(&KeypairSigner::new_random());
        let tracker = Tracker::new(kit.core.public());
        let submitter = Submitter::new(kit.core.public(), tracker);

        let failure = submitter
            .send(rpc.clone(), &wire, 7, "Sign", &Cluster::Devnet, None)
            .await
            .unwrap_err();
        assert!(matches!(failure, SendFailure::Failed(_)));
        settle().await;
        assert!(matches!(
            status_of(&kit, signature),
            Some(TxStatus::Rejected { .. })
        ));
    }
}

#[tokio::test]
async fn sending_again_after_no_answer_keeps_one_notice_that_can_still_end() {
    let kit = kit();
    let rpc = Arc::new(FakeRpc::default());
    let (wire, signature) = signed_legacy(&KeypairSigner::new_random());
    let tracker = Tracker::new(kit.core.public());
    let submitter = Submitter::new(kit.core.public(), tracker);
    let send = || submitter.send(rpc.clone(), &wire, 7, "Sign", &Cluster::Devnet, None);

    rpc.set_send_fails(Some(RpcError::Timeout));
    assert!(matches!(send().await, Err(SendFailure::Unknown { .. })));
    // Still no answer.
    assert!(matches!(send().await, Err(SendFailure::Unknown { .. })));
    // The first went through after all, and the node says so.
    rpc.set_send_fails(Some(RpcError::Message(
        "This transaction has already been processed".into(),
    )));
    assert!(matches!(send().await, Err(SendFailure::Failed(_))));
    settle().await;
    assert_eq!(transaction_notices(&kit), 1);
    assert_eq!(status_of(&kit, signature), Some(TxStatus::Sent));

    rpc.set_signature_status(signature, confirmed());
    advance_polls(&kit.clock, 2).await;
    assert_eq!(
        status_of(&kit, signature),
        Some(TxStatus::Confirmed { slot: 9 })
    );
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

/// A legacy transfer `payer` pays for and has signed, and its signature.
fn signed_legacy(payer: &KeypairSigner) -> (Vec<u8>, Signature) {
    let signers: [&dyn WalletSigner; 1] = [payer];
    let wire = apply_local_signatures(&legacy_transfer(payer.pubkey(), pk(9)), &signers).unwrap();
    let signature = parse_wire(&wire).unwrap().signature().unwrap();
    (wire, signature)
}

#[tokio::test]
async fn what_a_site_sends_is_tracked_and_what_it_does_not_is_told() {
    let kit = kit();
    let (sent_wire, sent) = signed_legacy(&KeypairSigner::new_random());
    let (kept_wire, kept) = signed_legacy(&KeypairSigner::new_random());
    let rpc = Arc::new(FakeRpc::default().with_signature_status(
        sent,
        SignatureStatus {
            slot: 5,
            err: None,
            confirmation: Some(ConfirmationLevel::Processed),
        },
    ));
    let submitter = Submitter::new(kit.core.public(), Tracker::new(kit.core.public()));
    let answer = TxSubmit::watch_site_send(
        &submitter,
        rpc.clone(),
        vec![(sent_wire, 1), (kept_wire, 2)],
        "https://site.test".into(),
        Cluster::Devnet,
    );
    let polls = SITE_SEND_WINDOW.as_millis() / SITE_SEND_POLL.as_millis();
    for _ in 0..polls {
        settle().await;
        kit.clock.advance(SITE_SEND_POLL);
    }
    assert_eq!(answer.await, vec![(sent, true), (kept, false)]);
    assert_eq!(status_of(&kit, sent), Some(TxStatus::Sent));
    assert_eq!(status_of(&kit, kept), Some(TxStatus::NotSent));

    // The one the site sent is followed like one sent here.
    rpc.set_signature_status(
        sent,
        Some(SignatureStatus {
            slot: 6,
            err: None,
            confirmation: Some(ConfirmationLevel::Confirmed),
        }),
    );
    advance_polls(&kit.clock, 2).await;
    assert_eq!(status_of(&kit, sent), Some(TxStatus::Confirmed { slot: 6 }));
}
