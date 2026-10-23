//! Token accounts: often for the wallet in use, less often for every other
//! address read. SOL balances are the balances job's.

use std::sync::Arc;
use std::time::Duration;

use app_core::PublicCore;
use solana_pubkey::Pubkey;
use wallet_flows::refresh::read_tokens;

use super::balances::{
    apply_snapshot, current_owner, note_answered, note_reach, refresh_targets, RpcFn,
};
use super::scheduler::{
    Cancelled, JobCtx, JobFn, JobOk, JobResult, Scheduler, CURRENT_TOKENS_JOB, TOKENS_JOB,
};

/// How often the token accounts of the wallet in use are read.
pub const CURRENT_TOKENS_INTERVAL: Duration = Duration::from_secs(15);

/// How often the token accounts of every other address are read.
pub const TOKENS_INTERVAL: Duration = Duration::from_secs(30);

/// Whose token accounts a run reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Whose {
    /// The wallet in use.
    Current,
    /// Every other address read.
    Others,
}

/// Register both token jobs on `scheduler`.
pub fn register_tokens(scheduler: &Scheduler, rpc: RpcFn) {
    for (name, interval, whose) in [
        (CURRENT_TOKENS_JOB, CURRENT_TOKENS_INTERVAL, Whose::Current),
        (TOKENS_JOB, TOKENS_INTERVAL, Whose::Others),
    ] {
        let rpc = rpc.clone();
        let work: JobFn = Arc::new(move |ctx| {
            let rpc = rpc.clone();
            Box::pin(async move { run_tokens(ctx, rpc, whose).await })
        });
        scheduler.register(name, interval, work);
    }
}

async fn run_tokens(ctx: JobCtx, rpc: RpcFn, whose: Whose) -> JobResult {
    let owners = owners_for(&ctx.core, whose);
    if owners.is_empty() {
        return Ok(JobOk::default());
    }
    let rpc = rpc().map_err(|err| err.to_string())?;
    let snap = match ctx
        .run_cancellable(read_tokens(rpc.as_ref(), &owners))
        .await
    {
        Ok(Ok(snap)) => snap,
        Ok(Err(err)) => {
            note_reach(&ctx.core, &err);
            return Err(err.to_string());
        }
        Err(Cancelled) => return Err("sync job was cancelled.".into()),
    };
    if ctx.is_cancelled() {
        return Err("sync job was cancelled.".into());
    }
    note_answered(&ctx.core);
    apply_snapshot(&ctx.core, &snap);
    Ok(JobOk::default())
}

fn owners_for(core: &PublicCore, whose: Whose) -> Vec<Pubkey> {
    let wallets = core.slices().wallets.get();
    let current = current_owner(&wallets);
    match whose {
        Whose::Current => current.into_iter().collect(),
        Whose::Others => refresh_targets(&core.slices().book.get(), &wallets)
            .into_iter()
            .filter(|owner| Some(*owner) != current)
            .collect(),
    }
}

/// Read the token accounts of the wallet in use as soon as another one is
/// chosen.
pub fn watch_selection(core: PublicCore, scheduler: Scheduler) {
    let mut wallets = core.slices().wallets.subscribe();
    core.ports().spawner.spawn(Box::pin(async move {
        let mut last = current_owner(&wallets.borrow_and_update());
        while wallets.changed().await.is_ok() {
            let now = current_owner(&wallets.borrow_and_update());
            if now != last {
                last = now;
                if now.is_some() {
                    scheduler.kick(CURRENT_TOKENS_JOB);
                }
            }
        }
    }));
}
