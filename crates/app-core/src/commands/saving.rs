//! Saving the stores, checked or deferred.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing::{debug, debug_span, warn};
use wallet_core::{Holdings, VaultError, Workspace};

use super::{locked, report_checked, Core, Inner, CORE, SAVE_INTERVAL};
use crate::events::Event;
use crate::history::History;
use crate::notices::NoticeKind;
use crate::slices::{network_from, settings_from};
use crate::storage::{AppHistory, AppHoldings, AppNotices, AppWorkspace};

#[derive(Clone, Copy)]
pub(super) enum Store {
    Workspace,
    Holdings,
    Notices,
    History,
}

impl Store {
    const ALL: [Store; 4] = [
        Self::Workspace,
        Self::Holdings,
        Self::Notices,
        Self::History,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Holdings => "holdings",
            Self::Notices => "notices",
            Self::History => "history",
        }
    }
}

/// Where a deferred store stands.
#[derive(Default)]
pub(super) struct Saver {
    /// Memory holds something the store does not.
    pub(super) dirty: AtomicBool,
    /// A task is waiting to write it.
    scheduled: AtomicBool,
}

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

    // Deferred: caches that can be fetched again.

    pub fn workspace_deferred(&self, name: &'static str, f: impl FnOnce(&mut Workspace)) {
        self.inner.workspace_deferred(name, f);
    }

    pub fn holdings_deferred(&self, name: &'static str, f: impl FnOnce(&mut Holdings)) {
        self.inner.holdings_deferred(name, f);
    }

    /// Change the transaction history. `f` says whether it changed anything.
    pub fn history_deferred(&self, name: &'static str, f: impl FnOnce(&mut History) -> bool) {
        self.inner.history_deferred(name, f);
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
        let mut workspace = locked(&self.workspace);
        let saved = workspace.change(f);
        if saved.is_ok() {
            self.workspace_saver.dirty.store(false, Ordering::SeqCst);
            self.publish_workspace(&workspace);
        }
        report_checked(Store::Workspace.name(), &saved);
        saved
    }

    pub(crate) fn holdings_checked(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut Holdings),
    ) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name).entered();
        let mut holdings = locked(&self.holdings);
        let saved = holdings.change(f);
        if saved.is_ok() {
            self.holdings_saver.dirty.store(false, Ordering::SeqCst);
            self.publish_holdings(&holdings);
        }
        report_checked(Store::Holdings.name(), &saved);
        saved
    }

    pub(crate) fn workspace_deferred(
        self: &Arc<Self>,
        name: &'static str,
        f: impl FnOnce(&mut Workspace),
    ) {
        let _span = debug_span!(target: CORE, "command", name).entered();
        {
            let mut workspace = locked(&self.workspace);
            f(workspace.data_mut());
            self.publish_workspace(&workspace);
        }
        self.mark_dirty(Store::Workspace);
        debug!(target: CORE, outcome = "ok", "command");
    }

    pub(crate) fn holdings_deferred(
        self: &Arc<Self>,
        name: &'static str,
        f: impl FnOnce(&mut Holdings),
    ) {
        let _span = debug_span!(target: CORE, "command", name).entered();
        {
            let mut holdings = locked(&self.holdings);
            f(holdings.data_mut());
            self.publish_holdings(&holdings);
        }
        self.mark_dirty(Store::Holdings);
        debug!(target: CORE, outcome = "ok", "command");
    }

    pub(crate) fn history_deferred(
        self: &Arc<Self>,
        name: &'static str,
        f: impl FnOnce(&mut History) -> bool,
    ) {
        let _span = debug_span!(target: CORE, "command", name).entered();
        {
            let mut history = locked(&self.history);
            if !f(history.data_mut()) {
                debug!(target: CORE, outcome = "unchanged", "command");
                return;
            }
            self.publish_history(&history);
        }
        self.mark_dirty(Store::History);
        debug!(target: CORE, outcome = "ok", "command");
    }

    // Deferred saving.

    fn saver(&self, store: Store) -> &Saver {
        match store {
            Store::Workspace => &self.workspace_saver,
            Store::Holdings => &self.holdings_saver,
            Store::Notices => &self.notices_saver,
            Store::History => &self.history_saver,
        }
    }

    /// Note that `store` is ahead of its file, and make sure a task will write
    /// it. Changes that arrive before the task wakes ride along.
    pub(super) fn mark_dirty(self: &Arc<Self>, store: Store) {
        let saver = self.saver(store);
        saver.dirty.store(true, Ordering::SeqCst);
        if saver.scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let clock = self.ports.clock.clone();
        let core = Arc::downgrade(self);
        self.ports.spawner.spawn(Box::pin(async move {
            // Made in the task: a timer needs a runtime, and the bridge's
            // threads have none.
            clock.sleep(SAVE_INTERVAL).await;
            if let Some(core) = core.upgrade() {
                core.write_if_dirty(store);
            }
        }));
    }

    fn write_if_dirty(self: &Arc<Self>, store: Store) {
        let saver = self.saver(store);
        saver.scheduled.store(false, Ordering::SeqCst);
        if !saver.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        let saved = self.save_store(store);
        match saved {
            Ok(()) => {
                debug!(target: CORE, store = store.name(), policy = "deferred", result = "ok", "save");
            }
            Err(err) => {
                // Keep it dirty: the next change, or a flush, tries again.
                saver.dirty.store(true, Ordering::SeqCst);
                warn!(
                    target: CORE,
                    store = store.name(),
                    policy = "deferred",
                    error = %err,
                    "save failed; the change is kept in memory"
                );
                let error = err.to_string();
                let _ = self.events.send(Event::SaveFailed {
                    store: store.name(),
                    error: error.clone(),
                });
                // Do not push a notice when notices.json itself failed: that
                // would mark Notices dirty again and loop.
                if !matches!(store, Store::Notices) {
                    let _ = self.notices_push(NoticeKind::SaveFailed {
                        store: store.name().to_owned(),
                        error,
                    });
                }
            }
        }
    }

    fn save_store(&self, store: Store) -> Result<(), VaultError> {
        match store {
            Store::Workspace => locked(&self.workspace).save(),
            Store::Holdings => locked(&self.holdings).save(),
            Store::Notices => locked(&self.notices).save(),
            Store::History => locked(&self.history).save(),
        }
    }

    pub(super) fn flush_all(self: &Arc<Self>) {
        for store in Store::ALL {
            self.write_if_dirty(store);
        }
    }

    /// Flush without Arc (Drop). Save failures still emit events, not notices.
    fn flush_all_dropping(&self) {
        for store in Store::ALL {
            let saver = self.saver(store);
            saver.scheduled.store(false, Ordering::SeqCst);
            if !saver.dirty.swap(false, Ordering::SeqCst) {
                continue;
            }
            let saved = self.save_store(store);
            match saved {
                Ok(()) => {
                    debug!(target: CORE, store = store.name(), policy = "deferred", result = "ok", "save");
                }
                Err(err) => {
                    saver.dirty.store(true, Ordering::SeqCst);
                    warn!(
                        target: CORE,
                        store = store.name(),
                        policy = "deferred",
                        error = %err,
                        "save failed; the change is kept in memory"
                    );
                    let _ = self.events.send(Event::SaveFailed {
                        store: store.name(),
                        error: err.to_string(),
                    });
                }
            }
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The last handle went away with changes still waiting for the timer.
        self.flush_all_dropping();
    }
}
