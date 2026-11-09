//! The core: the stores, the slices published from them, and the commands that
//! change them.
//!
//! A command changes a store, publishes the new snapshot and saves, with one
//! of two policies:
//!
//! - **checked**: save now. When the save fails the change is rolled back,
//!   nothing is published and the error is returned.
//! - **deferred**: mark the store dirty. A task writes it once a second at
//!   most, and a failure is sent as `SaveFailed` while the change stays.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, watch};
use tracing::{debug, warn};
use wallet_core::VaultError;

use crate::bridge::BridgeState;
use crate::events::Event;
use crate::lock::LockPolicy;
use crate::ports::Ports;
use crate::public::PublicCore;
use crate::ranked::{Rank, Ranked, RankedGuard};
use crate::slices::{
    book_from, history_from, holdings_from, network_from, notices_from, settings_from,
    shown_wallets, vault_state_from, wallets_view_from, Reach, Slice, Slices,
};
use crate::storage::{
    open_history_in, open_holdings_in, open_leaf_marks_in, open_notices_in, open_vault_in,
    open_wallet_list_in, open_workspace_in, AppHistory, AppHoldings, AppLeafMarks, AppNotices,
    AppVault, AppWalletList, AppWorkspace,
};
use saving::{Saver, Store};
use sites::sites_view;

mod notices;
mod saving;
mod settings;
mod sites;
mod vault;

/// How often a deferred store is written at most.
pub const SAVE_INTERVAL: Duration = Duration::from_secs(1);

const EVENT_BACKLOG: usize = 64;
const CORE: &str = "airlock::core";
const VAULT: &str = "airlock::vault";

/// The whole core, with the vault's secrets. Shells that start things hold
/// this; services get [`PublicCore`].
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    ports: Ports,
    vault: Ranked<AppVault>,
    /// What each hash-based key has signed with. Written before the vault when a
    /// leaf is reserved.
    leaf_marks: Ranked<AppLeafMarks>,
    wallet_list: Ranked<AppWalletList>,
    workspace: Ranked<AppWorkspace>,
    holdings: Ranked<AppHoldings>,
    notices: Ranked<AppNotices>,
    history: Ranked<AppHistory>,
    bridge: Ranked<BridgeState>,
    slices: Slices,
    events: broadcast::Sender<Event>,
    visible: watch::Sender<bool>,
    activity: watch::Sender<()>,
    reach: Ranked<Reach>,
    workspace_saver: Saver,
    holdings_saver: Saver,
    notices_saver: Saver,
    history_saver: Saver,
}

fn locked<T>(mutex: &Ranked<T>) -> RankedGuard<'_, T> {
    mutex.lock()
}

impl Core {
    /// Open the stores and start the auto-lock. Runs on `ports.spawner`, so a
    /// native shell calls this inside its tokio runtime.
    pub fn open(ports: Ports) -> Core {
        let vault = open_vault_in(&*ports.storage);
        if let Some(why) = vault.read_problem() {
            warn!(target: VAULT, error = %why, "could not read the vault file");
        }
        let wallet_list = open_wallet_list_in(&*ports.storage);
        let leaf_marks = open_leaf_marks_in(&*ports.storage);
        if let Some(problem) = leaf_marks.load_problem() {
            warn!(target: CORE, store = "leaf marks", problem = %problem.message(), "load problem");
        }
        let workspace = open_workspace_in(&*ports.storage);
        let holdings = open_holdings_in(&*ports.storage);
        let mut notices = open_notices_in(&*ports.storage);
        let history = open_history_in(&*ports.storage);
        if let Some(problem) = history.load_problem() {
            warn!(target: CORE, store = "history", problem = %problem.message(), "load problem");
        }
        if let Some(problem) = workspace.load_problem() {
            warn!(target: CORE, store = "workspace", problem = %problem.message(), "load problem");
        }
        if let Some(problem) = workspace.grants_problem() {
            warn!(target: CORE, store = "grants", problem = %problem.message(), "load problem");
        }
        if let Some(problem) = holdings.load_problem() {
            warn!(target: CORE, store = "holdings", problem = %problem.message(), "load problem");
        }
        if let Some(problem) = notices.load_problem() {
            warn!(target: CORE, store = "notices", problem = %problem.message(), "load problem");
        }
        let now_secs = (ports.clock.now_ms() / 1000) as i64;
        let notices_pruned = notices.prune_expired(now_secs);

        let settings = settings_from(&workspace);
        let slices = Slices {
            wallets: Slice::new(wallets_view_from(
                shown_wallets(&vault, &wallet_list),
                &leaf_marks,
            )),
            book: Slice::new(book_from(&workspace)),
            holdings: Slice::new(holdings_from(&holdings)),
            network: Slice::new(network_from(&workspace, Reach::default())),
            vault: Slice::new(Arc::new(vault_state_from(&vault))),
            settings: Slice::new(settings),
            bridge: Slice::new(Arc::default()),
            sites: Slice::new(Arc::new(sites_view(&workspace, &BridgeState::default()))),
            notices: Slice::new(notices_from(&notices)),
            history: Slice::new(history_from(&history)),
            sync: Slice::new(Arc::new(crate::slices::SyncStatus::default())),
        };
        let inner = Arc::new(Inner {
            ports,
            vault: Ranked::new(Rank::Vault, vault),
            leaf_marks: Ranked::new(Rank::LeafMarks, leaf_marks),
            wallet_list: Ranked::new(Rank::WalletList, wallet_list),
            workspace: Ranked::new(Rank::Workspace, workspace),
            holdings: Ranked::new(Rank::Holdings, holdings),
            notices: Ranked::new(Rank::Notices, notices),
            history: Ranked::new(Rank::History, history),
            bridge: Ranked::new(Rank::Bridge, BridgeState::default()),
            slices,
            events: broadcast::channel(EVENT_BACKLOG).0,
            visible: watch::channel(true).0,
            activity: watch::channel(()).0,
            reach: Ranked::new(Rank::Reach, Reach::default()),
            workspace_saver: Saver::default(),
            holdings_saver: Saver::default(),
            notices_saver: Saver::default(),
            history_saver: Saver::default(),
        });
        if notices_pruned {
            inner.mark_dirty(Store::Notices);
        }

        let policy = LockPolicy::new(
            inner.visible.subscribe(),
            inner.activity.subscribe(),
            inner.slices.settings.subscribe(),
            inner.ports.clock.clone(),
            Arc::downgrade(&inner),
        );
        inner.ports.spawner.spawn(Box::pin(policy.run()));
        Core { inner }
    }

