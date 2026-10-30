//! The app's shared state: the core's slices as read-only signals, the screen
//! forms, and the operations the screens perform, which call into the core.

use crate::platform::storage::{AppIdls, AppLeafMarks};
use crate::state::forms::{BlockhashExpiry, Forms, TxSource};
use crate::state::proposals::ProposalsOpen;
use crate::state::pump::Pump;
use crate::state::slices::{NetworkStatus, VaultState, WalletsView};
use crate::state::submit::SubmitHandle;
use app_core::bridge::{BridgeAnswer, BridgeQueue};
use app_core::{Core, NoticeId, Notices, Settings, SyncStatus};
use dioxus::prelude::{CopyValue, Memo, ReadSignal, ReadableExt, Signal, WritableExt};
use solana_pubkey::Pubkey;
use std::collections::HashSet;
use std::sync::Arc;
use wallet_core::{
    AddressBook, ChainSnapshot, Cluster, EnrollError, HoldingSummary, Holdings, IdlBundleReport,
    IdlCheck, IdlError, IdlRecord, KeypairSigner, NewSigner, RosterError, RouteError, RpcError,
    SigningRoute, SolanaRpc, TokenFlag, TokenHolding, Vault, VaultData, VaultError, WalletId,
    WalletKind, WalletRecord, Workspace,
};

/// Future returned by a user-triggered chain refresh.
pub type RefreshFut =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>;

/// What the online shell plugs in for probes and the shared RPC client.
/// Airgap leaves no-ops / offline errors.
#[derive(Clone)]
pub struct OnlineHooks {
    get_rpc: Arc<dyn Fn() -> Result<Arc<dyn SolanaRpc>, RpcError> + Send + Sync>,
    start_detect: Arc<dyn Fn() + Send + Sync>,
    start_refresh: Arc<dyn Fn() + Send + Sync>,
    /// Address Book "Refresh from chain": run-now on the balances sync job.
    refresh_from_chain: Arc<dyn Fn() -> RefreshFut + Send + Sync>,
    /// Portfolio "Refresh": run-now on the prices sync job.
    refresh_prices: Arc<dyn Fn() -> RefreshFut + Send + Sync>,
}

impl OnlineHooks {
    pub fn new(
        get_rpc: impl Fn() -> Result<Arc<dyn SolanaRpc>, RpcError> + Send + Sync + 'static,
        start_detect: impl Fn() + Send + Sync + 'static,
        start_refresh: impl Fn() + Send + Sync + 'static,
        refresh_from_chain: impl Fn() -> RefreshFut + Send + Sync + 'static,
    ) -> Self {
        Self {
            get_rpc: Arc::new(get_rpc),
            start_detect: Arc::new(start_detect),
            start_refresh: Arc::new(start_refresh),
            refresh_from_chain: Arc::new(refresh_from_chain),
            refresh_prices: Arc::new(|| {
                Box::pin(async { Err("Prices are not available in this build.".into()) })
            }),
        }
    }

    /// Set what the Portfolio's Refresh runs.
    pub fn with_refresh_prices(
        mut self,
        refresh_prices: impl Fn() -> RefreshFut + Send + Sync + 'static,
    ) -> Self {
        self.refresh_prices = Arc::new(refresh_prices);
        self
    }

    /// No sockets: every rpc call is offline, detect/refresh do nothing.
    pub fn none() -> Self {
        Self::new(
            || Err(RpcError::Offline),
            || {},
            || {},
            || Box::pin(async { Err("RPC is disabled in offline mode.".into()) }),
        )
    }
}

mod idls;
mod network;
mod stores;
mod vault;
mod wallets;

pub use vault::UnlockRequest;
pub use wallet_core::WalletSummary;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Portfolio,
    AddressBook,
    Programs,
    Wallets,
    Stake,
    Build,
    Sign,
    Settings,
}

