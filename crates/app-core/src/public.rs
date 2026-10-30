//! The handle for background services.

use std::sync::Arc;

use solana_signature::Signature;
use tokio::sync::broadcast;
use wallet_core::{Holdings, Workspace};

use crate::bridge::{BridgeReply, BridgeRequest, SiteConnect, SiteSign};
use crate::commands::Inner;
use crate::events::Event;
use crate::notices::{BridgeRequestOutcome, NoticeId, NoticeKind, TxStatus};
use crate::ports::Ports;
use crate::slices::Slices;

/// What a sync job, the network monitor or the bridge server may do: read the
/// slices, listen for events, and run deferred commands.
///
/// Its type has no way to reach key material, so a service could not touch a
/// key even through a bug. Unlocking, locking, the vault's own commands and
/// signing are on [`Core`](crate::Core) only:
///
/// ```compile_fail,E0599
/// let kit = app_core::testing::kit();
/// let public = kit.core.public();
/// public.unlock("password").unwrap();
/// ```
///
/// ```compile_fail,E0599
/// let kit = app_core::testing::kit();
/// let public = kit.core.public();
/// public.with_vault(|vault| vault.is_unlocked());
/// ```
///
/// ```compile_fail,E0599
/// let kit = app_core::testing::kit();
/// let public = kit.core.public();
/// public.vault_checked("rename", |_| {}).unwrap();
/// ```
///
/// Nor can it answer a browser request: that is the user's approval.
///
/// ```compile_fail,E0599
/// let kit = app_core::testing::kit();
/// let public = kit.core.public();
/// public.bridge_answer("id", &app_core::bridge::BridgeAnswer::Rejected);
/// ```
///
/// What it can do:
///
/// ```no_run
/// # async fn run() {
/// let kit = app_core::testing::kit();
/// let public = kit.core.public();
/// let _book = public.slices().book.get();
/// public.holdings_deferred("clear", |holdings| *holdings = Default::default());
/// # }
/// ```
#[derive(Clone)]
pub struct PublicCore {
    inner: Arc<Inner>,
}

impl PublicCore {
    pub(crate) fn new(inner: Arc<Inner>) -> Self {
        Self { inner }
    }

    pub fn slices(&self) -> &Slices {
        self.inner.slices()
    }

    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.inner.events()
    }

    pub fn ports(&self) -> &Ports {
        self.inner.ports()
    }

    /// The shell reports whether the window is on screen.
    pub fn set_ui_visible(&self, visible: bool) {
        self.inner.set_ui_visible(visible);
    }

    /// Whether the shell last reported the window as visible.
    pub fn ui_visible(&self) -> bool {
        self.inner.ui_visible()
    }

    /// The shell reports user input while the window is visible.
    pub fn ui_activity(&self) {
        self.inner.ui_activity();
    }

    /// What the last probe of the node found.
    pub fn set_reach(&self, reach: crate::slices::Reach) {
        self.inner.set_reach(reach);
    }

    /// Publish the scheduler's per-job status for the chrome and panes.
    pub fn set_sync_status(&self, status: crate::slices::SyncStatus) {
        self.inner.set_sync_status(status);
    }

    pub fn workspace_deferred(&self, name: &'static str, f: impl FnOnce(&mut Workspace)) {
        self.inner.workspace_deferred(name, f);
    }

    pub fn holdings_deferred(&self, name: &'static str, f: impl FnOnce(&mut Holdings)) {
        self.inner.holdings_deferred(name, f);
    }

    pub fn notices_push(&self, kind: NoticeKind) -> NoticeId {
        self.inner.notices_push(kind)
    }

    pub fn notices_set_tx_status(&self, signature: Signature, status: TxStatus) -> bool {
        self.inner.notices_set_tx_status(signature, status)
    }

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

    pub fn notices_mark_read(&self, ids: &[NoticeId]) {
        self.inner.notices_mark_read(ids);
    }

    pub fn notices_mark_all_read(&self) {
        self.inner.notices_mark_all_read();
    }

    pub fn notices_remove(&self, id: NoticeId) {
        self.inner.notices_remove(id);
    }

    pub fn notices_clear(&self) {
        self.inner.notices_clear();
    }

    /// Record a sync job failure (notice + [`Event::SyncFailed`]).
    /// The scheduler calls this at most once per job until it succeeds again.
    pub fn report_sync_failed(&self, job: &str, error: &str) {
        self.inner.report_sync_failed(job, error);
    }

    /// Queue a browser request and send `RequestArrived`. `reply` sends the
    /// user's answer back to the site; the window answers through
    /// [`Core::bridge_answer`](crate::Core::bridge_answer), never a service.
    pub fn bridge_push(&self, request: BridgeRequest, reply: BridgeReply) {
        self.inner.bridge_push(request, reply);
    }

    /// What a `connect` from `origin` gets before anyone is asked. Only
    /// public keys the user already showed this site can come back.
    pub fn site_connect(&self, origin: &str, silent: bool) -> SiteConnect {
        self.inner.site_connect(origin, silent)
    }

    /// Whether `origin` may ask `accounts` for a signature. The user still
    /// approves the signature itself.
    pub fn site_sign(&self, origin: &str, accounts: &[solana_pubkey::Pubkey]) -> SiteSign {
        self.inner.site_sign(origin, accounts)
    }

    /// The site disconnected. A remembered site keeps its grant.
    pub fn site_disconnected(&self, origin: &str) {
        self.inner.site_disconnected(origin);
    }

    /// The origins with a page open, across every browser.
    pub fn site_pages(&self, pages: std::collections::BTreeSet<String>) {
        self.inner.site_pages(pages);
    }

    /// The labels of `keys`, to tell a site what it holds.
    pub fn site_labels(&self, keys: Vec<solana_pubkey::Pubkey>) -> Vec<([u8; 32], String)> {
        self.inner.site_labels(keys)
    }

    /// The wallets changed: tell the sites that hold accounts what they hold
    /// now. A site that was not given the current wallet keeps what it has.
    pub fn sites_sync(&self) {
        self.inner.sites_sync();
    }

    /// Drop a request nobody answered in time. Its reply is dropped unsent.
    /// Answers whether it was waiting.
    pub fn bridge_expire(&self, id: &str) -> bool {
        self.inner.bridge_expire(id)
    }
}
