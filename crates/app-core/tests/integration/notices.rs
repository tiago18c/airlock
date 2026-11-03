//! Notices slice: status ordering, cap, dismiss, mark/clear, and persistence.

use app_core::testing::{kit, kit_on, settle, MemoryStorage};
use app_core::{
    notice_row_text, BridgeRequestOutcome, Event, LockReason, NoticeKind, ToastState, TxStatus,
    VaultState, MAX_NOTICES, RETENTION_SECS, SAVE_INTERVAL,
};
use solana_signature::Signature;
use wallet_core::Cluster;

fn sig(n: usize) -> Signature {
    let mut bytes = [0u8; 64];
    bytes[..8].copy_from_slice(&(n as u64).to_le_bytes());
    Signature::from(bytes)
}

fn tx_kind(signature: Signature, status: TxStatus) -> NoticeKind {
    NoticeKind::Transaction {
        flow: 1,
        signature,
        cluster: Cluster::Devnet,
        label: "Transfer".into(),
        status,
    }
}

#[tokio::test]
async fn notice_status_moves_forward() {
    let kit = kit();
    let signature = sig(1);
    let mut events = kit.core.events();

    kit.core.notices_push(tx_kind(signature, TxStatus::Sent));
    assert!(kit
        .core
        .notices_set_tx_status(signature, TxStatus::Confirmed { slot: 42 }));

    let notices = kit.core.slices().notices.get();
    match &notices.items()[0].kind {
        NoticeKind::Transaction { status, .. } => {
            assert_eq!(status, &TxStatus::Confirmed { slot: 42 });
        }
        other => panic!("expected Transaction, got {other:?}"),
    }
    assert_eq!(
        events.try_recv().unwrap(),
        Event::TxStatusChanged {
            signature,
            status: TxStatus::Confirmed { slot: 42 },
        }
    );

    assert!(!kit.core.notices_set_tx_status(signature, TxStatus::Sent));
    let notices = kit.core.slices().notices.get();
    match &notices.items()[0].kind {
        NoticeKind::Transaction { status, .. } => {
            assert_eq!(status, &TxStatus::Confirmed { slot: 42 });
        }
        other => panic!("expected Transaction, got {other:?}"),
    }
    assert!(events.try_recv().is_err());
}

fn browser_request(id: usize) -> NoticeKind {
    NoticeKind::BridgeRequest {
        id: format!("req-{id}"),
        origin: "https://flood.test".into(),
        method: "connect".into(),
        outcome: BridgeRequestOutcome::Expired,
    }
}

#[tokio::test]
async fn no_amount_of_notices_drops_a_transaction_being_watched() {
    let kit = kit();
    let watched = sig(1);
    kit.core.notices_push(tx_kind(watched, TxStatus::Sent));
    for i in 2..=(MAX_NOTICES + 10) {
        kit.core
            .notices_push(tx_kind(sig(i), TxStatus::Confirmed { slot: 1 }));
    }
    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), MAX_NOTICES);
    let kept = |signature| {
        notices.items().iter().any(
            |n| matches!(&n.kind, NoticeKind::Transaction { signature: s, .. } if *s == signature),
        )
    };
    // It is the oldest, and what went instead are the oldest that had ended.
    assert!(kept(watched));
    assert!(!kept(sig(2)));

    // Its status still lands.
    assert!(kit
        .core
        .notices_set_tx_status(watched, TxStatus::Confirmed { slot: 9 }));
}

#[tokio::test]
async fn a_flood_of_browser_requests_does_not_push_out_transactions() {
    let kit = kit();
    for i in 0..10 {
        kit.core
            .notices_push(tx_kind(sig(i), TxStatus::Confirmed { slot: 1 }));
    }
    for id in 0..(MAX_NOTICES * 2) {
        kit.core.notices_push(browser_request(id));
    }
    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), MAX_NOTICES);
    let transactions = notices
        .items()
        .iter()
        .filter(|n| matches!(n.kind, NoticeKind::Transaction { .. }))
        .count();
    assert_eq!(transactions, 10);
    // The newest request is the one on top.
    assert!(matches!(
        &notices.items()[0].kind,
        NoticeKind::BridgeRequest { id, .. } if *id == format!("req-{}", MAX_NOTICES * 2 - 1)
    ));
}

#[tokio::test]
async fn a_new_browser_request_is_kept_when_only_transactions_are_older() {
    let kit = kit();
    for i in 0..MAX_NOTICES {
        kit.core
            .notices_push(tx_kind(sig(i), TxStatus::Confirmed { slot: 1 }));
    }
    kit.core.notices_push(browser_request(1));
    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), MAX_NOTICES);
    assert!(matches!(
        notices.items()[0].kind,
        NoticeKind::BridgeRequest { .. }
    ));
}