#[derive(Clone, Copy)]
pub struct Session {
    /// The core: stores, vault and saving. Commands go through it.
    core: CopyValue<Core>,
    /// Moves what the core publishes into the signals below.
    pump: Pump,
    pub idls: Signal<AppIdls>,
    /// Whether the IDL pack, the third-party IDLs shipped beside the app, is
    /// in `idls` yet.
    pub idl_pack: Signal<crate::platform::idl_pack::IdlPackState>,
    /// Programs a node said publish no IDL, by the node's URL. Not asked
    /// again while the app runs.
    pub idl_misses: CopyValue<HashSet<(String, Pubkey)>>,
    /// XMSS leaf marks; written before the vault when a leaf is reserved.
    pub leaf_marks: Signal<AppLeafMarks>,
    /// Last XMSS leaf and salt per account, so a second Sign press reuses them.
    pub xmss_retries: Signal<wallet_core::vector::XmssRetryCache>,
    pub tab: Signal<Tab>,
    pub forms: Forms,
    pub save_error: Signal<Option<String>>,
    /// Welcome lock screen. Skip or unlock clears it; later locks stay in the app.
    pub gate: Signal<bool>,
    /// Something waits for the vault to be unlocked; the shell asks for the
    /// password. See [`Session::when_unlocked`].
    pub unlock_prompt: Signal<Option<UnlockRequest>>,
    /// Probes and the shared RPC client, provided by the online shell.
    online: CopyValue<OnlineHooks>,
    /// Sends and tracks transactions. The online shell plugs in the submitter.
    submitter: CopyValue<SubmitHandle>,
    /// Proposals modal target; Wallets sets it, the online shell mounts the modal.
    pub proposals: Signal<Option<ProposalsOpen>>,
    /// Public wallet list. While the vault is locked it is the copy kept
    /// without secrets.
    pub wallets: ReadSignal<Arc<WalletsView>>,
    /// Address book.
    pub book: ReadSignal<Arc<AddressBook>>,
    /// Cluster, mode and probe flag.
    pub network: ReadSignal<Arc<NetworkStatus>>,
    /// Token-account cache for display.
    pub holdings_view: ReadSignal<Arc<Holdings>>,
    /// What the user chose in Settings.
    pub settings: ReadSignal<Arc<Settings>>,
    /// Notices for toasts and the notification center.
    pub notices: ReadSignal<Arc<Notices>>,
    /// Background sync job status.
    pub sync: ReadSignal<Arc<SyncStatus>>,
    /// Browser requests waiting for the user. Empty without the bridge.
    pub bridge: ReadSignal<Arc<BridgeQueue>>,
    /// Sites connected, remembered and blocked (desktop shell).
    pub sites: ReadSignal<Arc<app_core::bridge::SitesView>>,
    /// Whether the vault exists and is unlocked.
    pub vault_state: Memo<VaultState>,
    /// Notification center panel open.
    pub notices_open: Signal<bool>,
    /// When set, the panel scrolls to / expands this notice.
    pub notices_focus: Signal<Option<NoticeId>>,
    /// What to call each address, from the wallets, the book and the holdings.
    pub directory: Memo<Arc<wallet_core::Directory>>,
}

