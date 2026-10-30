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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bridge_protocol::reply::SiteAccount;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use tokio::sync::{broadcast, watch};
use tracing::{debug, debug_span, info, warn};
use wallet_core::{
    connect_choice, current_first, reconnect_accounts, AddressBook, EnrollError, Holdings,
    RosterError, ShareableAccount, SiteGrant, Vault, VaultData, VaultError, WalletId, Workspace,
};

use crate::bridge::{
    BridgeAnswer, BridgeReply, BridgeRequest, BridgeState, ConnectedSite, SiteChoice, SiteConnect,
    SiteSign, SitesView, Taken,
};
use crate::diagnostics::log_status;
use crate::events::Event;
use crate::lock::{LockPolicy, LockReason};
use crate::notices::{BridgeRequestOutcome, NoticeId, NoticeKind, TxStatus};
use crate::ports::Ports;
use crate::public::PublicCore;
use crate::slices::{
    book_from, holdings_from, network_from, notices_from, settings_from, shown_wallets,
    vault_state_from, wallets_view_from, Reach, Slice, Slices,
};
use crate::storage::{
    open_holdings_in, open_notices_in, open_vault_in, open_wallet_list_in, open_workspace_in,
    AppHoldings, AppNotices, AppVault, AppWalletList, AppWorkspace,
};

/// How often a deferred store is written at most.
pub const SAVE_INTERVAL: Duration = Duration::from_secs(1);

const EVENT_BACKLOG: usize = 64;
const CORE: &str = "airlock::core";
const VAULT: &str = "airlock::vault";

#[derive(Clone, Copy)]
enum Store {
    Workspace,
    Holdings,
    Notices,
}

impl Store {
    fn name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Holdings => "holdings",
            Self::Notices => "notices",
        }
    }
}

/// Where a deferred store stands.
#[derive(Default)]
struct Saver {
    /// Memory holds something the store does not.
    dirty: AtomicBool,
    /// A task is waiting to write it.
    scheduled: AtomicBool,
}

