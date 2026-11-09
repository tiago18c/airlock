//! The notices' commands.

use std::sync::Arc;

use solana_signature::Signature;
use tracing::{debug, debug_span};

use super::saving::Store;
use super::{locked, Core, Inner, CORE};
use crate::diagnostics::log_status;
use crate::events::Event;
use crate::notices::{BridgeRequestOutcome, NoticeId, NoticeKind, TxStatus};

impl Core {
    /// Push a notice and save deferred.
    pub fn notices_push(&self, kind: NoticeKind) -> NoticeId {
        self.inner.notices_push(kind)
    }

    /// Move a transaction notice's status forward (ignored if it would regress).
    pub fn notices_set_tx_status(&self, signature: Signature, status: TxStatus) -> bool {
        self.inner.notices_set_tx_status(signature, status)
    }

    /// Hide the toast; the notice stays in the list.
    pub fn notices_dismiss_toast(&self, id: NoticeId) {
        self.inner.notices_dismiss_toast(id);
    }

    /// Record how a browser request ended, on the notice pushed when it
    /// arrived, or on a new one when none was.
    pub fn notices_set_bridge_outcome(
        &self,
        request_id: &str,
        origin: &str,
        method: &str,
        outcome: BridgeRequestOutcome,
    ) {
        self.inner
            .notices_set_bridge_outcome(request_id, origin, method, outcome);
    }

    /// Mark the given notices read.
    pub fn notices_mark_read(&self, ids: &[NoticeId]) {
        self.inner.notices_mark_read(ids);
    }

    /// Mark every notice read.
    pub fn notices_mark_all_read(&self) {
        self.inner.notices_mark_all_read();
    }

    /// Remove one notice; a transaction still `Sent` stays until it ends.
    pub fn notices_remove(&self, id: NoticeId) {
        self.inner.notices_remove(id);
    }

    /// Clear notices; transactions still `Sent` stay until they end.
    pub fn notices_clear(&self) {
        self.inner.notices_clear();
    }
}

impl Inner {
    pub(crate) fn notices_push(self: &Arc<Self>, kind: NoticeKind) -> NoticeId {
        let _span = debug_span!(target: CORE, "command", name = "notices_push").entered();
        let now = self.now_secs();
        let id = {
            let mut notices = locked(&self.notices);
            let id = notices.data_mut().push(kind, now);
            self.publish_notices(&notices);
            id
        };
        self.mark_dirty(Store::Notices);
        debug!(target: CORE, outcome = "ok", "command");
        id
    }

    pub(crate) fn notices_set_tx_status(
        self: &Arc<Self>,
        signature: Signature,
        status: TxStatus,
    ) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "notices_set_tx_status").entered();
        let now = self.now_secs();
        let updated = {
            let mut notices = locked(&self.notices);
            let Some(notice) = notices.data_mut().set_tx_status(&signature, status, now) else {
                debug!(target: CORE, outcome = "ignored", "command");
                return false;
            };
            self.publish_notices(&notices);
            notice
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
        self.mark_dirty(Store::Notices);
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
        let found = {
            let mut notices = locked(&self.notices);
            let found = notices
                .data_mut()
                .set_bridge_outcome(request_id, outcome, now);
            if found {
                self.publish_notices(&notices);
            }
            found
        };
        if found {
            self.mark_dirty(Store::Notices);
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
        let _span = debug_span!(target: CORE, "command", name = "notices_dismiss_toast").entered();
        {
            let mut notices = locked(&self.notices);
            if !notices.data_mut().dismiss_toast(id) {
                debug!(target: CORE, outcome = "unknown id", "command");
                return;
            }
            self.publish_notices(&notices);
        }
        self.mark_dirty(Store::Notices);
        debug!(target: CORE, outcome = "ok", "command");
    }

    pub(crate) fn notices_mark_read(self: &Arc<Self>, ids: &[NoticeId]) {
        let _span = debug_span!(target: CORE, "command", name = "notices_mark_read").entered();
        {
            let mut notices = locked(&self.notices);
            if !notices.data_mut().mark_read(ids) {
                debug!(target: CORE, outcome = "unchanged", "command");
                return;
            }
            self.publish_notices(&notices);
        }
        self.mark_dirty(Store::Notices);
        debug!(target: CORE, outcome = "ok", "command");
    }

    pub(crate) fn notices_mark_all_read(self: &Arc<Self>) {
        let _span = debug_span!(target: CORE, "command", name = "notices_mark_all_read").entered();
        {
            let mut notices = locked(&self.notices);
            if !notices.data_mut().mark_all_read() {
                debug!(target: CORE, outcome = "unchanged", "command");
                return;
            }
            self.publish_notices(&notices);
        }
        self.mark_dirty(Store::Notices);
        debug!(target: CORE, outcome = "ok", "command");
    }

    pub(crate) fn notices_remove(self: &Arc<Self>, id: NoticeId) {
        let _span = debug_span!(target: CORE, "command", name = "notices_remove").entered();
        {
            let mut notices = locked(&self.notices);
            if !notices.data_mut().remove(id) {
                debug!(target: CORE, outcome = "unchanged", "command");
                return;
            }
            self.publish_notices(&notices);
        }
        self.mark_dirty(Store::Notices);
        debug!(target: CORE, outcome = "ok", "command");
    }

    pub(crate) fn notices_clear(self: &Arc<Self>) {
        let _span = debug_span!(target: CORE, "command", name = "notices_clear").entered();
        {
            let mut notices = locked(&self.notices);
            notices.data_mut().clear();
            self.publish_notices(&notices);
        }
        self.mark_dirty(Store::Notices);
        debug!(target: CORE, outcome = "ok", "command");
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
