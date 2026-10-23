//! The app's state, saving, vault and logging, with no UI and no network, so
//! the same core runs under any shell, native or web.
//!
//! Stores are owned here and published as slices ([`slices`]). Commands
//! ([`Core`]) change them and save, checked or deferred. [`PublicCore`] is the
//! handle services get: it reaches no secrets. See `docs/architecture.md`.

pub mod bridge;
pub mod diagnostics;
pub mod events;
pub mod lock;
pub mod notices;
pub mod ports;
pub mod runtime;
pub mod slices;
pub mod storage;

mod commands;
mod public;

#[cfg(not(target_arch = "wasm32"))]
pub mod testing;

pub use crate::commands::{Core, SAVE_INTERVAL};
pub use events::Event;
pub use lock::{LockPolicy, LockReason};
pub use notices::{
    notice_row_text, BridgeRequestOutcome, FlowId, Notice, NoticeId, NoticeKind, Notices,
    ToastState, TxStatus, MAX_NOTICES, RETENTION_SECS,
};
pub use ports::{
    BoxFuture, Clock, Links, MaybeSend, NoLinks, NoNotifier, Notification, Notifier, Ports,
    Spawner, Storage,
};
pub use public::PublicCore;
pub use slices::{
    ApiKey, JobSyncStatus, NetworkStatus, Reach, Settings, Slice, Slices, SyncStatus, VaultState,
    WalletsView,
};
