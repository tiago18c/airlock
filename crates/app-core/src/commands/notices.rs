//! The notices' commands.

use std::convert::Infallible;
use std::sync::Arc;

use solana_signature::Signature;
use tracing::{debug, debug_span};

use super::deferred::changed;
use super::{Inner, CORE};
use crate::diagnostics::log_status;
use crate::events::Event;
use crate::notices::{BridgeRequestOutcome, NoticeId, NoticeKind, TxStatus};

impl Inner {
    pub(crate) fn notices_push(self: &Arc<Self>, kind: NoticeKind) -> NoticeId {
        let now = self.now_secs();
        let Ok(id) = self.notices.change(self, "notices_push", |notices| {
            Ok::<_, Infallible>(notices.push(kind, now))
        });
        id
    }

    pub(crate) fn notices_set_tx_status(
        self: &Arc<Self>,
        signature: Signature,
        status: TxStatus,
    ) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "notices_set_tx_status").entered();
        let now = self.now_secs();
        let Ok(updated) = self.notices.edit(self, |notices| {
            notices.set_tx_status(&signature, status, now).ok_or(())
        }) else {
            debug!(target: CORE, outcome = "ignored", "command");
            return false;
        };
        let (flow, status) = match &updated.kind {
            NoticeKind::Transaction { flow, status, .. } => (*flow, status.clone()),
            other => {
                debug!(target: CORE, outcome = "ignored", kind = ?other, "command");
                return false;
            }
        };
        log_status(flow, &signature, &status);
        // History is what happened on chain: one that landed is kept, with its
        // outcome, and one that did not goes.
        self.history_deferred("history_status", |history| {
            history.set_status(&signature, &status)
        });
        let _ = self
            .events
            .send(Event::TxStatusChanged { signature, status });
        self.notices.mark_dirty(self);
        debug!(target: CORE, outcome = "ok", "command");
        true
    }

    pub(crate) fn notices_set_bridge_outcome(
        self: &Arc<Self>,
        request_id: &str,
        origin: &str,
        method: &str,
        outcome: BridgeRequestOutcome,
    ) {
        let _span =
            debug_span!(target: CORE, "command", name = "notices_set_bridge_outcome").entered();
        let now = self.now_secs();
        let found = self.notices.edit(self, |notices| {
            if notices.set_bridge_outcome(request_id, outcome, now) {
                Ok(())
            } else {
                Err(())
            }
        });
        if found.is_ok() {
            self.notices.mark_dirty(self);
            debug!(target: CORE, outcome = "ok", "command");
            return;
        }
        // The request arrived before the core was listening, so no waiting
        // notice was pushed. Record the outcome on its own.
        let _ = self.notices_push(NoticeKind::BridgeRequest {
            id: request_id.to_owned(),
            origin: origin.to_owned(),
            method: method.to_owned(),
            outcome,
        });
    }

    pub(crate) fn notices_dismiss_toast(self: &Arc<Self>, id: NoticeId) {
        let _ = self
            .notices
            .change(self, "notices_dismiss_toast", |notices| {
                changed(notices.dismiss_toast(id), "unknown id")
            });
    }

    pub(crate) fn notices_mark_read(self: &Arc<Self>, ids: &[NoticeId]) {
        let _ = self.notices.change(self, "notices_mark_read", |notices| {
            changed(notices.mark_read(ids), "unchanged")
        });
    }

    pub(crate) fn notices_mark_all_read(self: &Arc<Self>) {
        let _ = self
            .notices
            .change(self, "notices_mark_all_read", |notices| {
                changed(notices.mark_all_read(), "unchanged")
            });
    }

    pub(crate) fn notices_remove(self: &Arc<Self>, id: NoticeId) {
        let _ = self.notices.change(self, "notices_remove", |notices| {
            changed(notices.remove(id), "unchanged")
        });
    }

    pub(crate) fn notices_clear(self: &Arc<Self>) {
        let Ok(()) = self.notices.change(self, "notices_clear", |notices| {
            notices.clear();
            Ok::<_, Infallible>(())
        });
    }

    pub(crate) fn report_sync_failed(self: &Arc<Self>, job: &str, error: &str) {
        let _ = self.events.send(Event::SyncFailed {
            job: job.to_owned(),
            error: error.to_owned(),
        });
        let _ = self.notices_push(NoticeKind::SyncFailed {
            job: job.to_owned(),
            error: error.to_owned(),
        });
    }
}
