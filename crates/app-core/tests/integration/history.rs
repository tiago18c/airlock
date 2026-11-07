//! History slice: what the tracker finds reaches it, and it is kept on disk.

use app_core::testing::{kit, kit_on};
use app_core::{NoticeKind, Outcome, TxStatus};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use wallet_core::{AddressSignature, Cluster};

fn sig(byte: u8) -> Signature {
    Signature::from([byte; 64])
}

fn sent(kit: &app_core::testing::Kit, signature: Signature) {
    kit.core.notices_push(NoticeKind::Transaction {
        flow: 1,
        signature,
        cluster: Cluster::Devnet,
        label: "Sign".into(),
        status: TxStatus::Sent,
    });
    kit.core.public().history_deferred("sent", |history| {
        history.record_sent(
            signature,
            &Cluster::Devnet,
            &[1, 2, 3],
            vec![Pubkey::new_from_array([1; 32])],
            "Sign",
            10,
        );
        true
    });
}

#[tokio::test]
async fn what_the_tracker_finds_moves_a_sent_transaction_on() {
    let kit = kit();
    sent(&kit, sig(1));
    sent(&kit, sig(2));
    let history = kit.core.slices().history.get();
    assert_eq!(history.get(&sig(1)).unwrap().outcome, Outcome::Pending);

    assert!(kit
        .core
        .notices_set_tx_status(sig(1), TxStatus::Confirmed { slot: 77 }));
    assert!(kit.core.notices_set_tx_status(sig(2), TxStatus::Expired));

    let history = kit.core.slices().history.get();
    let landed = history.get(&sig(1)).unwrap();
    assert_eq!(landed.outcome, Outcome::Succeeded);
    assert_eq!(landed.slot, Some(77));
    // It never landed, so it is not history.
    assert!(history.get(&sig(2)).is_none());
}

#[tokio::test]
async fn history_is_kept_across_a_restart() {
    let kit = kit();
    kit.core.public().history_deferred("listed", |history| {
        history.merge_listed(
            &Cluster::Devnet,
            Pubkey::new_from_array([1; 32]),
            &[AddressSignature {
                signature: sig(3),
                slot: 5,
                block_time: Some(1_700_000_000),
                err: None,
            }],
            10,
        )
    });
    kit.core.flush();
    assert!(kit.storage.read("history.json").is_some());

    let again = kit_on(kit.storage.clone());
    let history = again.core.slices().history.get();
    assert_eq!(history.get(&sig(3)).unwrap().slot, Some(5));
}
