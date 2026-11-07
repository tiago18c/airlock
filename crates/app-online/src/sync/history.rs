//! Transaction history: the latest transactions of each wallet, listed, and
//! each read in full once. What is kept is not asked for again, so a run only
//! reads what is new. Public data: runs while the vault is locked.

use std::sync::Arc;
use std::time::Duration;

use app_core::PublicCore;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use wallet_core::SolanaRpc;
use wallet_flows::history::{list, list_older, read, DETAILS_PER_READ, HISTORY_PAGE};

use super::balances::{note_answered, note_reach, wallet_addresses, RpcFn};
use super::scheduler::{Cancelled, JobCtx, JobFn, JobOk, JobResult, Scheduler, HISTORY_JOB};

/// How often the wallets' transactions are listed while nothing says one
/// happened. The activity feed and a transaction landing list them at once.
pub const HISTORY_INTERVAL: Duration = Duration::from_secs(120);

/// Register the history job on `scheduler`. `rpc` is usually
/// [`Monitor::rpc`](crate::Monitor::rpc).
pub fn register_history(scheduler: &Scheduler, rpc: RpcFn) {
    let work: JobFn = Arc::new(move |ctx| {
        let rpc = rpc.clone();
        Box::pin(async move { run_history(ctx, rpc).await })
    });
    scheduler.register(HISTORY_JOB, HISTORY_INTERVAL, work);
}

/// List the latest transactions of every wallet, then read in full those not
/// read yet, newest first, up to [`DETAILS_PER_READ`].
pub async fn run_history(ctx: JobCtx, rpc: RpcFn) -> JobResult {
    let addresses = wallet_addresses(&ctx.core.slices().wallets.get());
    if addresses.is_empty() {
        return Ok(JobOk::default());
    }
    let rpc = rpc().map_err(|err| err.to_string())?;
    let cluster = ctx.core.slices().network.get().cluster.clone();
    let now = (ctx.core.ports().clock.now_ms() / 1000) as i64;

    let listed = ctx
        .run_cancellable(list(rpc.as_ref(), &addresses, HISTORY_PAGE))
        .await
        .map_err(|Cancelled| "sync job was cancelled.".to_string())?;
    if ctx.is_cancelled() {
        return Err("sync job was cancelled.".into());
    }
    if !listed.addresses.is_empty() {
        note_answered(&ctx.core);
    }
    ctx.core.history_deferred("history_listed", |history| {
        let mut changed = false;
        for (address, items) in &listed.addresses {
            changed |= history.merge_page(&cluster, *address, items, HISTORY_PAGE, now);
        }
        changed
    });

    let wanted = ctx
        .core
        .slices()
        .history
        .get()
        .wanting_details(&cluster, DETAILS_PER_READ);
    let done = ctx
        .run_cancellable(read(rpc.as_ref(), &wanted))
        .await
        .map_err(|Cancelled| "sync job was cancelled.".to_string())?;
    if ctx.is_cancelled() {
        return Err("sync job was cancelled.".into());
    }
    let read_count = done.landed.len();
    ctx.core.history_deferred("history_landed", |history| {
        let mut changed = false;
        for (signature, landed) in done.landed {
            changed |= history.set_landed(&signature, &cluster, landed);
        }
        changed
    });

    // What was read stands, and the rest waits for the next run.
    if let Some(err) = &listed.stopped {
        note_reach(&ctx.core, err);
        if listed.addresses.is_empty() {
            return Err(err.to_string());
        }
        return Err(format!(
            "Listed the transactions of {} of {} wallets. {err}",
            listed.addresses.len(),
            addresses.len(),
        ));
    }
    if let Some(err) = &done.stopped {
        note_reach(&ctx.core, err);
        return Err(format!(
            "Read {read_count} of {} transactions in full. {err}",
            wanted.len(),
        ));
    }
    Ok(JobOk {
        note: Some(format!(
            "Listed the transactions of {} wallets, and read {read_count} in full.",
            addresses.len()
        )),
        ..JobOk::default()
    })
}

/// The page of each of `addresses` before the oldest kept, each read in full
/// at once so its row says what it did. An address whose first transaction
/// is kept, or that has not been listed yet, is left out. Answers how many
/// transactions were listed. What was read stands when a node stops it.
pub async fn load_older(
    core: &PublicCore,
    rpc: Arc<dyn SolanaRpc>,
    addresses: &[Pubkey],
) -> Result<usize, String> {
    let cluster = core.slices().network.get().cluster.clone();
    let from: Vec<(Pubkey, Signature)> = {
        let history = core.slices().history.get();
        addresses
            .iter()
            .filter(|address| !history.reached_start(&cluster, address))
            .filter_map(|address| Some((*address, history.oldest_listed(&cluster, address)?)))
            .collect()
    };
    if from.is_empty() {
        return Ok(0);
    }
    let listed = list_older(rpc.as_ref(), &from, HISTORY_PAGE).await;
    let found: Vec<Signature> = listed
        .addresses
        .iter()
        .flat_map(|(_, items)| items.iter().map(|item| item.signature))
        .collect();
    let now = (core.ports().clock.now_ms() / 1000) as i64;
    core.history_deferred("history_older", |history| {
        let mut changed = false;
        for (address, items) in &listed.addresses {
            changed |= history.merge_page(&cluster, *address, items, HISTORY_PAGE, now);
        }
        changed
    });

    let wanted: Vec<Signature> = {
        let history = core.slices().history.get();
        found
            .iter()
            .filter(|signature| {
                history
                    .get(signature)
                    .is_some_and(|entry| entry.wants_details())
            })
            .copied()
            .collect()
    };
    let done = read(rpc.as_ref(), &wanted).await;
    core.history_deferred("history_landed", |history| {
        let mut changed = false;
        for (signature, landed) in done.landed {
            changed |= history.set_landed(&signature, &cluster, landed);
        }
        changed
    });
    match listed.stopped.or(done.stopped) {
        Some(err) => Err(err.to_string()),
        None => Ok(found.len()),
    }
}

/// List the transactions again as soon as a wallet is added, so its history
/// does not wait for the next run.
pub fn watch_wallets(core: PublicCore, scheduler: Scheduler) {
    let mut wallets = core.slices().wallets.subscribe();
    core.ports().spawner.spawn(Box::pin(async move {
        let mut last = wallet_addresses(&wallets.borrow_and_update());
        while wallets.changed().await.is_ok() {
            let now = wallet_addresses(&wallets.borrow_and_update());
            if now.iter().any(|address| !last.contains(address)) {
                scheduler.kick(HISTORY_JOB);
            }
            last = now;
        }
    }));
}
