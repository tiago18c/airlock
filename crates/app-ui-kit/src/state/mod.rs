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
pub use submit::{SubmitFut, SubmitHandle, TxSubmit};

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

/// The session over `core`, provided as context for every screen: the
/// pumped slices, and the screen state a window holds (forms, tab, gate,
/// banners). `online` and `submitter` are what the shell plugs in; a shell
/// with no network passes [`OnlineHooks::none`] and
/// [`SubmitHandle::none`](crate::SubmitHandle::none). Call once, from the root.
pub fn use_session(core: CopyValue<Core>, online: OnlineHooks, submitter: SubmitHandle) -> Session {
    let pump = pump::use_core(core.peek().clone());
    let idls = use_signal(crate::platform::storage::open_idls);
    let idl_misses = use_hook(|| CopyValue::new(std::collections::HashSet::new()));
    let leaf_marks = use_signal(crate::platform::storage::open_leaf_marks);
    let xmss_retries = use_signal(wallet_core::vector::XmssRetryCache::default);
    let tab = use_signal(|| Tab::Wallets);
    let forms = use_forms();
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