#[tokio::test]
async fn notices_capped() {
    let kit = kit();
    for i in 0..=MAX_NOTICES {
        kit.core
            .notices_push(tx_kind(sig(i), TxStatus::Confirmed { slot: 1 }));
    }
    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), MAX_NOTICES);
    // Newest first: the 501st push (i = 500) is at the front; i = 0 was dropped.
    match &notices.items()[0].kind {
        NoticeKind::Transaction { signature, .. } => {
            assert_eq!(*signature, sig(MAX_NOTICES));
        }
        other => panic!("expected Transaction, got {other:?}"),
    }
    match &notices.items()[MAX_NOTICES - 1].kind {
        NoticeKind::Transaction { signature, .. } => {
            assert_eq!(*signature, sig(1));
        }
        other => panic!("expected Transaction, got {other:?}"),
    }

    // Age limit at load: a notice older than retention is dropped.
    // ManualClock's epoch is 1_700_000_000_000 ms (see testing.rs).
    let storage = MemoryStorage::default();
    let now = 1_700_000_000_i64;
    let created = now - RETENTION_SECS - 1;
    let signature = sig(9);
    let json = format!(
        r#"{{
  "items": [
    {{
      "id": 0,
      "created": {created},
      "updated": {created},
      "kind": {{
        "Transaction": {{
          "flow": 1,
          "signature": "{signature}",
          "cluster": {{ "kind": "devnet" }},
          "label": "Transfer",
          "status": "Sent"
        }}
      }},
      "toast": "Up",
      "read": false
    }}
  ],
  "next_id": 1
}}"#
    );
    storage.put("notices.json", json.as_bytes());

    let kit = kit_on(storage);
    assert!(kit.core.slices().notices.get().is_empty());
}

#[tokio::test]
async fn dismissed_toast_stays_in_list() {
    let kit = kit();
    let signature = sig(2);
    let id = kit.core.notices_push(tx_kind(signature, TxStatus::Sent));
    let before = kit.core.slices().notices.get().items()[0].clone();

    kit.core.notices_dismiss_toast(id);

    let after = kit.core.slices().notices.get();
    let after = &after.items()[0];
    assert_eq!(after.id, before.id);
    assert_eq!(after.created, before.created);
    assert_eq!(after.updated, before.updated);
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.read, before.read);
    assert_eq!(after.toast, ToastState::Dismissed);
    assert_ne!(before.toast, ToastState::Dismissed);
    assert_eq!(kit.core.slices().notices.get().len(), 1);
}

#[tokio::test]
async fn bridge_request_and_vault_locked_kinds() {
    let kit = kit();
    kit.core.create_vault("pw").unwrap();
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unlocked);

    kit.core.notices_push(NoticeKind::BridgeRequest {
        id: "abc".into(),
        origin: "https://example.org".into(),
        method: "connect".into(),
        outcome: BridgeRequestOutcome::Waiting,
    });
    kit.core.lock();
    settle().await;

    let items = kit.core.slices().notices.get();
    // Newest first: VaultLocked from lock(), then BridgeRequest.
    assert!(matches!(
        &items.items()[0].kind,
        NoticeKind::VaultLocked {
            reason: LockReason::User
        }
    ));
    assert!(matches!(
        &items.items()[1].kind,
        NoticeKind::BridgeRequest {
            outcome: BridgeRequestOutcome::Waiting,
            ..
        }
    ));
}

#[tokio::test]
async fn mark_read_and_unread_count() {
    let kit = kit();
    let a = kit.core.notices_push(tx_kind(sig(1), TxStatus::Sent));
    let _b = kit.core.notices_push(tx_kind(sig(2), TxStatus::Sent));
    assert_eq!(kit.core.slices().notices.get().unread_count(), 2);

    kit.core.notices_mark_read(&[a]);
    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.unread_count(), 1);
    assert!(notices.get(a).unwrap().read);
    assert!(!notices.items().iter().find(|n| n.id != a).unwrap().read);

    kit.core.notices_mark_all_read();
    assert_eq!(kit.core.slices().notices.get().unread_count(), 0);
}

#[tokio::test]
async fn clear_keeps_sent_transactions() {
    let kit = kit();
    let sent = sig(1);
    let done = sig(2);
    kit.core.notices_push(tx_kind(sent, TxStatus::Sent));
    kit.core.notices_push(tx_kind(done, TxStatus::Sent));
    assert!(kit
        .core
        .notices_set_tx_status(done, TxStatus::Confirmed { slot: 1 }));
    kit.core.notices_push(NoticeKind::BridgeRequest {
        id: "x".into(),
        origin: "https://example.org".into(),
        method: "connect".into(),
        outcome: BridgeRequestOutcome::Rejected,
    });
    kit.core.notices_push(NoticeKind::VaultLocked {
        reason: LockReason::Idle,
    });

    kit.core.notices_clear();
    let items = kit.core.slices().notices.get();
    assert_eq!(items.len(), 1);
    match &items.items()[0].kind {
        NoticeKind::Transaction {
            signature, status, ..
        } => {
            assert_eq!(*signature, sent);
            assert_eq!(*status, TxStatus::Sent);
        }
        other => panic!("expected Sent transaction, got {other:?}"),
    }
}

