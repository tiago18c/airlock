//! The app's shared state: the core's slices as read-only signals, the screen
//! forms, and the operations the screens perform, which call into the core.

use crate::platform::storage::AppIdls;
use crate::state::forms::{Forms, SignLoad};
use crate::state::proposals::ProposalsOpen;
use crate::state::pump::Pump;
use crate::state::slices::{NetworkStatus, VaultState, WalletsView};
use crate::state::submit::SubmitHandle;
use app_core::bridge::{BridgeAnswer, BridgeQueue};
use app_core::{Core, NoticeId, Notices, Settings, SyncStatus};
use dioxus::prelude::{CopyValue, Memo, ReadSignal, ReadableExt, Signal, WritableExt};
use solana_pubkey::Pubkey;
use std::collections::{HashMap, HashSet};
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
    History,
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
    /// Token logos read from the logos folder, as `data:` URLs, by mint.
    /// `None` for a file that could not be read. Kept while the app runs,
    /// so each file is read once. See [`Session::logo_url`].
    pub logo_urls: CopyValue<HashMap<Pubkey, Option<String>>>,
    /// Last XMSS leaf and salt per account, so a second Sign press reuses them.
    pub xmss_retries: Signal<wallet_core::vector::XmssRetryCache>,
    /// Rotations this device was given to send, by the device that holds
    /// the key, until the chain shows each landed. Kept while the app runs,
    /// so one that did not land can be sent again without scanning it again.
    pub received_rotations: Signal<Vec<wallet_core::protocol::KeyRotation>>,
    pub tab: Signal<Tab>,
    pub forms: Forms,
    pub save_error: Signal<Option<String>>,
    /// Welcome lock screen. Skip or unlock clears it; later locks stay in the app.
    pub gate: Signal<bool>,
    /// Something waits for the vault to be unlocked; the shell asks for the
    /// password. See [`Session::when_unlocked`].
    pub unlock_prompt: Signal<Option<UnlockRequest>>,
    /// The job on a hardware wallet the device dialog shows. See
    /// `actions::device`.
    pub device: Signal<Option<crate::state::device::DeviceRun>>,
    /// Probes and the shared RPC client, provided by the online shell.
    online: CopyValue<OnlineHooks>,
    /// Sends and tracks transactions. The online shell plugs in the submitter.
    submitter: CopyValue<SubmitHandle>,
    /// Proposals modal target; Wallets sets it, the online shell mounts the modal.
    pub proposals: Signal<Option<ProposalsOpen>>,
    /// The account the inspector shows. Any address card can set it; the
    /// shell mounts the inspector.
    pub inspect: Signal<Option<Pubkey>>,
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
    /// What the wallets did on chain, newest first.
    pub history: ReadSignal<Arc<app_core::History>>,
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
        logo_urls: CopyValue<HashMap<Pubkey, Option<String>>>,
        xmss_retries: Signal<wallet_core::vector::XmssRetryCache>,
        received_rotations: Signal<Vec<wallet_core::protocol::KeyRotation>>,
        tab: Signal<Tab>,
        forms: Forms,
        save_error: Signal<Option<String>>,
        gate: Signal<bool>,
        unlock_prompt: Signal<Option<UnlockRequest>>,
        device: Signal<Option<crate::state::device::DeviceRun>>,
        online: CopyValue<OnlineHooks>,
        submitter: CopyValue<SubmitHandle>,
        proposals: Signal<Option<ProposalsOpen>>,
        inspect: Signal<Option<Pubkey>>,
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
            logo_urls,
            xmss_retries,
            received_rotations,
            tab,
            forms,
            save_error,
            gate,
            unlock_prompt,
            device,
            online,
            submitter,
            proposals,
            inspect,
            wallets: pump.wallets,
            book: pump.book,
            network: pump.network,
            holdings_view: pump.holdings,
            settings: pump.settings,
            notices: pump.notices,
            history: pump.history,
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

    /// The logo the prices job kept for `mint`, in `kind`, as a `data:` URL
    /// for an `img`: the page never loads a remote image. The file is read
    /// the first time it is asked for and kept in memory after, so a row
    /// drawn again does not read it again. `None` when the file is gone,
    /// and always in the browser build, which keeps no logos.
    pub fn logo_url(
        &self,
        mint: &Pubkey,
        kind: wallet_core::portfolio::LogoKind,
    ) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut urls = self.logo_urls;
            if let Some(known) = urls.peek().get(mint) {
                return known.clone();
            }
            let url =
                app_core::storage::load_logo(self.core().ports().storage.as_ref(), mint, kind)
                    .map(|bytes| app_core::storage::logo_data_url(kind, &bytes));
            urls.write().insert(*mint, url.clone());
            url
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (mint, kind);
            None
        }
    }

    /// The clock of the core, which a test can move.
    pub fn clock(&self) -> std::sync::Arc<dyn app_core::ports::Clock> {
        self.core().ports().clock.clone()
    }

    /// Whether `origin` was given every one of `accounts`, and may ask them for a
    /// signature. The user still approves the signature itself.
    pub fn site_given(&self, origin: &str, accounts: &[Pubkey]) -> bool {
        self.core().public().site_sign(origin, accounts) == app_core::bridge::SiteSign::Allowed
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

    /// Open the Manage tokens modal on `wallet`'s token accounts (online
    /// shell mounts it).
    pub fn open_cleanup(&self, wallet: WalletId) {
        let form = self.forms.cleanup;
        let (mut slot, mut notice) = (form.open, form.notice);
        notice.set(None);
        slot.set(Some(wallet));
    }

    /// Open the Migrate wallet modal, on `from` when it is known (online shell
    /// mounts it).
    pub fn open_migrate(&self, from: Option<WalletId>) {
        let form = self.forms.migrate;
        let (mut slot, mut notice) = (form.open, form.notice);
        notice.set(None);
        slot.set(Some(from));
    }

    /// Close the Migrate wallet modal.
    pub fn close_migrate(&self) {
        let mut slot = self.forms.migrate.open;
        slot.set(None);
    }

    /// Close the Manage tokens modal.
    pub fn close_cleanup(&self) {
        let mut slot = self.forms.cleanup.open;
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
    pub fn open_in_sign(&self, load: SignLoad) {
        self.forms.sign.load(load);
        self.goto(Tab::Sign);
    }

    /// Open `signature` on the History screen and go there.
    pub fn open_in_history(&self, signature: solana_signature::Signature) {
        let form = self.forms.history;
        let (mut selected, mut view, mut wallet) = (form.selected, form.view, form.wallet);
        selected.set(Some(signature));
        view.set(crate::state::forms::SignView::Summary);
        // A filter that hides it would hide it.
        let shown = wallet.peek().is_none_or(|only| {
            self.history
                .peek()
                .get(&signature)
                .is_none_or(|entry| entry.addresses.contains(&only))
        });
        if !shown {
            wallet.set(None);
        }
        self.goto(Tab::History);
    }

    /// Change the transaction history: a transaction a screen read from the
    /// node. `f` says whether it changed anything.
    pub fn history_deferred(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut app_core::History) -> bool,
    ) {
        self.command(|core| core.history_deferred(name, f));
    }

    /// What the notices say became of `signature`, sent from here.
    pub fn tx_status(&self, signature: &solana_signature::Signature) -> Option<app_core::TxStatus> {
        self.notices
            .read()
            .items()
            .iter()
            .find_map(|notice| match &notice.kind {
                app_core::NoticeKind::Transaction {
                    signature: known,
                    status,
                    ..
                } if known == signature => Some(status.clone()),
                _ => None,
            })
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
