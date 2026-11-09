//! Settings, and what the shell reports about the network.

use std::sync::Arc;
use std::time::Duration;

use wallet_core::{VaultError, Workspace};

use super::{locked, Core, Inner};
use crate::slices::{network_from, Reach};

impl Core {
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
}

impl Inner {
    pub(crate) fn set_reach(&self, reach: Reach) {
        // With the workspace's lock, so that this cannot pass a cluster change.
        let workspace = locked(&self.workspace);
        *locked(&self.reach) = reach;
        self.slices.network.publish(network_from(&workspace, reach));
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
}
