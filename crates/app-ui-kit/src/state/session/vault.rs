//! The vault: creating it, unlocking it, and saving every change to it. The
//! vault itself lives in the core; these are thin calls into it.

use super::*;
use crate::rt::off_the_window;
use std::cell::RefCell;
use std::rc::Rc;
use wallet_core::SealKey;
use zeroize::Zeroizing;

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

    pub async fn create(&self, password: Zeroizing<String>) -> Result<(), VaultError> {
        let key = off_the_window(move || SealKey::new(&password)).await;
        let saved = self.command(|core| core.finish_create_vault(key));
        if let Err(err) = self.report_save(saved) {
            self.lock();
            return Err(err);
        }
        self.dismiss_gate();
        Ok(())
    }

    /// Taking in what was opened is one step from there to the end, so a form
    /// that goes away once the vault is open has finished by then.
    pub async fn unlock(&self, password: Zeroizing<String>) -> Result<(), VaultError> {
        let opened = match self.command(|core| core.begin_unlock()) {
            Ok(reading) => off_the_window(move || reading.open(&password)).await,
            Err(err) => Err(err),
        };
        self.command(|core| core.finish_unlock(opened))?;
        self.report_save(Ok(()))?;
        self.dismiss_gate();
        Ok(())
    }

    /// Seal the vault under `new`, once `current` opens it.
    pub async fn change_password(
        &self,
        current: Zeroizing<String>,
        new: Zeroizing<String>,
    ) -> Result<(), VaultError> {
        let key = match self.command(|core| core.begin_password_change()) {
            Ok(change) => off_the_window(move || change.derive(&current, &new)).await,
            Err(err) => Err(err),
        };
        self.command(|core| core.finish_password_change(key))
    }

    pub fn lock(&self) {
        self.command(|core| core.lock());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::submit::SubmitHandle;
    use crate::state::use_session;
    use app_core::testing::{kit, Kit};
    use dioxus::dioxus_core::{NoOpMutations, ScopeId, VirtualDom};
    use dioxus::prelude::*;
    use std::cell::Cell;
    use std::time::Duration;

    #[derive(Clone)]
    struct Held {
        core: Core,
        session: Rc<Cell<Option<Session>>>,
    }

    fn app(held: Held) -> Element {
        let core = use_hook(|| CopyValue::new(held.core.clone()));
        let hooks = OnlineHooks::new(
            || Err(RpcError::Offline),
            || {},
            || {},
            || Box::pin(async { Ok(String::new()) }),
        );
        held.session
            .set(Some(use_session(core, hooks, SubmitHandle::none())));
        rsx! {}
    }

    struct Window {
        kit: Kit,
        dom: VirtualDom,
        session: Session,
    }

    impl Window {
        fn new() -> Self {
            let kit = kit();
            let session = Rc::new(Cell::new(None));
            let mut dom = VirtualDom::new_with_props(
                app,
                Held {
                    core: kit.core.clone(),
                    session: session.clone(),
                },
            );
            dom.rebuild_in_place();
            let session = session.get().unwrap();
            Self { kit, dom, session }
        }

        /// Run `work` as a form does, and answer what it came to once it has.
        async fn run<F>(&mut self, work: F) -> Result<(), VaultError>
        where
            F: std::future::Future<Output = Result<(), VaultError>> + 'static,
        {
            let answer = Rc::new(RefCell::new(None));
            let told = answer.clone();
            self.dom.in_scope(ScopeId::ROOT, || {
                spawn(async move {
                    let out = work.await;
                    *told.borrow_mut() = Some(out);
                });
            });
            for _ in 0..200 {
                let _ =
                    tokio::time::timeout(Duration::from_millis(10), self.dom.wait_for_work()).await;
                self.dom.render_immediate(&mut NoOpMutations);
                if let Some(out) = answer.borrow_mut().take() {
                    return out;
                }
            }
            panic!("it never finished");
        }

        fn state(&self) -> VaultState {
            self.dom
                .in_scope(ScopeId::ROOT, || (self.session.vault_state)())
        }
    }

    fn word(text: &str) -> Zeroizing<String> {
        Zeroizing::new(text.to_owned())
    }

    #[tokio::test]
    async fn a_vault_made_and_unlocked_away_from_the_window_is_on_screen_at_once() {
        let mut window = Window::new();
        let session = window.session;
        assert_eq!(window.state(), VaultState::Uninitialized);

        window
            .run(async move { session.create(word("pw")).await })
            .await
            .unwrap();
        assert_eq!(window.state(), VaultState::Unlocked);

        session.lock();
        assert_eq!(window.state(), VaultState::Locked);
        window
            .run(async move { session.unlock(word("pw")).await })
            .await
            .unwrap();
        assert_eq!(window.state(), VaultState::Unlocked);
    }

    #[tokio::test]
    async fn a_wrong_password_leaves_it_locked_and_says_so() {
        let mut window = Window::new();
        let session = window.session;
        window.kit.core.create_vault("pw").unwrap();
        session.lock();
        let tried = window
            .run(async move { session.unlock(word("nope")).await })
            .await;
        assert!(matches!(tried, Err(VaultError::UnlockFailed)));
        assert_eq!(window.state(), VaultState::Locked);
    }

    #[tokio::test]
    async fn there_is_no_vault_to_open_and_no_second_one_to_make() {
        let mut window = Window::new();
        let session = window.session;
        let tried = window
            .run(async move { session.unlock(word("pw")).await })
            .await;
        assert!(matches!(tried, Err(VaultError::NotInitialized)));

        window.kit.core.create_vault("pw").unwrap();
        let tried = window
            .run(async move { session.create(word("other")).await })
            .await;
        assert!(matches!(tried, Err(VaultError::AlreadyExists)));
    }

    #[tokio::test]
    async fn the_password_changes_once_the_current_one_opens_it() {
        let mut window = Window::new();
        let session = window.session;
        window.kit.core.create_vault("pw").unwrap();

        let tried = window
            .run(async move { session.change_password(word("nope"), word("new")).await })
            .await;
        assert!(matches!(tried, Err(VaultError::UnlockFailed)));

        window
            .run(async move { session.change_password(word("pw"), word("new")).await })
            .await
            .unwrap();
        session.lock();
        assert!(window.kit.core.unlock("pw").is_err());
        window.kit.core.unlock("new").unwrap();
    }
}
