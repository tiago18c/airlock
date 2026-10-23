//! The state the core publishes. Each slice has one owner, and goes out as a
//! `watch` of `Arc<T>`: readers get the latest value and never block it.

use std::sync::Arc;
use std::time::Duration;

use solana_pubkey::Pubkey;
use tokio::sync::watch;
use wallet_core::{AddressBook, Cluster, Holdings, Vault, VaultData, WalletId, WalletSummary};

use crate::bridge::BridgeQueue;
use crate::notices::Notices;
use crate::storage::{AppHoldings, AppNotices, AppVault, AppWalletList, AppWorkspace};

/// One published value.
pub struct Slice<T> {
    tx: watch::Sender<Arc<T>>,
}

impl<T> Slice<T> {
    pub(crate) fn new(value: Arc<T>) -> Self {
        Self {
            tx: watch::channel(value).0,
        }
    }

    /// The latest value.
    pub fn get(&self) -> Arc<T> {
        self.tx.borrow().clone()
    }

    /// A receiver that wakes on `changed()` and reads the latest value.
    pub fn subscribe(&self) -> watch::Receiver<Arc<T>> {
        self.tx.subscribe()
    }

    pub(crate) fn publish(&self, value: Arc<T>) {
        self.tx.send_replace(value);
    }
}

/// Every slice of the core.
pub struct Slices {
    pub wallets: Slice<WalletsView>,
    pub book: Slice<AddressBook>,
    pub holdings: Slice<Holdings>,
    pub network: Slice<NetworkStatus>,
    pub vault: Slice<VaultState>,
    pub settings: Slice<Settings>,
    pub bridge: Slice<BridgeQueue>,
    pub notices: Slice<Notices>,
    /// Background sync jobs. Empty until the online scheduler registers them.
    pub sync: Slice<SyncStatus>,
}

/// Public wallet list and related pubkeys. While the vault is locked it comes
/// from the copy kept without secrets: `local_signer_pubkeys` then names the
/// keys the vault holds, which sign only once it is unlocked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WalletsView {
    pub summaries: Vec<WalletSummary>,
    pub selected: Option<WalletId>,
    pub local_signer_pubkeys: Vec<Pubkey>,
    pub local_vector_accounts: Vec<Pubkey>,
}

/// The cluster in use, and whether its node answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkStatus {
    pub cluster: Cluster,
    pub reach: Reach,
}

/// What the last probe of the node found. A status for the chrome only: the
/// online build offers its online features whatever it says, and a call to a
/// node that is down fails with its own error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reach {
    /// A probe is running, or none has run yet.
    #[default]
    Probing,
    Answered,
    Unreachable,
}

/// What each background sync job last did and when it will try again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncStatus {
    pub jobs: Vec<JobSyncStatus>,
}

/// One job's last success, last error and next scheduled run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSyncStatus {
    pub job: String,
    /// Wall-clock ms of the last successful run, when there was one.
    pub last_success_ms: Option<u64>,
    /// Error from the last failed run; cleared on success.
    pub last_error: Option<String>,
    /// Wall-clock ms when the job will run next (interval or backoff).
    pub next_run_ms: Option<u64>,
}

impl SyncStatus {
    pub fn job(&self, name: &str) -> Option<&JobSyncStatus> {
        self.jobs.iter().find(|j| j.job == name)
    }
}

/// Whether the vault exists and whether its secrets are in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultState {
    Uninitialized,
    Locked,
    Unlocked,
}

/// What the user chose in Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub vector_wallets: bool,
    /// How long the vault stays unlocked while the window is hidden, or idle
    /// while visible.
    pub auto_lock: Duration,
    /// Close hides the window instead of quitting.
    pub close_to_tray: bool,
    /// Key for the Tokens API, which prices the portfolio.
    pub tokens_api_key: Option<ApiKey>,
}

/// An API key. Its `Debug` does not print it, so it stays out of logs.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(pub String);

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(..)")
    }
}

impl Settings {
    pub const DEFAULT_AUTO_LOCK: Duration =
        Duration::from_secs(wallet_core::DEFAULT_AUTO_LOCK_SECS);
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            vector_wallets: false,
            auto_lock: Self::DEFAULT_AUTO_LOCK,
            close_to_tray: false,
            tokens_api_key: None,
        }
    }
}

/// Settings published from a workspace file (or defaults for missing fields).
pub fn settings_from(workspace: &AppWorkspace) -> Arc<Settings> {
    let data = wallet_core::WorkspaceStore::data(workspace);
    let secs = if data.auto_lock_secs == 0 {
        wallet_core::DEFAULT_AUTO_LOCK_SECS
    } else {
        data.auto_lock_secs
    };
    Arc::new(Settings {
        vector_wallets: data.vector_wallets,
        auto_lock: Duration::from_secs(secs),
        close_to_tray: data.close_to_tray,
        tokens_api_key: data
            .tokens_api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(|key| ApiKey(key.to_string())),
    })
}

/// The wallets the screens show: the vault's while it is unlocked, the copy
/// kept without secrets while it is locked, none before it exists.
pub fn shown_wallets<'a>(vault: &'a AppVault, list: &'a AppWalletList) -> Option<&'a VaultData> {
    match vault_state_from(vault) {
        VaultState::Unlocked => Vault::data(vault).ok(),
        VaultState::Locked => list.data(),
        VaultState::Uninitialized => None,
    }
}

pub fn wallets_view_from(data: Option<&VaultData>) -> Arc<WalletsView> {
    let Some(data) = data else {
        return Arc::new(WalletsView::default());
    };
    Arc::new(WalletsView {
        summaries: data.summaries(),
        selected: data.acting_wallet(),
        local_signer_pubkeys: data.local_signer_pubkeys(),
        local_vector_accounts: wallet_core::vector::held_vector_accounts(&data.wallets),
    })
}

pub fn book_from(workspace: &AppWorkspace) -> Arc<AddressBook> {
    Arc::new(wallet_core::WorkspaceStore::data(workspace).book.clone())
}

pub fn network_from(workspace: &AppWorkspace, reach: Reach) -> Arc<NetworkStatus> {
    Arc::new(NetworkStatus {
        cluster: wallet_core::WorkspaceStore::data(workspace).cluster.clone(),
        reach,
    })
}

pub fn holdings_from(holdings: &AppHoldings) -> Arc<Holdings> {
    Arc::new(wallet_core::HoldingsStore::data(holdings).clone())
}

pub fn notices_from(notices: &AppNotices) -> Arc<Notices> {
    Arc::new(notices.data().clone())
}

pub fn vault_state_from(vault: &AppVault) -> VaultState {
    if !vault.is_initialized().unwrap_or(false) {
        VaultState::Uninitialized
    } else if vault.is_unlocked() {
        VaultState::Unlocked
    } else {
        VaultState::Locked
    }
}
