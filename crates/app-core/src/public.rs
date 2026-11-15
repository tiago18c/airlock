//! The handle for background services.

use std::sync::Arc;

use solana_signature::Signature;
use tokio::sync::broadcast;
use wallet_core::intent::{
    ProfileError, Resolution, ResolutionFailure, SendResult, SigningProfile,
};
use wallet_core::{Holdings, Workspace};

use crate::bridge::{BridgeRefused, BridgeReply, BridgeRequest, SiteConnect, SiteSign};
use crate::commands::Inner;
use crate::events::Event;
use crate::notices::{BridgeRequestOutcome, NoticeId, NoticeKind, TxStatus};
use crate::ports::Ports;
use crate::resolutions::{ResolutionCommandError, ResolutionRefused};
use crate::slices::Slices;

/// What a sync job, the network monitor or the bridge server may do: read the
/// slices, listen for events, and run deferred commands. [`Core`](crate::Core)
/// dereferences to it, so the window has all of this too.
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
/// ```compile_fail,E0599
/// let kit = app_core::testing::kit();
/// let public = kit.core.public();
/// let _key = public.vault_key_bytes();
/// ```
///
/// ```compile_fail,E0599
/// let kit = app_core::testing::kit();
/// let public = kit.core.public();
/// public.with_vault_and_marks(|_vault, _marks| ());
/// ```
///
/// Nor can it be made back into the [`Core`](crate::Core) it came from:
///
/// ```compile_fail,E0277
/// let kit = app_core::testing::kit();
/// let public = kit.core.public();
/// let _core: app_core::Core = public.into();
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

    /// The shell reports user input while the window is visible (idle timer).
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

    /// Change the transaction history. `f` says whether it changed anything.
    pub fn history_deferred(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut crate::history::History) -> bool,
    ) {
        self.inner.history_deferred(name, f);
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

    /// Record a sync job failure (notice + [`Event::SyncFailed`]).
    /// The scheduler calls this at most once per job until it succeeds again.
    pub fn report_sync_failed(&self, job: &str, error: &str) {
        self.inner.report_sync_failed(job, error);
    }

    /// Queue a browser request and send `RequestArrived`. `reply` sends the
    /// user's answer back to the site; the window answers through
    /// [`Core::bridge_answer`](crate::Core::bridge_answer), never a service.
    pub fn bridge_push(
        &self,
        request: BridgeRequest,
        reply: BridgeReply,
    ) -> Result<(), BridgeRefused> {
        self.inner.bridge_push(request, reply)
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
    pub fn site_labels(
        &self,
        keys: Vec<solana_pubkey::Pubkey>,
    ) -> Vec<bridge_protocol::reply::SiteAccount> {
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

    // Resolutions (`docs/intents.md`). Each belongs to the origin that made
    // it, and is found by that origin and its id: another origin's lookup
    // finds nothing. Each change is saved deferred and published in
    // `slices().resolutions`.

    /// How `account`, a wallet of this device, signs, and the payer of its
    /// signing route. Read without the vault's secrets, locked or not.
    pub fn resolution_profile(
        &self,
        account: solana_pubkey::Pubkey,
    ) -> Result<(SigningProfile, Option<solana_pubkey::Pubkey>), ProfileError> {
        self.inner.resolution_profile(account)
    }

    /// What `cluster` said of `multisig` (threshold, members and their
    /// permissions, time lock), as the Proposals pane read it. A site asking
    /// how its vault signs, while the app is on that cluster, is answered
    /// from this rather than from the wallet record. Kept in memory until
    /// the app closes.
    pub fn squad_read(
        &self,
        multisig: solana_pubkey::Pubkey,
        cluster: wallet_core::Cluster,
        state: wallet_core::squads::SquadState,
    ) {
        self.inner.squad_read(multisig, cluster, state);
    }

    /// Keep `resolution`, in `awaiting-approval`, for the origin of
    /// `request`, a [`SiteCall::Resolve`](crate::bridge::SiteCall::Resolve),
    /// and queue `request` for the user: both or neither. Nobody waits on a
    /// reply to it; what the user decides goes to the resolution. It expires
    /// once its approval window passes with nobody having decided.
    pub fn resolution_create(
        &self,
        request: BridgeRequest,
        resolution: Resolution,
    ) -> Result<Resolution, ResolutionRefused> {
        self.inner.resolution_create(request, resolution)
    }

    /// Resolution `id` of `origin`, as kept, with the wires it holds.
    pub fn resolution(&self, origin: &str, id: &str) -> Option<Resolution> {
        self.inner.resolution(origin, id)
    }

    /// The site cancelled: in `awaiting-approval`, which also takes its
    /// request out of the queue, or in `awaiting-app`.
    pub fn resolution_cancel(
        &self,
        origin: &str,
        id: &str,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.inner.resolution_cancel(origin, id)
    }

    /// Nobody decided within the approval window. Only in
    /// `awaiting-approval`; its request leaves the queue.
    pub fn resolution_expire(
        &self,
        origin: &str,
        id: &str,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.inner.resolution_expire(origin, id)
    }

    /// It cannot go on, for `failure`, before anything was sent: a check of
    /// the notes that needed a node or the lookup tables, a rewrite that
    /// failed. In `awaiting-approval` its request leaves the queue.
    pub fn resolution_fail(
        &self,
        origin: &str,
        id: &str,
        failure: ResolutionFailure,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.inner.resolution_fail(origin, id, failure)
    }

    /// The app's signatures, each `(transaction, signer, signature)` with
    /// `transaction` an index into the resolution's `transactions`. All go
    /// in or none does. Once none is owed it is `sending`, and the window
    /// sends it (see [`Self::resolution_begin_send`]).
    pub fn resolution_provide_signatures(
        &self,
        origin: &str,
        id: &str,
        signatures: Vec<(usize, solana_pubkey::Pubkey, [u8; 64])>,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.inner
            .resolution_provide_signatures(origin, id, signatures)
    }

    /// The wires to send for a resolution that is `sending` and that nobody
    /// is sending yet (`KeptResolution::ready_to_send`), in order. Given
    /// once: whoever takes them sends them, then calls
    /// [`Self::resolution_sent`].
    pub fn resolution_begin_send(&self, origin: &str, id: &str) -> Option<Vec<Vec<u8>>> {
        self.inner.resolution_begin_send(origin, id)
    }

    /// What sending each transaction came to, one for each of its
    /// `transactions` (`crate::resolutions::send_results` reads a site
    /// send's results).
    pub fn resolution_sent(
        &self,
        origin: &str,
        id: &str,
        results: Vec<SendResult>,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.inner.resolution_sent(origin, id, results)
    }

    /// The proposal that its transaction `transaction` created ran, in the
    /// transaction `signature`.
    pub fn resolution_executed(
        &self,
        origin: &str,
        id: &str,
        transaction: usize,
        signature: Signature,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.inner
            .resolution_executed(origin, id, transaction, signature)
    }

    /// A proposal it waits on was rejected or cancelled: `failed`.
    pub fn resolution_proposal_closed(
        &self,
        origin: &str,
        id: &str,
        reason: String,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.inner.resolution_proposal_closed(origin, id, reason)
    }
}
