//! What the app holds while it runs: the core and its pumped slices, and what
//! each screen's forms contain.

pub mod forms;
pub mod online;
pub mod proposals;
pub mod pump;
pub mod session;
pub mod slices;
pub mod submit;
pub mod theme;

pub use online::use_network;
pub use proposals::ProposalsOpen;
pub use submit::{SiteSendFut, SubmitFut, SubmitHandle, TxSubmit};

use std::sync::Arc;

use app_core::{Core, NoNotifier, Notifier, Ports};
use dioxus::prelude::*;

use crate::state::forms::use_forms;
use crate::state::session::{OnlineHooks, Session, Tab};

/// Open the core on this platform's files, with tokio or the browser to run
/// its tasks. Call it inside the shell's runtime (Dioxus desktop provides one).
pub fn open_core() -> Core {
    open_core_with(Arc::new(NoNotifier))
}

/// Like [`open_core`], but with a platform [`Notifier`] (OS toasts on desktop).
pub fn open_core_with(notifier: Arc<dyn Notifier>) -> Core {
    #[cfg(not(target_arch = "wasm32"))]
    let ports = Ports {
        spawner: Arc::new(app_core::runtime::TokioSpawner::current()),
        clock: Arc::new(app_core::runtime::TokioClock),
        notifier,
        storage: Arc::from(app_core::storage::platform_storage()),
        links: Arc::new(app_core::runtime::NativeLinks),
    };
    #[cfg(target_arch = "wasm32")]
    let ports = Ports {
        spawner: Arc::new(app_core::runtime::WebSpawner),
        clock: Arc::new(app_core::runtime::WebClock),
        notifier,
        storage: Arc::from(app_core::storage::platform_storage()),
        links: Arc::new(app_core::runtime::WebLinks),
    };
    Core::open(ports)
}

/// The IDL catalog, on the network of the cluster in use, with the IDL pack
/// in it: read while it opens on desktop, fetched once in a browser.
fn use_idls(
    network: ReadSignal<Arc<app_core::NetworkStatus>>,
) -> (
    Signal<crate::platform::storage::AppIdls>,
    Signal<crate::platform::idl_pack::IdlPackState>,
) {
    use crate::platform::idl_pack::{open_catalog, IdlPackState};
    // Both signals start in this render: the catalog says how the pack went.
    let opened = std::cell::Cell::new(None);
    let idls = use_signal(|| {
        let (store, state) = open_catalog(wallet_core::IdlNetwork::of(&network.peek().cluster));
        opened.set(Some(state));
        store
    });
    let idl_pack = use_signal(|| opened.take().unwrap_or(IdlPackState::Loading));
    #[cfg(target_arch = "wasm32")]
    use_hook(move || {
        let (mut idls, mut idl_pack) = (idls, idl_pack);
        spawn(async move {
            let fetched = crate::platform::idl_pack::fetch_pack().await;
            let state = crate::platform::idl_pack::add_pack(&mut idls.write(), fetched);
            idl_pack.set(state);
        });
    });
    use_idl_network(network, idls);
    (idls, idl_pack)
}

/// Keep the IDL catalog on the network of the cluster in use. A custom RPC
/// reads the IDLs of the public cluster its genesis matched.
fn use_idl_network(
    network: ReadSignal<Arc<app_core::NetworkStatus>>,
    mut idls: Signal<crate::platform::storage::AppIdls>,
) {
    use_effect(move || {
        let wanted = wallet_core::IdlNetwork::of(&network().cluster);
        if idls.peek().network() != wanted {
            idls.write().set_network(wanted);
        }
    });
}