/// The whole core, with the vault's secrets. Shells that start things hold
/// this; services get [`PublicCore`].
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    ports: Ports,
    vault: Mutex<AppVault>,
    /// Always taken after `vault`, and before `workspace`.
    wallet_list: Mutex<AppWalletList>,
    workspace: Mutex<AppWorkspace>,
    holdings: Mutex<AppHoldings>,
    notices: Mutex<AppNotices>,
    bridge: Mutex<BridgeState>,
    slices: Slices,
    events: broadcast::Sender<Event>,
    visible: watch::Sender<bool>,
    activity: watch::Sender<()>,
    reach: Mutex<Reach>,
    workspace_saver: Saver,
    holdings_saver: Saver,
    notices_saver: Saver,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not stop a save or a lock.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Core {
    /// Open the stores and start the auto-lock. Runs on `ports.spawner`, so a
    /// native shell calls this inside its tokio runtime.
    pub fn open(ports: Ports) -> Core {
        let vault = open_vault_in(&*ports.storage);
        let wallet_list = open_wallet_list_in(&*ports.storage);
        let workspace = open_workspace_in(&*ports.storage);
        let holdings = open_holdings_in(&*ports.storage);
        let mut notices = open_notices_in(&*ports.storage);
        if let Some(problem) = workspace.load_problem() {
            warn!(target: CORE, store = "workspace", problem = %problem.message(), "load problem");
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
            wallets: Slice::new(wallets_view_from(shown_wallets(&vault, &wallet_list))),
            book: Slice::new(book_from(&workspace)),
            holdings: Slice::new(holdings_from(&holdings)),
            network: Slice::new(network_from(&workspace, Reach::default())),
            vault: Slice::new(Arc::new(vault_state_from(&vault))),
            settings: Slice::new(settings),
            bridge: Slice::new(Arc::default()),
            sites: Slice::new(Arc::new(sites_view(&workspace, &BridgeState::default()))),
            notices: Slice::new(notices_from(&notices)),
            sync: Slice::new(Arc::new(crate::slices::SyncStatus::default())),
        };
        let inner = Arc::new(Inner {
            ports,
            vault: Mutex::new(vault),
            wallet_list: Mutex::new(wallet_list),
            workspace: Mutex::new(workspace),
            holdings: Mutex::new(holdings),
            notices: Mutex::new(notices),
            bridge: Mutex::new(BridgeState::default()),
            slices,
            events: broadcast::channel(EVENT_BACKLOG).0,
            visible: watch::channel(true).0,
            activity: watch::channel(()).0,
            reach: Mutex::new(Reach::default()),
            workspace_saver: Saver::default(),
            holdings_saver: Saver::default(),
            notices_saver: Saver::default(),
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

    /// Reply to a browser request with what the user decided. Only here, not
    /// on [`PublicCore`]: an answer is the user's approval. Answers whether
    /// the request was waiting and `answer` fits its method.
    pub fn bridge_answer(&self, id: &str, answer: &BridgeAnswer) -> bool {
        self.inner.bridge_answer(id, answer)
    }

    /// Answer a `connect` with the accounts the user chose, the current
    /// wallet moved first, and keep the choice: saved when `remember`, for as
    /// long as the app is open otherwise. Answers whether it was waiting.
    pub fn bridge_connect(
        &self,
        id: &str,
        accounts: Vec<ShareableAccount>,
        remember: bool,
    ) -> bool {
        self.inner.bridge_connect(id, accounts, remember)
    }

    /// Answer a `signIn` with what `account` signed, and let the site see it,
    /// with what it could already: saved when `remember`, for as long as the
    /// app is open otherwise. Answers whether it was waiting.
    pub fn bridge_sign_in(
        &self,
        id: &str,
        account: Pubkey,
        signed: Vec<u8>,
        signature: [u8; 64],
        remember: bool,
    ) -> bool {
        self.inner
            .bridge_sign_in(id, account, signed, signature, remember)
    }

    /// Turn away the site that made request `id` from now on, and reject
    /// everything it has waiting. Answers whether it was waiting.
    pub fn bridge_block(&self, id: &str) -> bool {
        self.inner.bridge_block(id)
    }

    /// End `origin`'s connection. A remembered site keeps its grant and is
    /// let back in when it next asks to connect.
    pub fn site_disconnect(&self, origin: &str) {
        self.inner.site_disconnect(origin);
    }

    /// Forget what `origin` was given and disconnect it.
    pub fn site_revoke(&self, origin: &str) {
        self.inner.site_revoke(origin);
    }

    /// Turn `origin` away from now on, and reject what it has waiting.
    pub fn site_block(&self, origin: &str) {
        self.inner.site_block(origin);
    }

    pub fn site_unblock(&self, origin: &str) {
        self.inner.site_unblock(origin);
    }

    /// What the connect modal opens with for `origin`.
    pub fn site_choice(&self, origin: &str) -> SiteChoice {
        self.inner.site_choice(origin)
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

    // The vault. Only here, never on `PublicCore`.

    /// Start a vault under `password`.
    pub fn create_vault(&self, password: &str) -> Result<(), VaultError> {
        self.inner.create_vault(password)
    }

    pub fn unlock(&self, password: &str) -> Result<(), VaultError> {
        self.inner.unlock(password)
    }

    /// Seal the unlocked vault under `new`, once `current` opens it.
    pub fn change_password(&self, current: &str, new: &str) -> Result<(), VaultError> {
        self.inner.change_password(current, new)
    }

    /// Lock now, as the user asked.
    pub fn lock(&self) {
        self.inner.lock(LockReason::User);
    }

    /// Change what the vault holds and save it now. A failed save leaves the
    /// vault as it was.
    pub fn vault_checked(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut VaultData),
    ) -> Result<(), VaultError> {
        self.inner.vault_checked(name, f)
    }

    /// Read the vault, keys included, for signing. The secrets stay inside
    /// the closure's borrow.
    pub fn with_vault<R>(&self, f: impl FnOnce(&AppVault) -> R) -> R {
        f(&locked(&self.inner.vault))
    }

    /// Read the wallets the screens show, with no secret in reach: the
    /// vault's while it is unlocked, the copy kept without secrets while it
    /// is locked, `None` before a vault exists. Nothing read here signs.
    pub fn with_wallets<R>(&self, f: impl FnOnce(Option<&VaultData>) -> R) -> R {
        let vault = locked(&self.inner.vault);
        let list = locked(&self.inner.wallet_list);
        f(shown_wallets(&vault, &list))
    }

    /// Act as `id` from now on. While the vault is locked the choice is kept
    /// with the copy of the wallets, and the vault takes it when unlocked.
    pub fn select_wallet(&self, id: WalletId) -> Result<(), VaultError> {
        self.inner.select_wallet(id)
    }

    /// Read the vault mutably, for the one job that must reserve a signing
    /// leaf inside it before anything is signed. Publishes the slices after.
    pub fn with_vault_mut<R>(&self, f: impl FnOnce(&mut AppVault) -> R) -> R {
        let mut vault = locked(&self.inner.vault);
        let out = f(&mut vault);
        self.inner.publish_vault(&vault);
        out
    }

    /// How long the vault stays unlocked while hidden or idle. Saved now.
    pub fn set_auto_lock(&self, timeout: Duration) -> Result<(), VaultError> {
        self.inner.set_auto_lock(timeout)
    }

    /// Whether closing the window should hide it (tray) instead of quitting.
    pub fn set_close_to_tray(&self, close_to_tray: bool) -> Result<(), VaultError> {
        self.inner.set_close_to_tray(close_to_tray)
    }

    /// Read the public workspace: what the published slices do not carry,
    /// such as the custom cluster last typed.
    pub fn with_workspace<R>(&self, f: impl FnOnce(&Workspace) -> R) -> R {
        f(locked(&self.inner.workspace).data())
    }

    /// Add to the vault what `add` decides, and let `name` write it in the
    /// address book. Both are saved now; nothing is kept if either save fails.
    pub fn enroll<T>(
        &self,
        name: &'static str,
        add: impl FnOnce(&mut VaultData) -> Result<T, RosterError>,
        label: impl FnOnce(&mut AddressBook, &T),
    ) -> Result<T, EnrollError> {
        self.inner.enroll(name, add, label)
    }

    /// Stores that could not be read at startup. Each message says whether a
    /// copy was kept or saving is blocked.
    pub fn load_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(problem) = locked(&self.inner.workspace).load_problem() {
            out.push(problem.message().to_string());
        }
        if let Some(problem) = locked(&self.inner.holdings).load_problem() {
            out.push(problem.message().to_string());
        }
        if let Some(problem) = locked(&self.inner.notices).load_problem() {
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

    // Publishing. Each runs with its store's lock held, so two threads cannot
    // publish out of order.

    fn publish_workspace(&self, workspace: &AppWorkspace) {
        let slices = &self.slices;
        slices.book.publish(book_from(workspace));
        slices
            .network
            .publish(network_from(workspace, *locked(&self.reach)));
        slices.settings.publish(settings_from(workspace));
    }

    fn publish_holdings(&self, holdings: &AppHoldings) {
        self.slices.holdings.publish(holdings_from(holdings));
    }

    fn publish_notices(&self, notices: &AppNotices) {
        self.slices.notices.publish(notices_from(notices));
    }

    /// Publish the vault, and while it is open keep its wallets without
    /// their secrets for when it is locked.
    fn publish_vault(&self, vault: &AppVault) {
        let mut list = locked(&self.wallet_list);
        if let Ok(data) = Vault::data(vault) {
            if let Err(err) = list.keep(data) {
                warn!(
                    target: CORE,
                    store = "wallets",
                    error = %err,
                    "save failed; the wallets shown while locked are not current"
                );
                let _ = self.events.send(Event::SaveFailed {
                    store: "wallets",
                    error: err.to_string(),
                });
            }
        }
        self.slices
            .wallets
            .publish(wallets_view_from(shown_wallets(vault, &list)));
        self.slices.vault.publish(Arc::new(vault_state_from(vault)));
    }

    fn now_secs(&self) -> i64 {
        (self.ports.clock.now_ms() / 1000) as i64
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

    pub(crate) fn set_reach(&self, reach: Reach) {
        *locked(&self.reach) = reach;
        let workspace = locked(&self.workspace);
        self.publish_workspace(&workspace);
    }

    pub(crate) fn set_sync_status(&self, status: crate::slices::SyncStatus) {
        self.slices.sync.publish(Arc::new(status));
    }

    fn set_auto_lock(&self, timeout: Duration) -> Result<(), VaultError> {
        let secs = timeout.as_secs().max(1);
        self.workspace_checked("set_auto_lock", |ws| {
            ws.auto_lock_secs = secs;
        })
    }

    fn set_close_to_tray(&self, close_to_tray: bool) -> Result<(), VaultError> {
        self.workspace_checked("set_close_to_tray", |ws| {
            ws.close_to_tray = close_to_tray;
        })
    }

    pub(crate) fn bridge_push(&self, request: BridgeRequest, reply: BridgeReply) {
        let _span = debug_span!(target: CORE, "command", name = "bridge_push").entered();
        let id = request.id.clone();
        {
            let mut bridge = locked(&self.bridge);
            bridge.push(request, reply);
            self.slices.bridge.publish(Arc::new(bridge.queue().clone()));
        }
        let _ = self.events.send(Event::RequestArrived { id });
        debug!(target: CORE, outcome = "ok", "command");
    }

    /// Reply to `id` with what the user decided. Answers whether the request
    /// was waiting and `answer` fits its method.
    pub(crate) fn bridge_answer(self: &Arc<Self>, id: &str, answer: &BridgeAnswer) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_answer").entered();
        let Some(taken) = self.bridge_take(id, Some(answer)) else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        let (bytes, outcome) = answer.reply(id);
        if let Some(reply) = taken.reply {
            reply(bytes);
        }
        self.bridge_finished(&taken.request, outcome);
        true
    }

    /// Drop `id` unanswered: the transport stopped waiting. Answers whether
    /// it was waiting.
    pub(crate) fn bridge_expire(self: &Arc<Self>, id: &str) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_expire").entered();
        let Some(taken) = self.bridge_take(id, None) else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        self.bridge_finished(&taken.request, BridgeRequestOutcome::Expired);
        true
    }

    /// The accounts a site can be shown, and the current wallet's address.
    fn site_accounts(&self) -> (Vec<ShareableAccount>, Option<Pubkey>) {
        let vault = locked(&self.vault);
        let list = locked(&self.wallet_list);
        let data = shown_wallets(&vault, &list);
        let current = data.and_then(VaultData::acting_address);
        let workspace = locked(&self.workspace);
        (
            wallet_core::shareable_accounts(data, workspace.data()),
            current,
        )
    }

    /// What `origin` was given, remembered or for this run of the app.
    fn site_grant(&self, origin: &str) -> Option<SiteGrant> {
        let remembered = locked(&self.workspace)
            .data()
            .connections
            .grant(origin)
            .cloned();
        remembered.or_else(|| locked(&self.bridge).session(origin).cloned())
    }

    fn site_is_blocked(&self, origin: &str) -> bool {
        locked(&self.workspace)
            .data()
            .connections
            .is_blocked(origin)
    }

    pub(crate) fn site_connect(&self, origin: &str, silent: bool) -> SiteConnect {
        if self.site_is_blocked(origin) {
            return SiteConnect::Blocked;
        }
        let Some(grant) = self.site_grant(origin) else {
            return SiteConnect::Undecided;
        };
        // A site that disconnected stays out until it asks again.
        if grant.ended && silent {
            return SiteConnect::Undecided;
        }
        let (shareable, current) = self.site_accounts();
        let keys: Vec<Pubkey> = shareable.iter().map(|account| account.pubkey).collect();
        let Some(accounts) = reconnect_accounts(&grant.accounts, current, &keys) else {
            return SiteConnect::Undecided;
        };
        if grant.ended {
            self.set_site_ended(origin, false);
        }
        info!(target: "airlock::bridge", origin = %origin, silent, "site reconnected");
        locked(&self.bridge).set_holding(origin, accounts.clone());
        self.publish_sites();
        SiteConnect::Allowed(labelled(accounts, &shareable))
    }

    pub(crate) fn site_sign(&self, origin: &str, accounts: &[Pubkey]) -> SiteSign {
        if self.site_is_blocked(origin) {
            return SiteSign::Blocked;
        }
        match self.site_grant(origin) {
            Some(grant)
                if !grant.ended && accounts.iter().all(|key| grant.accounts.contains(key)) =>
            {
                // A page from before the app restarted still holds its accounts.
                let mut bridge = locked(&self.bridge);
                if !bridge.holding().contains_key(origin) {
                    bridge.set_holding(origin, grant.accounts);
                    drop(bridge);
                    self.publish_sites();
                }
                SiteSign::Allowed
            }
            _ => SiteSign::NotConnected,
        }
    }

    pub(crate) fn site_disconnected(&self, origin: &str) {
        self.end_site(origin);
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site disconnected");
    }

    pub(crate) fn site_disconnect(self: &Arc<Self>, origin: &str) {
        self.end_site(origin);
        self.reject_waiting(origin, "signTransaction");
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site disconnected by the user");
    }

    pub(crate) fn site_revoke(self: &Arc<Self>, origin: &str) {
        locked(&self.bridge).end_session(origin);
        locked(&self.bridge).drop_holding(origin);
        let saved = self.workspace_checked("site_revoke", |workspace| {
            workspace.connections.forget(origin);
        });
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
        self.reject_waiting(origin, "signTransaction");
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site revoked");
    }

    pub(crate) fn site_block(self: &Arc<Self>, origin: &str) {
        let saved = self.workspace_checked("site_block", |workspace| {
            workspace.connections.block(origin);
        });
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
        locked(&self.bridge).end_session(origin);
        locked(&self.bridge).drop_holding(origin);
        self.reject_waiting(origin, "");
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site blocked");
    }

    pub(crate) fn site_unblock(&self, origin: &str) {
        let saved = self.workspace_checked("site_unblock", |workspace| {
            workspace
                .connections
                .blocked
                .retain(|known| known != origin);
        });
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
        self.publish_sites();
        info!(target: "airlock::bridge", origin = %origin, "site unblocked");
    }

    /// Reject what `origin` has waiting: requests of `method`, or any when empty.
    fn reject_waiting(self: &Arc<Self>, origin: &str, method: &str) {
        let waiting = locked(&self.bridge).waiting_from(origin, method);
        for id in waiting {
            self.bridge_answer(&id, &BridgeAnswer::Rejected);
        }
    }

    pub(crate) fn site_pages(&self, pages: std::collections::BTreeSet<String>) {
        locked(&self.bridge).set_pages(pages);
        self.publish_sites();
    }

    pub(crate) fn sites_sync(&self) {
        let (shareable, current) = self.site_accounts();
        let keys: Vec<Pubkey> = shareable.iter().map(|account| account.pubkey).collect();
        let holding = locked(&self.bridge).holding().clone();
        for (origin, held) in holding {
            let granted = self
                .site_grant(&origin)
                .map(|grant| grant.accounts)
                .unwrap_or_default();
            // A wallet the site was not given changes nothing, but one no
            // longer on this device cannot stay with it.
            let accounts = reconnect_accounts(&granted, current, &keys).unwrap_or_else(|| {
                held.iter()
                    .filter(|key| keys.contains(key))
                    .copied()
                    .collect()
            });
            if accounts != held {
                locked(&self.bridge).set_holding(&origin, accounts);
            }
        }
        self.publish_sites();
    }

    /// The labels of `keys`, for the bridge to tell a site.
    pub(crate) fn site_labels(&self, keys: Vec<Pubkey>) -> Vec<SiteAccount> {
        let (shareable, _) = self.site_accounts();
        labelled(keys, &shareable)
    }

    fn publish_sites(&self) {
        let workspace = locked(&self.workspace);
        let bridge = locked(&self.bridge);
        let view = sites_view(&workspace, &bridge);
        self.slices.sites.publish(Arc::new(view));
    }

    /// Take `origin` off what it holds. A remembered site keeps its grant.
    fn end_site(&self, origin: &str) {
        locked(&self.bridge).end_session(origin);
        locked(&self.bridge).drop_holding(origin);
        let remembered = locked(&self.workspace)
            .data()
            .connections
            .grant(origin)
            .is_some();
        if remembered {
            self.set_site_ended(origin, true);
        }
    }

    /// Saved now rather than deferred: this runs on the bridge's own thread,
    /// which cannot start the saver task.
    fn set_site_ended(&self, origin: &str, ended: bool) {
        let saved = self.workspace_checked("site_ended", |workspace| {
            workspace.connections.set_ended(origin, ended);
        });
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
    }

    pub(crate) fn site_choice(&self, origin: &str) -> SiteChoice {
        let (shareable, current) = self.site_accounts();
        let keys: Vec<Pubkey> = shareable.iter().map(|account| account.pubkey).collect();
        let grant = self.site_grant(origin);
        let remembered = locked(&self.workspace)
            .data()
            .connections
            .grant(origin)
            .is_some();
        SiteChoice {
            choice: connect_choice(
                &keys,
                current,
                grant.as_ref().map(|grant| grant.accounts.as_slice()),
            ),
            remembered,
        }
    }

    pub(crate) fn bridge_connect(
        self: &Arc<Self>,
        id: &str,
        accounts: Vec<ShareableAccount>,
        remember: bool,
    ) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_connect").entered();
        let (_, current) = self.site_accounts();
        let keys: Vec<Pubkey> = accounts.iter().map(|account| account.pubkey).collect();
        let ordered = current_first(keys, current);
        let answer = BridgeAnswer::Connect(labelled(ordered.clone(), &accounts));
        let Some(taken) = self.bridge_take(id, Some(&answer)) else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        let origin = taken.request.origin.clone();
        // A request with no origin has nothing a later one could match.
        if !origin.is_empty() {
            locked(&self.bridge).set_holding(&origin, ordered.clone());
            self.keep_site(&origin, ordered, remember);
            self.publish_sites();
        }
        let (bytes, outcome) = answer.reply(id);
        if let Some(reply) = taken.reply {
            reply(bytes);
        }
        self.bridge_finished(&taken.request, outcome);
        true
    }

    pub(crate) fn bridge_sign_in(
        self: &Arc<Self>,
        id: &str,
        account: Pubkey,
        signed: Vec<u8>,
        signature: [u8; 64],
        remember: bool,
    ) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_sign_in").entered();
        let Some(origin) = locked(&self.bridge)
            .queue()
            .get(id)
            .map(|request| request.origin.clone())
        else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        let (shareable, _) = self.site_accounts();
        // The account that signed in comes first: it is who the site now sees.
        let mut keys = vec![account];
        if let Some(grant) = self.site_grant(&origin) {
            keys.extend(grant.accounts.into_iter().filter(|key| *key != account));
        }
        let answer = BridgeAnswer::SignedIn {
            account: account.to_bytes(),
            accounts: labelled(keys.clone(), &shareable),
            signed,
            signature,
        };
        let Some(taken) = self.bridge_take(id, Some(&answer)) else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        if !origin.is_empty() {
            locked(&self.bridge).set_holding(&origin, keys.clone());
            self.keep_site(&origin, keys, remember);
            self.publish_sites();
        }
        let (bytes, outcome) = answer.reply(id);
        if let Some(reply) = taken.reply {
            reply(bytes);
        }
        self.bridge_finished(&taken.request, outcome);
        true
    }

    /// Keep what `origin` was given. When it cannot be saved the site still
    /// has it until the app closes, and the failure is reported.
    fn keep_site(&self, origin: &str, accounts: Vec<Pubkey>, remember: bool) {
        let was_remembered = locked(&self.workspace)
            .data()
            .connections
            .grant(origin)
            .is_some();
        let saved = if remember {
            let kept = accounts.clone();
            self.workspace_checked("site_remember", move |workspace| {
                workspace.connections.remember(origin, kept);
            })
        } else if was_remembered {
            self.workspace_checked("site_forget", |workspace| {
                workspace.connections.forget(origin);
            })
        } else {
            Ok(())
        };
        let mut bridge = locked(&self.bridge);
        bridge.end_session(origin);
        if let Err(err) = &saved {
            self.report_site_save(err);
        }
        if !remember || saved.is_err() {
            bridge.set_session(origin, accounts);
        }
    }

    fn report_site_save(&self, err: &VaultError) {
        let _ = self.events.send(Event::SaveFailed {
            store: Store::Workspace.name(),
            error: err.to_string(),
        });
    }

    pub(crate) fn bridge_block(self: &Arc<Self>, id: &str) -> bool {
        let _span = debug_span!(target: CORE, "command", name = "bridge_block").entered();
        let Some(origin) = locked(&self.bridge)
            .queue()
            .get(id)
            .map(|request| request.origin.clone())
        else {
            debug!(target: CORE, outcome = "not waiting", "command");
            return false;
        };
        if origin.is_empty() {
            self.bridge_answer(id, &BridgeAnswer::Rejected);
        } else {
            self.site_block(&origin);
        }
        true
    }

    fn bridge_take(&self, id: &str, answer: Option<&BridgeAnswer>) -> Option<Taken> {
        let mut bridge = locked(&self.bridge);
        let taken = bridge.take(id, answer)?;
        self.slices.bridge.publish(Arc::new(bridge.queue().clone()));
        Some(taken)
    }

    fn bridge_finished(self: &Arc<Self>, request: &BridgeRequest, outcome: BridgeRequestOutcome) {
        info!(
            target: "airlock::bridge",
            code = %request.code(),
            method = %request.method,
            origin = %request.origin,
            outcome = outcome.as_str(),
            "request finished"
        );
        self.notices_set_bridge_outcome(&request.id, &request.origin, &request.method, outcome);
        debug!(target: CORE, outcome = "ok", "command");
    }

    // The vault.

    fn create_vault(&self, password: &str) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name = "create_vault").entered();
        let mut vault = locked(&self.vault);
        let created = vault.create(password);
        if created.is_ok() {
            info!(target: VAULT, "created");
            self.publish_vault(&vault);
        }
        report_command(&created);
        created
    }

    fn unlock(&self, password: &str) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name = "unlock").entered();
        let mut vault = locked(&self.vault);
        let unlocked = vault.unlock(password);
        if unlocked.is_ok() {
            info!(target: VAULT, "unlocked");
            self.adopt_locked_choice(&mut vault);
            self.publish_vault(&vault);
        }
        report_command(&unlocked);
        unlocked
    }

    fn change_password(&self, current: &str, new: &str) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name = "change_password").entered();
        let changed = locked(&self.vault).change_password(current, new);
        if changed.is_ok() {
            info!(target: VAULT, "password changed");
        }
        report_command(&changed);
        changed
    }

    /// Forget the secrets. Does nothing, and says nothing, when the vault is
    /// not unlocked.
    pub(crate) fn lock(self: &Arc<Self>, reason: LockReason) {
        let _span = debug_span!(target: CORE, "command", name = "lock").entered();
        {
            let mut vault = locked(&self.vault);
            if !vault.is_unlocked() {
                return;
            }
            vault.lock();
            self.publish_vault(&vault);
        }
        info!(target: VAULT, reason = %reason, "locked");
        let _ = self.events.send(Event::VaultLocked { reason });
        let _ = self.notices_push(NoticeKind::VaultLocked { reason });
    }

    /// Take the wallet chosen while the vault was locked, when it is still
    /// one of the vault's.
    fn adopt_locked_choice(&self, vault: &mut AppVault) {
        let chosen = locked(&self.wallet_list)
            .data()
            .and_then(|list| list.selected);
        let Some(id) = chosen else {
            return;
        };
        let Ok(data) = Vault::data(vault) else {
            return;
        };
        if data.selected == Some(id) || !data.wallets.iter().any(|wallet| wallet.id == id) {
            return;
        }
        if let Err(err) = vault.change(|data| data.selected = Some(id)) {
            warn!(target: VAULT, error = %err, "could not keep the wallet chosen while locked");
        }
    }

    fn select_wallet(&self, id: WalletId) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name = "select_wallet").entered();
        let mut vault = locked(&self.vault);
        let saved = if vault.is_unlocked() {
            vault.change(|data| data.selected = Some(id))
        } else {
            locked(&self.wallet_list).select(id)
        };
        if saved.is_ok() {
            self.publish_vault(&vault);
        }
        report_checked("vault", &saved);
        saved
    }

    fn enroll<T>(
        &self,
        name: &'static str,
        add: impl FnOnce(&mut VaultData) -> Result<T, RosterError>,
        label: impl FnOnce(&mut AddressBook, &T),
    ) -> Result<T, EnrollError> {
        let _span = debug_span!(target: CORE, "command", name).entered();
        // Always vault first, then workspace.
        let mut vault = locked(&self.vault);
        let mut workspace = locked(&self.workspace);
        let enrolled = wallet_core::enroll(&mut *vault, &mut *workspace, add, label);
        match &enrolled {
            Ok(_) => {
                self.workspace_saver.dirty.store(false, Ordering::SeqCst);
                self.publish_vault(&vault);
                self.publish_workspace(&workspace);
                debug!(target: CORE, outcome = "ok", "command");
            }
            Err(err) => {
                debug!(target: CORE, outcome = "error", error = %err, "command");
            }
        }
        enrolled
    }

    fn vault_checked(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut VaultData),
    ) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name).entered();
        let mut vault = locked(&self.vault);
        let saved = vault.change(f);
        if saved.is_ok() {
            self.publish_vault(&vault);
        }
        report_checked("vault", &saved);
        saved
    }

    // Deferred saving.

    fn saver(&self, store: Store) -> &Saver {
        match store {
            Store::Workspace => &self.workspace_saver,
            Store::Holdings => &self.holdings_saver,
            Store::Notices => &self.notices_saver,
        }
    }

    /// Note that `store` is ahead of its file, and make sure a task will write
    /// it. Changes that arrive before the task wakes ride along.
    fn mark_dirty(self: &Arc<Self>, store: Store) {
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
        let saved = match store {
            Store::Workspace => locked(&self.workspace).save(),
            Store::Holdings => locked(&self.holdings).save(),
            Store::Notices => locked(&self.notices).save(),
        };
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

    fn flush_all(self: &Arc<Self>) {
        self.write_if_dirty(Store::Workspace);
        self.write_if_dirty(Store::Holdings);
        self.write_if_dirty(Store::Notices);
    }

    /// Flush without Arc (Drop). Save failures still emit events, not notices.
    fn flush_all_dropping(&self) {
        for store in [Store::Workspace, Store::Holdings, Store::Notices] {
            let saver = self.saver(store);
            saver.scheduled.store(false, Ordering::SeqCst);
            if !saver.dirty.swap(false, Ordering::SeqCst) {
                continue;
            }
            let saved = match store {
                Store::Workspace => locked(&self.workspace).save(),
                Store::Holdings => locked(&self.holdings).save(),
                Store::Notices => locked(&self.notices).save(),
            };
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

fn sites_view(workspace: &AppWorkspace, bridge: &BridgeState) -> SitesView {
    let connections = &workspace.data().connections;
    SitesView {
        connected: bridge
            .connected()
            .map(|(origin, accounts)| ConnectedSite {
                origin: origin.clone(),
                accounts: accounts.clone(),
                remembered: connections.grant(origin).is_some(),
            })
            .collect(),
        authorized: connections.sites.clone(),
        blocked: connections.blocked.clone(),
        holding: bridge.holding().clone(),
    }
}

/// `keys` as a site is sent them, each with the label it has here.
fn labelled(keys: Vec<Pubkey>, accounts: &[ShareableAccount]) -> Vec<SiteAccount> {
    keys.into_iter()
        .map(|key| {
            let known = accounts.iter().find(|account| account.pubkey == key);
            SiteAccount {
                pubkey: key.to_bytes(),
                label: known
                    .map(|account| account.label.clone())
                    .unwrap_or_default(),
                signs_messages: known.is_some_and(|account| !account.contract),
            }
        })
        .collect()
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