impl Session {
    /// Build the shared session. Only [`use_session`](crate::use_session)
    /// calls this. The stores stay in the core, and the slices come from the
    /// pump.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        core: CopyValue<Core>,
        pump: Pump,
        idls: Signal<AppIdls>,
        idl_pack: Signal<crate::platform::idl_pack::IdlPackState>,
        idl_misses: CopyValue<HashSet<(String, Pubkey)>>,
        leaf_marks: Signal<AppLeafMarks>,
        xmss_retries: Signal<wallet_core::vector::XmssRetryCache>,
        tab: Signal<Tab>,
        forms: Forms,
        save_error: Signal<Option<String>>,
        gate: Signal<bool>,
        unlock_prompt: Signal<Option<UnlockRequest>>,
        online: CopyValue<OnlineHooks>,
        submitter: CopyValue<SubmitHandle>,
        proposals: Signal<Option<ProposalsOpen>>,
        notices_open: Signal<bool>,
        notices_focus: Signal<Option<NoticeId>>,
        directory: Memo<Arc<wallet_core::Directory>>,
    ) -> Self {
        Self {
            core,
            pump,
            idls,
            idl_pack,
            idl_misses,
            leaf_marks,
            xmss_retries,
            tab,
            forms,
            save_error,
            gate,
            unlock_prompt,
            online,
            submitter,
            proposals,
            wallets: pump.wallets,
            book: pump.book,
            network: pump.network,
            holdings_view: pump.holdings,
            settings: pump.settings,
            notices: pump.notices,
            sync: pump.sync,
            bridge: pump.bridge,
            sites: pump.sites,
            vault_state: pump.vault_state,
            notices_open,
            notices_focus,
            directory,
        }
    }

    /// The shell reports user input, for the idle lock.
    pub fn ui_activity(&self) {
        self.core.peek().ui_activity();
    }

    /// Hide a toast; the notice stays in the list.
    pub fn dismiss_toast(&self, id: NoticeId) {
        self.command(|core| core.notices_dismiss_toast(id));
    }

    /// Open the notification center, optionally focused on one notice.
    pub fn open_notices(&self, focus: Option<NoticeId>) {
        let mut focus_sig = self.notices_focus;
        focus_sig.set(focus);
        let mut open = self.notices_open;
        open.set(true);
    }

    /// Close the notification center.
    pub fn close_notices(&self) {
        let mut open = self.notices_open;
        open.set(false);
        let mut focus = self.notices_focus;
        focus.set(None);
    }

    /// Answer a browser request with what the user decided. Answers whether
    /// it was still waiting and the answer fits it.
    pub fn bridge_answer(&self, id: &str, answer: BridgeAnswer) -> bool {
        self.command(|core| core.bridge_answer(id, &answer))
    }

    /// Answer a `connect` with the accounts the user chose. Answers whether
    /// it was still waiting.
    pub fn bridge_connect(
        &self,
        id: &str,
        accounts: Vec<wallet_core::ShareableAccount>,
        remember: bool,
    ) -> bool {
        self.command(|core| core.bridge_connect(id, accounts, remember))
    }

    /// Answer a `signIn` with what `account` signed. Answers whether it was
    /// still waiting.
    pub fn bridge_sign_in(
        &self,
        id: &str,
        account: solana_pubkey::Pubkey,
        signed: Vec<u8>,
        signature: [u8; 64],
        remember: bool,
    ) -> bool {
        self.command(|core| core.bridge_sign_in(id, account, signed, signature, remember))
    }

    /// Turn away the site that made request `id` from now on.
    pub fn bridge_block(&self, id: &str) -> bool {
        self.command(|core| core.bridge_block(id))
    }

    /// End `origin`'s connection. What it was given stays.
    pub fn site_disconnect(&self, origin: &str) {
        self.command(|core| core.site_disconnect(origin));
    }

    /// Forget what `origin` was given and disconnect it.
    pub fn site_revoke(&self, origin: &str) {
        self.command(|core| core.site_revoke(origin));
    }

    /// Turn `origin` away from now on.
    pub fn site_block(&self, origin: &str) {
        self.command(|core| core.site_block(origin));
    }

    pub fn site_unblock(&self, origin: &str) {
        self.command(|core| core.site_unblock(origin));
    }

    /// What the connect modal opens with for `origin`.
    pub fn site_choice(&self, origin: &str) -> app_core::bridge::SiteChoice {
        self.track_wallets();
        self.core().site_choice(origin)
    }

    /// Mark the given notices read.
    pub fn notices_mark_read(&self, ids: &[NoticeId]) {
        self.command(|core| core.notices_mark_read(ids));
    }

    /// Mark every notice read.
    pub fn notices_mark_all_read(&self) {
        self.command(|core| core.notices_mark_all_read());
    }

    /// Remove one notice; a transaction still `Sent` stays until it ends.
    pub fn notices_remove(&self, id: NoticeId) {
        self.command(|core| core.notices_remove(id));
    }

    /// Clear notices; transactions still `Sent` stay until they end.
    pub fn notices_clear(&self) {
        self.command(|core| core.notices_clear());
    }

    /// Open a URL through the shell's [`Links`](app_core::Links) port.
    pub fn open_link(&self, url: &str) -> Result<(), String> {
        self.core().ports().links.open(url)
    }

    /// A handle on the core, for a command or a read.
    pub(crate) fn core(&self) -> Core {
        self.core.peek().clone()
    }

    /// Ask the online shell's monitor to probe the node.
    pub fn start_detect(&self) {
        (self.online.peek().start_detect)();
    }

    /// Ask the online shell's monitor to refresh balances.
    pub fn start_refresh(&self) {
        (self.online.peek().start_refresh)();
    }

    /// Address Book Refresh: run the balances sync job now.
    pub fn refresh_from_chain(&self) -> RefreshFut {
        (self.online.peek().refresh_from_chain)()
    }

    /// Ask the prices job to run now.
    pub fn refresh_prices(&self) -> RefreshFut {
        (self.online.peek().refresh_prices)()
    }

    /// The submitter: the only path that sends a transaction.
    pub fn submitter(&self) -> SubmitHandle {
        self.submitter.peek().clone()
    }

    /// Open the Proposals modal for a Squads wallet (online shell mounts it).
    pub fn open_proposals(
        &self,
        wallet: WalletId,
        label: String,
        squad: wallet_core::squads::Squad,
    ) {
        let mut open = self.proposals;
        open.set(Some(ProposalsOpen {
            wallet,
            label,
            squad,
        }));
    }

    /// Close the Proposals modal.
    pub fn close_proposals(&self) {
        let mut open = self.proposals;
        open.set(None);
    }

    /// Open the Send modal with what the caller already knows (online shell
    /// mounts it).
    pub fn open_send(&self, open: crate::state::forms::SendOpen) {
        let mut slot = self.forms.send.open;
        slot.set(Some(open));
    }

    /// Close the Send modal.
    pub fn close_send(&self) {
        let mut slot = self.forms.send.open;
        slot.set(None);
    }

    /// Run a core command, then bring the pumped signals up to date, so a
    /// read right after it sees the change.
    pub(crate) fn command<R>(&self, run: impl FnOnce(&Core) -> R) -> R {
        let core = self.core();
        let out = run(&core);
        self.pump.catch_up(&core);
        out
    }

    /// Make the running screen depend on the wallets, for reads that go to
    /// the core instead of the pumped signal.
    fn track_wallets(&self) {
        let _ = self.wallets.read();
    }

    fn report_save(&self, result: Result<(), VaultError>) -> Result<(), VaultError> {
        let mut shown = self.save_error;
        match result {
            Ok(()) => {
                shown.set(None);
                Ok(())
            }
            Err(err) => {
                shown.set(Some(format!("Could not save: {err}")));
                Err(err)
            }
        }
    }

    /// Stores that could not be read at startup. Each message says whether a
    /// copy was kept or saving is blocked.
    pub fn load_problems(&self) -> Vec<String> {
        let mut out = self.core().load_problems();
        if let Some(problem) = self.idls.peek().load_problem() {
            out.push(problem.message().to_string());
        }
        if let Some(problem) = self.leaf_marks.peek().load_problem() {
            out.push(problem.message().to_string());
        }
        out
    }
    /// Show a message in the banner that reports failed saves.
    pub fn show_problem(&self, message: String) {
        let mut shown = self.save_error;
        shown.set(Some(message));
    }

    pub fn dismiss_gate(&self) {
        let mut gate = self.gate;
        gate.set(false);
    }

    pub fn current_tab(&self) -> Tab {
        *self.tab.read()
    }

    pub fn goto(&self, tab: Tab) {
        let mut tab_sig = self.tab;
        tab_sig.set(tab);
    }

    /// Hand a transaction to the Sign screen and go there. This is the one
    /// way in for every other part of the app.
    pub fn open_in_sign(
        &self,
        wire: Vec<u8>,
        from: TxSource,
        note: impl Into<wallet_core::Phrase>,
    ) {
        self.open_in_sign_with_lifetime(wire, from, note, None);
    }

    /// Like [`open_in_sign`](Self::open_in_sign), when this app chose the
    /// blockhash and knows when it expires.
    pub fn open_in_sign_with_lifetime(
        &self,
        wire: Vec<u8>,
        from: TxSource,
        note: impl Into<wallet_core::Phrase>,
        last_valid_height: Option<u64>,
    ) {
        self.open_in_sign_prepared(
            wire,
            from,
            note,
            last_valid_height.map(BlockhashExpiry::exact),
            wallet_core::LookupTables::new(),
        );
    }

    /// Hand a prepared transaction to Sign, with when its blockhash expires
    /// when that is known, and lookup tables for v0.
    pub fn open_in_sign_prepared(
        &self,
        wire: Vec<u8>,
        from: TxSource,
        note: impl Into<wallet_core::Phrase>,
        expiry: Option<BlockhashExpiry>,
        tables: wallet_core::LookupTables,
    ) {
        self.open_in_sign_adapted(wire, from, note, expiry, tables, Vec::new());
    }

    /// [`open_in_sign_prepared`](Self::open_in_sign_prepared), with what a
    /// rebuild already read of the Vector accounts the transaction lists.
    pub fn open_in_sign_adapted(
        &self,
        wire: Vec<u8>,
        from: TxSource,
        note: impl Into<wallet_core::Phrase>,
        expiry: Option<BlockhashExpiry>,
        tables: wallet_core::LookupTables,
        vector: Vec<wallet_core::relay::VectorContext>,
    ) {
        self.open_in_sign_flow(wire, from, note, expiry, tables, vector, None);
    }

    /// Like [`open_in_sign_adapted`](Self::open_in_sign_adapted), reusing a
    /// log flow id when the transaction was already received.
    #[allow(clippy::too_many_arguments)]
    pub fn open_in_sign_flow(
        &self,
        wire: Vec<u8>,
        from: TxSource,
        note: impl Into<wallet_core::Phrase>,
        expiry: Option<BlockhashExpiry>,
        tables: wallet_core::LookupTables,
        vector: Vec<wallet_core::relay::VectorContext>,
        flow: Option<u64>,
    ) {
        let note: wallet_core::Phrase = note.into();
        self.forms.sign.load_prepared_flow(
            wire,
            from,
            Some(note).filter(|note| !note.plain().is_empty()),
            expiry,
            tables,
            vector,
            flow,
        );
        self.goto(Tab::Sign);
    }

    /// Hand a site's batch to Sign, on its transaction `batch.current`, and
    /// go there.
    pub fn open_batch_in_sign(&self, batch: crate::state::forms::BrowserBatch) {
        self.forms.sign.load_batch(batch);
        self.goto(Tab::Sign);
    }

    pub fn clear_build(&self) {
        self.forms.build.clear();
        self.default_build_fee_payer();
    }
}
