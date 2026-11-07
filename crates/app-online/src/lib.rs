//! Online-only UI and services: the network monitor over `wallet-rpc`,
//! submitter/tracker, sync scheduler, Stake, and Proposals. The airgap shell
//! does not link this.

pub mod actions;
pub mod network;
pub mod sync;
pub mod tx;
pub mod ui;

pub use network::{rpc_for_cluster, Monitor};
pub use sync::{
    kick_after_probe, refresh_from_chain_fn, register_balances, register_prices, register_tokens,
    Scheduler, BALANCES_JOB, PRICES_JOB,
};
pub use tx::{Submitter, Tracker};
pub use ui::{HistoryPanel, NetworkChip, PortfolioPanel, ProposalsHost, SendHost, StakePanel};

use std::sync::Arc;

use app_core::{Core, PublicCore};
use app_ui_kit::{OnlineHooks, SubmitHandle};
use dioxus::prelude::*;

/// The sync scheduler, for a pane that asks a job to run now, and the core
/// it writes through, for one that reads more than a job does.
#[derive(Clone)]
pub struct Jobs {
    pub scheduler: Scheduler,
    pub core: PublicCore,
}

/// The online services over `core`: the network monitor, the sync scheduler
/// with its balances and history jobs, and the submitter with its tracker,
/// which picks up transactions still being followed. Returns what the shell
/// hands [`app_ui_kit::use_session`]. Call once, from the root.
pub fn use_online(core: CopyValue<Core>) -> (OnlineHooks, SubmitHandle) {
    let (hooks, submitter, jobs) = use_hook(|| {
        let public = core.peek().public();
        let monitor = Monitor::new(public.clone());
        let scheduler = Scheduler::new(public.clone());
        let rpc: Arc<dyn Fn() -> _ + Send + Sync> = {
            let monitor = monitor.clone();
            Arc::new(move || monitor.rpc())
        };
        let moved = sync::Moved::default();
        register_balances(&scheduler, rpc.clone(), moved.clone());
        register_tokens(&scheduler, rpc.clone());
        sync::register_history(&scheduler, rpc);
        register_prices(&scheduler);
        kick_after_probe(&monitor, &scheduler);
        scheduler.start();
        sync::watch_selection(public.clone(), scheduler.clone());
        sync::watch_wallets(public.clone(), scheduler.clone());
        sync::start_activity_feed(
            public.clone(),
            scheduler.clone(),
            moved.clone(),
            sync::connect_to_node(),
        );

        let tracker = Tracker::new(public.clone());
        tracker.set_on_landed(sync::read_after_landing(
            public.clone(),
            scheduler.clone(),
            moved,
        ));
        tracker.resume(rpc_for_cluster);
        let submitter = SubmitHandle::new(Submitter::new(public.clone(), tracker));

        let hooks = OnlineHooks::new(
            {
                let monitor = monitor.clone();
                move || monitor.rpc()
            },
            {
                let monitor = monitor.clone();
                move || monitor.start_detect()
            },
            move || monitor.start_refresh(),
            refresh_from_chain_fn(scheduler.clone()),
        )
        .with_refresh_prices({
            let scheduler = scheduler.clone();
            move || {
                let scheduler = scheduler.clone();
                Box::pin(async move {
                    scheduler
                        .run_now(PRICES_JOB)
                        .await
                        .map(|ok| ok.note.unwrap_or_default())
                })
            }
        });
        let jobs = Jobs {
            scheduler,
            core: public,
        };
        (hooks, submitter, jobs)
    });
    use_context_provider(|| jobs);
    (hooks, submitter)
}
