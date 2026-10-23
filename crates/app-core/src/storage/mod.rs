//! Where the app keeps its files: the local app data folder on desktop,
//! `localStorage` in a browser. Nothing here knows about the UI.

mod dirs;
#[cfg(not(target_arch = "wasm32"))]
mod file;
#[cfg(target_arch = "wasm32")]
mod web;

pub use dirs::{app_dirs, set_app_dirs, AppDirs};
#[cfg(not(target_arch = "wasm32"))]
pub use file::{data_dir, logs_dir, FileStorage};
#[cfg(target_arch = "wasm32")]
pub use web::{LocalStorage, WebIdlBodies};

use wallet_core::vector::LeafMarks;
use wallet_core::{
    HoldingsStore, IdlStore, Vault, VaultError, VaultStore, WalletListStore, WorkspaceStore,
};

use crate::notices::NoticesStore;

/// A place that opens one named file, or one browser key, at a time.
pub trait Storage: Send + Sync {
    fn open(&self, name: &str) -> Box<dyn VaultStore>;
}

pub const VAULT_FILE: &str = "vault.json";
/// The vault's wallets without their secrets, shown while it is locked.
pub const WALLETS_FILE: &str = "wallets.json";
pub const WORKSPACE_FILE: &str = "workspace.json";
pub const HOLDINGS_FILE: &str = "holdings.json";
pub const NOTICES_FILE: &str = "notices.json";
const LEAF_MARKS_FILE: &str = "vector-leaves.json";

pub struct BoxedStore(pub Box<dyn VaultStore>);

impl VaultStore for BoxedStore {
    fn load(&self) -> Result<Option<Vec<u8>>, VaultError> {
        self.0.load()
    }

    fn save(&self, bytes: &[u8]) -> Result<(), VaultError> {
        self.0.save(bytes)
    }

    fn set_aside(&self, bytes: &[u8]) -> Result<(), VaultError> {
        self.0.set_aside(bytes)
    }
}

pub type AppVault = Vault<BoxedStore>;
pub type AppWalletList = WalletListStore<BoxedStore>;
pub type AppWorkspace = WorkspaceStore<BoxedStore>;
pub type AppHoldings = HoldingsStore<BoxedStore>;
pub type AppNotices = NoticesStore<BoxedStore>;
pub type AppLeafMarks = LeafMarks<BoxedStore>;
#[cfg(not(target_arch = "wasm32"))]
pub type AppIdls = IdlStore<BoxedStore, wallet_core::DirIdlBodies>;
#[cfg(target_arch = "wasm32")]
pub type AppIdls = IdlStore<BoxedStore, WebIdlBodies>;

/// The files of this platform: the data folder, or `localStorage`.
pub fn platform_storage() -> Box<dyn Storage> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Box::new(FileStorage::new(data_dir()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Box::new(LocalStorage)
    }
}

pub fn open_vault_in(storage: &dyn Storage) -> AppVault {
    Vault::new(BoxedStore(storage.open(VAULT_FILE)))
}

pub fn open_wallet_list_in(storage: &dyn Storage) -> AppWalletList {
    WalletListStore::open(BoxedStore(storage.open(WALLETS_FILE)))
}

pub fn open_workspace_in(storage: &dyn Storage) -> AppWorkspace {
    WorkspaceStore::open(BoxedStore(storage.open(WORKSPACE_FILE)))
}

pub fn open_holdings_in(storage: &dyn Storage) -> AppHoldings {
    HoldingsStore::open(BoxedStore(storage.open(HOLDINGS_FILE)))
}

pub fn open_notices_in(storage: &dyn Storage) -> AppNotices {
    NoticesStore::open(BoxedStore(storage.open(NOTICES_FILE)))
}

pub fn open_vault() -> AppVault {
    open_vault_in(&*platform_storage())
}

/// High-water marks for XMSS leaves, outside the vault so a restore of only
/// the vault cannot reuse a spent leaf.
pub fn open_leaf_marks() -> AppLeafMarks {
    LeafMarks::open(BoxedStore(platform_storage().open(LEAF_MARKS_FILE)))
}

pub fn open_workspace() -> AppWorkspace {
    open_workspace_in(&*platform_storage())
}

pub fn open_holdings() -> AppHoldings {
    open_holdings_in(&*platform_storage())
}

pub fn open_notices() -> AppNotices {
    open_notices_in(&*platform_storage())
}

pub fn open_idls() -> AppIdls {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = data_dir().join("idls");
        IdlStore::open(
            BoxedStore(Box::new(wallet_core::FileStore::new(
                dir.join("index.json"),
            ))),
            wallet_core::DirIdlBodies::new(dir),
        )
        .with_builtins()
    }
    #[cfg(target_arch = "wasm32")]
    {
        IdlStore::open(
            BoxedStore(LocalStorage.open("idls-index.json")),
            WebIdlBodies,
        )
        .with_builtins()
    }
}
