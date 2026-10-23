//! Shell-side rules: core events → OS toasts while the window is hidden.

use app_core::{
    BridgeRequestOutcome, Event, NoticeId, NoticeKind, Notification, PublicCore, TxStatus,
};

/// Listen for core events and, while the UI is hidden, raise OS toasts.
/// Also records [`NoticeKind::BridgeRequest`] when a request arrives or ends.
///
/// Runs until the process exits. Spawn from the desktop shell after the core opens.
pub async fn run_hidden_notifier(core: PublicCore) {
    let mut events = core.events();
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        match event {
            Event::RequestArrived { id } => on_request_arrived(&core, &id),
            Event::TxStatusChanged { signature, status } => {
                on_tx_final(&core, signature, status);
            }
            _ => {}
        }
    }
}

fn on_request_arrived(core: &PublicCore, id: &str) {
    let queue = core.slices().bridge.get();
    let Some(request) = queue.pending().iter().find(|r| r.id == id) else {
        return;
    };
    let origin = request.origin.clone();
    let method = request.method.clone();
    let code = request.code();

    let notice_id = core.notices_push(NoticeKind::BridgeRequest {
        id: id.to_owned(),
        origin: origin.clone(),
        method: method.clone(),
        outcome: BridgeRequestOutcome::Waiting,
    });

    if core.ui_visible() {
        return;
    }

    let action = if method == "connect" {
        "asks to connect"
    } else {
        "asks to sign"
    };
    let title = "Airlock".to_owned();
    let body = if origin.is_empty() {
        format!("A site {action}")
    } else {
        format!("{origin} {action}")
    };
    core.ports().notifier.notify(Notification {
        title,
        body,
        notice_id,
    });
    tracing::info!(
        target: "airlock::bridge",
        code = %code,
        "notified"
    );
}

fn on_tx_final(core: &PublicCore, signature: solana_signature::Signature, status: TxStatus) {
    if !status.is_final() || core.ui_visible() {
        return;
    }
    let notices = core.slices().notices.get();
    let Some(notice) = notices.items().iter().find(|n| {
        matches!(
            &n.kind,
            NoticeKind::Transaction { signature: sig, .. } if *sig == signature
        )
    }) else {
        return;
    };
    let notice_id = notice.id;
    let NoticeKind::Transaction { label, status, .. } = &notice.kind else {
        return;
    };
    let title = label.clone();
    let body = status.label().to_owned();
    core.ports().notifier.notify(Notification {
        title,
        body,
        notice_id,
    });
}

/// What a toast click should open: the waiting request modal, or the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastClickAction {
    /// Re-open the bridge request modal for a still-waiting request.
    OpenRequestModal,
    /// Open the notification center on this notice (or the list if unknown).
    OpenNotices { focus: Option<NoticeId> },
}

/// Decide where a toast click should land.
pub fn toast_click_action(core: &PublicCore, notice_id: NoticeId) -> ToastClickAction {
    let notices = core.slices().notices.get();
    let Some(notice) = notices.get(notice_id) else {
        return ToastClickAction::OpenNotices { focus: None };
    };
    match &notice.kind {
        NoticeKind::BridgeRequest {
            id,
            outcome: BridgeRequestOutcome::Waiting,
            ..
        } => {
            let queue = core.slices().bridge.get();
            if queue.pending().iter().any(|r| r.id == *id) {
                ToastClickAction::OpenRequestModal
            } else {
                ToastClickAction::OpenNotices {
                    focus: Some(notice_id),
                }
            }
        }
        _ => ToastClickAction::OpenNotices {
            focus: Some(notice_id),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::testing::kit;
    use app_core::{bridge::BridgeRequest, TxStatus};
    use solana_signature::Signature;
    use wallet_core::Cluster;

    #[tokio::test]
    async fn waiting_request_opens_modal() {
        let kit = kit();
        let public = kit.core.public();
        public.bridge_push(
            BridgeRequest {
                id: "req-1".into(),
                origin: "https://example.org".into(),
                method: "connect".into(),
                params: serde_json::Value::Null,
            },
            Box::new(|_| {}),
        );
        let id = public.notices_push(NoticeKind::BridgeRequest {
            id: "req-1".into(),
            origin: "https://example.org".into(),
            method: "connect".into(),
            outcome: BridgeRequestOutcome::Waiting,
        });
        assert_eq!(
            toast_click_action(&public, id),
            ToastClickAction::OpenRequestModal
        );
    }

    #[tokio::test]
    async fn decided_notice_opens_panel() {
        let kit = kit();
        let public = kit.core.public();
        let mut bytes = [0u8; 64];
        bytes[0] = 1;
        let signature = Signature::from(bytes);
        let id = public.notices_push(NoticeKind::Transaction {
            flow: 1,
            signature,
            cluster: Cluster::Devnet,
            label: "Transfer".into(),
            status: TxStatus::Confirmed { slot: 1 },
        });
        assert_eq!(
            toast_click_action(&public, id),
            ToastClickAction::OpenNotices { focus: Some(id) }
        );

        let rejected = public.notices_push(NoticeKind::BridgeRequest {
            id: "gone".into(),
            origin: "https://example.org".into(),
            method: "connect".into(),
            outcome: BridgeRequestOutcome::Rejected,
        });
        assert_eq!(
            toast_click_action(&public, rejected),
            ToastClickAction::OpenNotices {
                focus: Some(rejected)
            }
        );
    }
}
