//! Saving the stores, checked or deferred.

use std::convert::Infallible;
use std::sync::Arc;

use tracing::debug_span;
use wallet_core::{Holdings, VaultError, Workspace};

use super::deferred::{changed, Deferred, Saves};
use super::{locked, report_checked, Core, Inner, CORE};
use crate::history::History;
use crate::notices::NoticeKind;
use crate::slices::{network_from, settings_from};
use crate::storage::{AppHistory, AppHoldings, AppNotices, AppWorkspace};

impl Core {
    /// Write every deferred store now. A shell calls this before it quits.
    pub fn flush(&self) {
        self.inner.clone().flush_all();
    }

    // Checked: user edits.

    /// Change the workspace (book, cluster, settings) and save it now.
    pub fn workspace_checked(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut Workspace),
    ) -> Result<(), VaultError> {
        self.inner.workspace_checked(name, f)
    }

    /// Change the holdings and save them now.
    pub fn holdings_checked(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut Holdings),
    ) -> Result<(), VaultError> {
        self.inner.holdings_checked(name, f)
    }
}

impl Inner {
    // Publishing. Each runs with its store's lock held, so two threads cannot
    // publish out of order.

    pub(super) fn publish_workspace(&self, workspace: &AppWorkspace) {
        let slices = &self.slices;
        slices
            .book
            .publish_copy(&wallet_core::WorkspaceStore::data(workspace).book);
        slices
            .network
            .publish(network_from(workspace, *locked(&self.reach)));
        slices.settings.publish(settings_from(workspace));
    }

    pub(super) fn publish_holdings(&self, holdings: &AppHoldings) {
        self.slices
            .holdings
            .publish_copy(wallet_core::HoldingsStore::data(holdings));
    }

    pub(super) fn publish_notices(&self, notices: &AppNotices) {
        self.slices.notices.publish_copy(notices.data());
    }

    pub(super) fn publish_history(&self, history: &AppHistory) {
        self.slices.history.publish_copy(history.data());
    }

    // Commands.

    pub(crate) fn workspace_checked(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut Workspace),
    ) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name).entered();
        let saved = self
            .workspace
            .checked(self, |workspace| workspace.change(f));
        report_checked(self.workspace.name(), &saved);
        saved
    }

    pub(crate) fn holdings_checked(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut Holdings),
    ) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name).entered();
        let saved = self.holdings.checked(self, |holdings| holdings.change(f));
        report_checked(self.holdings.name(), &saved);
        saved
    }

    pub(crate) fn workspace_deferred(
        self: &Arc<Self>,
        name: &'static str,
        f: impl FnOnce(&mut Workspace),
    ) {
        let Ok(()) = self.workspace.change(self, name, |workspace| {
            f(workspace);
            Ok::<_, Infallible>(())
        });
    }

    pub(crate) fn holdings_deferred(
        self: &Arc<Self>,
        name: &'static str,
        f: impl FnOnce(&mut Holdings),
    ) {
        let Ok(()) = self.holdings.change(self, name, |holdings| {
            f(holdings);
            Ok::<_, Infallible>(())
        });
    }

    pub(crate) fn history_deferred(
        self: &Arc<Self>,
        name: &'static str,
        f: impl FnOnce(&mut History) -> bool,
    ) {
        let _ = self
            .history
            .change(self, name, |history| changed(f(history), "unchanged"));
    }

    // Deferred saving.

    /// Write `store` if it is dirty, and put a failure in the notices.
    pub(super) fn save_deferred<S: Saves>(self: &Arc<Self>, store: &Deferred<S>) {
        if let Err(error) = store.save_if_dirty(&self.events) {
            self.save_failed(store.name(), error);
        }
    }

    /// The notice for a deferred save that failed. Not when notices.json
    /// itself failed: that would mark the notices dirty again and loop.
    fn save_failed(self: &Arc<Self>, store: &'static str, error: String) {
        if store != self.notices.name() {
            let _ = self.notices_push(NoticeKind::SaveFailed {
                store: store.to_owned(),
                error,
            });
        }
    }

    pub(super) fn flush_all(self: &Arc<Self>) {
        self.save_deferred(&self.workspace);
        self.save_deferred(&self.holdings);
        self.save_deferred(&self.notices);
        self.save_deferred(&self.history);
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The last handle went away with changes still waiting for the timer.
        // Nothing can be pushed now, so a failure is only sent as an event.
        let _ = self.workspace.save_if_dirty(&self.events);
        let _ = self.holdings.save_if_dirty(&self.events);
        let _ = self.notices.save_if_dirty(&self.events);
        let _ = self.history.save_if_dirty(&self.events);
    }
}