/// Show a deferred save that failed in the `save_error` banner. The core
/// keeps the change and tries again on the next change; the notification
/// center records it too, but not every build has one.
fn use_save_failures(core: CopyValue<Core>, mut save_error: Signal<Option<String>>) {
    use tokio::sync::broadcast::error::RecvError;
    use_hook(move || {
        let mut events = core.peek().events();
        spawn(async move {
            loop {
                match events.recv().await {
                    Ok(app_core::Event::SaveFailed { store, error }) => {
                        save_error.set(Some(format!(
                            "Could not save the {store}: {error}. The change is kept, and saving is tried again on the next change."
                        )));
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                }
            }
        });
    });
}

/// However the vault locks, by itself or from the tray as well as from the top
/// bar, the screens let go of the secrets they hold.
fn use_forgetting_on_lock(core: CopyValue<Core>, forms: forms::Forms) {
    use_hook(move || {
        let mut vault = core.peek().slices().vault.subscribe();
        spawn(async move {
            while vault.changed().await.is_ok() {
                let state = **vault.borrow_and_update();
                if state != app_core::VaultState::Unlocked {
                    forms.wallet.forget_secrets();
                }
            }
        });
    });
}

/// The session over `core`, provided as context for every screen: the
/// pumped slices, and the screen state a window holds (forms, tab, gate,
/// banners). `online` and `submitter` are what the shell plugs in; a shell
/// with no network passes [`OnlineHooks::none`] and
/// [`SubmitHandle::none`](crate::SubmitHandle::none). Call once, from the root.
pub fn use_session(core: CopyValue<Core>, online: OnlineHooks, submitter: SubmitHandle) -> Session {
    let pump = pump::use_core(core.peek().clone());
    let (idls, idl_pack) = use_idls(pump.network);
    let idl_misses = use_hook(|| CopyValue::new(std::collections::HashSet::new()));
    let leaf_marks = use_signal(crate::platform::storage::open_leaf_marks);
    let xmss_retries = use_signal(wallet_core::vector::XmssRetryCache::default);
    let tab = use_signal(|| Tab::Wallets);
    let forms = use_forms();
    use_forgetting_on_lock(core, forms);
    let gate = use_signal(|| true);
    let unlock_prompt = use_signal(|| None);
    let save_error = use_signal(|| None::<String>);
    use_save_failures(core, save_error);
    let proposals = use_signal(|| None::<ProposalsOpen>);
    let notices_open = use_signal(|| false);
    let notices_focus = use_signal(|| None::<app_core::NoticeId>);
    let online = use_hook(|| CopyValue::new(online));
    let submitter = use_hook(|| CopyValue::new(submitter));
    theme::use_theme();
    let directory = use_memo(move || {
        let programs = idls
            .read()
            .records()
            .iter()
            .map(|record| (record.program_id, record.name.clone()))
            .collect();
        std::sync::Arc::new(wallet_core::Directory::with_programs(
            &(pump.wallets)().summaries,
            (pump.book)(),
            (pump.holdings)(),
            programs,
        ))
    });
    use_context_provider(|| {
        Session::new(
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
            notices_open,
            notices_focus,
            directory,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus::dioxus_core::VirtualDom;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    #[derive(Clone)]
    struct Held {
        core: Core,
        form: Rc<Cell<Option<forms::WalletForm>>>,
    }

    fn app(held: Held) -> Element {
        let core = use_hook(|| CopyValue::new(held.core.clone()));
        let forms = use_forms();
        use_forgetting_on_lock(core, forms);
        held.form.set(Some(forms.wallet));
        rsx! {}
    }

    /// A lock the core made by itself, as auto-lock and the tray do.
    #[tokio::test]
    async fn a_lock_the_window_did_not_ask_for_wipes_what_add_wallet_holds() {
        let kit = app_core::testing::kit();
        kit.core.create_vault("pw").unwrap();
        let form = Rc::new(Cell::new(None));
        let mut dom = VirtualDom::new_with_props(
            app,
            Held {
                core: kit.core.clone(),
                form: form.clone(),
            },
        );
        dom.rebuild_in_place();
        let wallet = form.get().unwrap();
        dom.in_runtime(|| {
            let (mut secret, mut passphrase, mut open) =
                (wallet.secret, wallet.passphrase, wallet.modal_open);
            secret.set("twelve words".into());
            passphrase.set("and a passphrase".into());
            open.set(true);
        });

        kit.core.lock();
        let _ = tokio::time::timeout(Duration::from_millis(200), dom.wait_for_work()).await;

        dom.in_runtime(|| {
            assert!(wallet.secret.peek().is_empty());
            assert!(wallet.passphrase.peek().is_empty());
            assert!(!*wallet.modal_open.peek());
        });
    }
}