    /// The handle for background services: no way to reach the secrets.
    pub fn public(&self) -> PublicCore {
        PublicCore::new(self.inner.clone())
    }

    pub fn slices(&self) -> &Slices {
        &self.inner.slices
    }

    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    pub fn ports(&self) -> &Ports {
        &self.inner.ports
    }

    /// The shell reports whether the window is on screen.
    pub fn set_ui_visible(&self, visible: bool) {
        self.inner.set_ui_visible(visible);
    }

    /// Whether the shell last reported the window as visible.
    pub fn ui_visible(&self) -> bool {
        self.inner.ui_visible()
    }

    /// The shell reports user input while the window is visible (idle timer).
    pub fn ui_activity(&self) {
        self.inner.ui_activity();
    }

    /// Stores that could not be read at startup. Each message says whether a
    /// copy was kept or saving is blocked.
    pub fn load_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(problem) = locked(&self.inner.leaf_marks).load_problem() {
            out.push(problem.message().to_string());
        }
        if let Some(why) = locked(&self.inner.vault).read_problem() {
            out.push(format!(
                "Could not read the vault's file ({why}). It was left as it is. Unlocking tries it again."
            ));
        }
        {
            let workspace = locked(&self.inner.workspace);
            for problem in [workspace.load_problem(), workspace.grants_problem()]
                .into_iter()
                .flatten()
            {
                out.push(problem.message().to_string());
            }
        }
        if let Some(problem) = locked(&self.inner.holdings).load_problem() {
            out.push(problem.message().to_string());
        }
        if let Some(problem) = locked(&self.inner.notices).load_problem() {
            out.push(problem.message().to_string());
        }
        if let Some(problem) = locked(&self.inner.history).load_problem() {
            out.push(problem.message().to_string());
        }
        out
    }
}

impl Inner {
    pub(crate) fn slices(&self) -> &Slices {
        &self.slices
    }

    pub(crate) fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub(crate) fn ports(&self) -> &Ports {
        &self.ports
    }

    pub(crate) fn set_ui_visible(&self, visible: bool) {
        debug!(target: "airlock::app", visible, "ui visibility");
        self.visible.send_replace(visible);
    }

    pub(crate) fn ui_visible(&self) -> bool {
        *self.visible.borrow()
    }

    pub(crate) fn ui_activity(&self) {
        self.activity.send_replace(());
    }

    fn now_secs(&self) -> i64 {
        (self.ports.clock.now_ms() / 1000) as i64
    }
}

fn report_checked(store: &'static str, saved: &Result<(), VaultError>) {
    match saved {
        Ok(()) => {
            debug!(target: CORE, store, policy = "checked", result = "ok", "save");
            debug!(target: CORE, outcome = "ok", "command");
        }
        Err(err) => {
            warn!(
                target: CORE,
                store,
                policy = "checked",
                error = %err,
                "save failed; the change was rolled back"
            );
            debug!(target: CORE, outcome = "error", "command");
        }
    }
}

fn report_command(result: &Result<(), VaultError>) {
    let outcome = if result.is_ok() { "ok" } else { "error" };
    debug!(target: CORE, outcome, "command");
}
