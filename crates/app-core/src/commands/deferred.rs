//! A store saved deferred: its lock, whether memory is ahead of its file, and
//! the task that writes it at most once per [`SAVE_INTERVAL`].

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::{debug, debug_span, warn};
use wallet_core::VaultError;

use super::{Inner, CORE, SAVE_INTERVAL};
use crate::events::Event;
use crate::history::History;
use crate::notices::Notices;
use crate::ranked::{Rank, Ranked, RankedGuard};
use crate::storage::{AppHistory, AppHoldings, AppNotices, AppWorkspace};

/// What the core needs of a store it saves deferred.
pub(super) trait Saves: 'static {
    type Data;

    fn data_mut(&mut self) -> &mut Self::Data;

    fn save(&self) -> Result<(), VaultError>;
}

/// What a command logs as its outcome when it changed nothing.
pub(super) trait Unchanged {
    fn outcome(&self) -> &'static str;
}

impl Unchanged for &'static str {
    fn outcome(&self) -> &'static str {
        self
    }
}

/// For a command that always changes the store.
impl Unchanged for Infallible {
    fn outcome(&self) -> &'static str {
        match *self {}
    }
}

pub(super) struct Deferred<S> {
    name: &'static str,
    store: Ranked<S>,
    /// Memory holds something the store does not.
    dirty: AtomicBool,
    /// A task is waiting to write it.
    scheduled: AtomicBool,
    /// Publishes the store's slices. Runs with its lock held, so two threads
    /// cannot publish out of order.
    publish: fn(&Inner, &S),
    /// Where the core keeps this store, for the task that writes it.
    find: fn(&Inner) -> &Deferred<S>,
}

impl<S: Saves> Deferred<S> {
    pub(super) fn new(
        rank: Rank,
        name: &'static str,
        store: S,
        publish: fn(&Inner, &S),
        find: fn(&Inner) -> &Deferred<S>,
    ) -> Self {
        Self {
            name,
            store: Ranked::new(rank, store),
            dirty: AtomicBool::new(false),
            scheduled: AtomicBool::new(false),
            publish,
            find,
        }
    }

    pub(super) fn name(&self) -> &'static str {
        self.name
    }

    pub(super) fn lock(&self) -> RankedGuard<'_, S> {
        self.store.lock()
    }

    /// Run command `name` on the store. `f` answers what the command gives
    /// back, or, when it changed nothing, why: then nothing is published or
    /// saved, and that is the outcome logged.
    pub(super) fn change<R, E: Unchanged>(
        &self,
        core: &Arc<Inner>,
        name: &'static str,
        f: impl FnOnce(&mut S::Data) -> Result<R, E>,
    ) -> Result<R, E> {
        let _span = debug_span!(target: CORE, "command", name).entered();
        match self.edit(core, f) {
            Ok(out) => {
                self.mark_dirty(core);
                debug!(target: CORE, outcome = "ok", "command");
                Ok(out)
            }
            Err(why) => {
                debug!(target: CORE, outcome = why.outcome(), "command");
                Err(why)
            }
        }
    }

    /// Change the store with `f` and publish it when `f` says it changed,
    /// but neither log nor save: for a command that does more before it
    /// marks the store dirty.
    pub(super) fn edit<R, E>(
        &self,
        core: &Inner,
        f: impl FnOnce(&mut S::Data) -> Result<R, E>,
    ) -> Result<R, E> {
        let mut store = self.store.lock();
        let out = f(store.data_mut());
        if out.is_ok() {
            (self.publish)(core, &store);
        }
        out
    }

    /// Change the store with `change`, which saves it now. When that works
    /// the file holds everything, and the slices are published.
    pub(super) fn checked(
        &self,
        core: &Inner,
        change: impl FnOnce(&mut S) -> Result<(), VaultError>,
    ) -> Result<(), VaultError> {
        let mut store = self.store.lock();
        let saved = change(&mut store);
        if saved.is_ok() {
            self.saved();
            (self.publish)(core, &store);
        }
        saved
    }

    /// The store was just saved whole, so nothing is left to write.
    pub(super) fn saved(&self) {
        self.dirty.store(false, Ordering::SeqCst);
    }

    /// Note that the store is ahead of its file, and make sure a task will
    /// write it. Changes that arrive before the task wakes ride along.
    pub(super) fn mark_dirty(&self, core: &Arc<Inner>) {
        self.dirty.store(true, Ordering::SeqCst);
        if self.scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let clock = core.ports.clock.clone();
        let find = self.find;
        let core_ref = Arc::downgrade(core);
        core.ports.spawner.spawn(Box::pin(async move {
            // Made in the task: a timer needs a runtime, and the bridge's
            // threads have none.
            clock.sleep(SAVE_INTERVAL).await;
            if let Some(core) = core_ref.upgrade() {
                core.save_deferred(find(&core));
            }
        }));
    }

    /// Write the store if it is dirty. A failure keeps it dirty, so the next
    /// change or a flush tries again, sends `SaveFailed` and comes back as
    /// the error's text.
    pub(super) fn save_if_dirty(&self, events: &broadcast::Sender<Event>) -> Result<(), String> {
        self.scheduled.store(false, Ordering::SeqCst);
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let saved = self.store.lock().save();
        match saved {
            Ok(()) => {
                debug!(target: CORE, store = self.name, policy = "deferred", result = "ok", "save");
                Ok(())
            }
            Err(err) => {
                // Keep it dirty: the next change, or a flush, tries again.
                self.dirty.store(true, Ordering::SeqCst);
                warn!(
                    target: CORE,
                    store = self.name,
                    policy = "deferred",
                    error = %err,
                    "save failed; the change is kept in memory"
                );
                let error = err.to_string();
                let _ = events.send(Event::SaveFailed {
                    store: self.name,
                    error: error.clone(),
                });
                Err(error)
            }
        }
    }
}

/// What a command that says whether it changed the store answers: `outcome`
/// when it did not.
pub(super) fn changed(changed: bool, outcome: &'static str) -> Result<(), &'static str> {
    if changed {
        Ok(())
    } else {
        Err(outcome)
    }
}

impl Saves for AppWorkspace {
    type Data = wallet_core::Workspace;

    fn data_mut(&mut self) -> &mut Self::Data {
        AppWorkspace::data_mut(self)
    }

    fn save(&self) -> Result<(), VaultError> {
        AppWorkspace::save(self)
    }
}

impl Saves for AppHoldings {
    type Data = wallet_core::Holdings;

    fn data_mut(&mut self) -> &mut Self::Data {
        AppHoldings::data_mut(self)
    }

    fn save(&self) -> Result<(), VaultError> {
        AppHoldings::save(self)
    }
}

impl Saves for AppNotices {
    type Data = Notices;

    fn data_mut(&mut self) -> &mut Self::Data {
        AppNotices::data_mut(self)
    }

    fn save(&self) -> Result<(), VaultError> {
        AppNotices::save(self)
    }
}

impl Saves for AppHistory {
    type Data = History;

    fn data_mut(&mut self) -> &mut Self::Data {
        AppHistory::data_mut(self)
    }

    fn save(&self) -> Result<(), VaultError> {
        AppHistory::save(self)
    }
}
