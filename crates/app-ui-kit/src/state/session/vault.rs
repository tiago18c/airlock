//! The vault: creating it, unlocking it, and saving every change to it. The
//! vault itself lives in the core; these are thin calls into it.

use super::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Run once, after the vault is unlocked.
type Then = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

/// What waits for the vault to be unlocked.
#[derive(Clone)]
pub struct UnlockRequest {
    /// Why the password is asked for, as one sentence.
    pub why: &'static str,
    then: Then,
}

impl Session {
    /// Signer records — no-op when the vault is locked. A failed save rolls
    /// the change back and records it on `save_error`. `name` is what the
    /// core logs the command as.
    pub(in crate::state) fn mutate_vault(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut VaultData),
    ) -> Result<(), VaultError> {
        if *self.core().slices().vault.get() != VaultState::Unlocked {
            return Ok(());
        }
        let saved = self.command(|core| core.vault_checked(name, f));
        self.report_save(saved)
    }

    pub fn create(&self, password: &str) -> Result<(), VaultError> {
        let saved = self.command(|core| core.create_vault(password));
        if let Err(err) = self.report_save(saved) {
            self.lock();
            return Err(err);
        }
        self.dismiss_gate();
        Ok(())
    }

    pub fn unlock(&self, password: &str) -> Result<(), VaultError> {
        self.command(|core| core.unlock(password))?;
        self.report_save(Ok(()))?;
        self.dismiss_gate();
        Ok(())
    }

    pub fn lock(&self) {
        self.command(|core| core.lock());
        self.forms.wallet.clear();
        let mut modal = self.forms.wallet.modal_open;
        modal.set(false);
    }

    /// Run `then` now when the vault is unlocked. Otherwise ask for the
    /// password, saying it is needed `why`, and run `then` once the vault is
    /// unlocked; nothing runs if the user cancels.
    pub fn when_unlocked(&self, why: &'static str, then: impl FnOnce() + 'static) {
        if self.is_unlocked() {
            then();
            return;
        }
        let mut prompt = self.unlock_prompt;
        prompt.set(Some(UnlockRequest {
            why,
            then: Rc::new(RefCell::new(Some(Box::new(then)))),
        }));
    }

    /// The user unlocked from the prompt: run what waited for it.
    pub fn unlock_prompt_done(&self) {
        let mut prompt = self.unlock_prompt;
        let Some(request) = prompt.take() else {
            return;
        };
        let then = request.then.borrow_mut().take();
        if let Some(then) = then {
            then();
        }
    }

    /// The user closed the prompt: what waited for it does not run.
    pub fn unlock_prompt_cancel(&self) {
        let mut prompt = self.unlock_prompt;
        prompt.set(None);
    }

    pub fn is_unlocked(&self) -> bool {
        (self.vault_state)() == VaultState::Unlocked
    }

    pub fn is_initialized(&self) -> bool {
        (self.vault_state)() != VaultState::Uninitialized
    }
}