#[tokio::test]
async fn remove_one_keeps_sent_transactions() {
    let kit = kit();
    let sent = kit.core.notices_push(tx_kind(sig(1), TxStatus::Sent));
    let lock = kit.core.notices_push(NoticeKind::VaultLocked {
        reason: LockReason::User,
    });
    let other = kit.core.notices_push(NoticeKind::VaultLocked {
        reason: LockReason::Idle,
    });

    kit.core.notices_remove(lock);
    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), 2);
    assert!(notices.get(lock).is_none());
    assert!(notices.get(other).is_some());

    kit.core.notices_remove(sent);
    assert!(kit.core.slices().notices.get().get(sent).is_some());
}

#[tokio::test]
async fn row_text_for_each_kind() {
    let kit = kit();
    let tx_id = kit.core.notices_push(tx_kind(sig(1), TxStatus::Sent));
    let bridge_id = kit.core.notices_push(NoticeKind::BridgeRequest {
        id: "abc".into(),
        origin: "https://example.org".into(),
        method: "connect".into(),
        outcome: BridgeRequestOutcome::Waiting,
    });
    let lock_id = kit.core.notices_push(NoticeKind::VaultLocked {
        reason: LockReason::Hidden,
    });
    let sync_id = kit.core.notices_push(NoticeKind::SyncFailed {
        job: "balances".into(),
        error: "timed out".into(),
    });
    let save_id = kit.core.notices_push(NoticeKind::SaveFailed {
        store: "holdings".into(),
        error: "disk full".into(),
    });

    let notices = kit.core.slices().notices.get();
    assert_eq!(
        notice_row_text(notices.get(tx_id).unwrap()),
        "Transfer · Sent"
    );
    assert_eq!(
        notice_row_text(notices.get(bridge_id).unwrap()),
        "https://example.org asks to connect"
    );
    assert_eq!(
        notice_row_text(notices.get(lock_id).unwrap()),
        "Vault locked · window hidden"
    );
    assert_eq!(
        notice_row_text(notices.get(sync_id).unwrap()),
        "Sync failed · balances: timed out"
    );
    assert_eq!(
        notice_row_text(notices.get(save_id).unwrap()),
        "Save failed · holdings: disk full"
    );
}

#[tokio::test]
async fn notices_survive_restart() {
    let storage = MemoryStorage::default();
    let signature = sig(3);
    let expected = {
        let kit = kit_on(storage.clone());
        kit.core.notices_push(tx_kind(signature, TxStatus::Sent));
        assert!(kit
            .core
            .notices_set_tx_status(signature, TxStatus::Confirmed { slot: 7 }));
        kit.core.notices_dismiss_toast(0);
        settle().await;
        kit.clock.advance(SAVE_INTERVAL);
        settle().await;
        kit.core.flush();
        (*kit.core.slices().notices.get()).clone()
    };
    assert!(storage.read("notices.json").is_some());

    let kit = kit_on(storage);
    let loaded = kit.core.slices().notices.get();
    assert_eq!(*loaded, expected);
}

#[tokio::test]
async fn bridge_outcome_updates_the_waiting_notice() {
    let kit = kit();
    let waiting = kit.core.notices_push(NoticeKind::BridgeRequest {
        id: "req-1".into(),
        origin: "https://example.org".into(),
        method: "signTransaction".into(),
        outcome: BridgeRequestOutcome::Waiting,
    });

    kit.core.notices_set_bridge_outcome(
        "req-1",
        "https://example.org",
        "signTransaction",
        BridgeRequestOutcome::Rejected,
    );

    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), 1);
    let notice = notices.get(waiting).unwrap();
    assert!(matches!(
        notice.kind,
        NoticeKind::BridgeRequest {
            outcome: BridgeRequestOutcome::Rejected,
            ..
        }
    ));
    assert_eq!(notices.unread_count(), 1);

    // A request with no waiting notice (it arrived before the core listened)
    // still gets one.
    kit.core.notices_set_bridge_outcome(
        "req-2",
        "https://example.org",
        "connect",
        BridgeRequestOutcome::Answered,
    );
    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), 2);
    assert!(matches!(
        &notices.items()[0].kind,
        NoticeKind::BridgeRequest { id, outcome: BridgeRequestOutcome::Answered, .. } if id == "req-2"
    ));
}
