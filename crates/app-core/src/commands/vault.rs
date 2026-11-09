//! The vault: creating, unlocking and locking it, and changing what it holds.

use std::sync::Arc;

use tracing::{debug, debug_span, info, warn};
use wallet_core::{
    AddressBook, EnrollError, PasswordChange, RosterError, SealKey, Unlocked, Unlocking, Vault,
    VaultData, VaultError, WalletId,
};

use super::{locked, report_checked, report_command, Core, Inner, CORE, VAULT};
use crate::events::Event;
use crate::lock::LockReason;
use crate::notices::NoticeKind;
use crate::slices::{shown_wallets, vault_state_from, wallets_view_from};
use crate::storage::{AppLeafMarks, AppVault};

impl Core {
    // The vault. Only here, never on `PublicCore`.

    // Deriving a key takes about a second. None of these holds the vault while
    // it does, and a window takes the steps apart so that it does not wait either.

    /// Start a vault under `password`.
    pub fn create_vault(&self, password: &str) -> Result<(), VaultError> {
        self.finish_create_vault(SealKey::new(password))
    }

    /// Start a vault under a key already derived.
    pub fn finish_create_vault(&self, key: Result<SealKey, VaultError>) -> Result<(), VaultError> {
        self.inner.finish_create_vault(key)
    }

    pub fn unlock(&self, password: &str) -> Result<(), VaultError> {
        let opened = self
            .begin_unlock()
            .and_then(|reading| reading.open(password));
        self.finish_unlock(opened)
    }

    /// The unlocked vault's key, for a [`KeyGuard`](crate::KeyGuard) to keep
    /// behind the user's fingerprint. Only an action the user started asks
    /// for it: turning fingerprint unlock on.
    pub fn vault_key_bytes(&self) -> Result<zeroize::Zeroizing<Vec<u8>>, VaultError> {
        locked(&self.inner.vault).seal_key_bytes()
    }

    /// Read what opening the vault needs, for [`Unlocking::open`].
    pub fn begin_unlock(&self) -> Result<Unlocking<VaultData>, VaultError> {
        self.inner.begin_unlock()
    }

    /// Take in what [`Unlocking::open`] made of it.
    pub fn finish_unlock(
        &self,
        opened: Result<Unlocked<VaultData>, VaultError>,
    ) -> Result<(), VaultError> {
        self.inner.finish_unlock(opened)
    }

    /// Seal the unlocked vault under `new`, once `current` opens it.
    pub fn change_password(&self, current: &str, new: &str) -> Result<(), VaultError> {
        let key = self
            .begin_password_change()
            .and_then(|change| change.derive(current, new));
        self.finish_password_change(key)
    }

    /// Read what changing the password needs, for [`PasswordChange::derive`].
    pub fn begin_password_change(&self) -> Result<PasswordChange, VaultError> {
        locked(&self.inner.vault).begin_password_change()
    }

    /// Seal under what [`PasswordChange::derive`] made.
    pub fn finish_password_change(
        &self,
        key: Result<SealKey, VaultError>,
    ) -> Result<(), VaultError> {
        self.inner.finish_password_change(key)
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

    /// Read the vault mutably. Publishes the slices after.
    pub fn with_vault_mut<R>(&self, f: impl FnOnce(&mut AppVault) -> R) -> R {
        let mut vault = locked(&self.inner.vault);
        let out = f(&mut vault);
        self.inner.publish_vault(&vault);
        out
    }

    /// Read the vault mutably with what each hash-based key has signed with, for
    /// the jobs that must reserve a signing leaf before anything is signed.
    /// Publishes the slices after.
    pub fn with_vault_and_marks<R>(
        &self,
        f: impl FnOnce(&mut AppVault, &mut AppLeafMarks) -> R,
    ) -> R {
        let mut vault = locked(&self.inner.vault);
        let out = f(&mut vault, &mut locked(&self.inner.leaf_marks));
        self.inner.publish_vault(&vault);
        out
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
}

impl Inner {
    /// Publish the vault, and while it is open keep its wallets without
    /// their secrets for when it is locked.
    pub(super) fn publish_vault(&self, vault: &AppVault) {
        let marks = locked(&self.leaf_marks);
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
            .publish(wallets_view_from(shown_wallets(vault, &list), &marks));
        self.slices.vault.publish(Arc::new(vault_state_from(vault)));
    }

    // The vault.

    fn finish_create_vault(&self, key: Result<SealKey, VaultError>) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name = "create_vault").entered();
        let mut vault = locked(&self.vault);
        let before = vault_state_from(&vault);
        let created = key.and_then(|key| vault.create_under(key));
        if created.is_ok() {
            info!(target: VAULT, "created");
        }
        if created.is_ok() || vault_state_from(&vault) != before {
            self.publish_vault(&vault);
        }
        report_command(&created);
        created
    }

    fn begin_unlock(&self) -> Result<Unlocking<VaultData>, VaultError> {
        let mut vault = locked(&self.vault);
        let before = vault_state_from(&vault);
        let reading = vault.begin_unlock();
        if vault_state_from(&vault) != before {
            // A file that could not be read before can be now, or the other way
            // round.
            self.publish_vault(&vault);
        }
        reading
    }

    fn finish_unlock(
        self: &Arc<Self>,
        opened: Result<Unlocked<VaultData>, VaultError>,
    ) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name = "unlock").entered();
        let unlocked = opened.map(|opened| {
            let recovered = {
                let mut vault = locked(&self.vault);
                if vault.finish_unlock(opened) {
                    info!(target: VAULT, "unlocked");
                    self.adopt_locked_choice(&mut vault);
                    self.publish_vault(&vault);
                }
                vault.take_recovered()
            };
            if recovered {
                warn!(target: VAULT, "the vault's file was damaged, and its backup was opened");
                let _ = self.notices_push(NoticeKind::VaultRecovered);
            }
        });
        report_command(&unlocked);
        unlocked
    }

    fn finish_password_change(&self, key: Result<SealKey, VaultError>) -> Result<(), VaultError> {
        let _span = debug_span!(target: CORE, "command", name = "change_password").entered();
        let changed = key.and_then(|key| locked(&self.vault).finish_password_change(key));
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
        let mut vault = locked(&self.vault);
        let enrolled = {
            let mut workspace = self.workspace.lock();
            let enrolled = wallet_core::enroll(&mut *vault, &mut *workspace, add, label);
            if enrolled.is_ok() {
                // Saved just now, and nothing else can have changed it yet.
                self.workspace.saved();
            }
            enrolled
        };
        match &enrolled {
            Ok(_) => {
                // The wallet list comes before the workspace, so the workspace's
                // lock is let go first. Each publishes what is current.
                self.publish_vault(&vault);
                self.publish_workspace(&self.workspace.lock());
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
}
