//! Token accounts: often for the wallet in use, and by turns for every other
//! address read, wallets first. SOL balances are the balances job's.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use app_core::PublicCore;
use solana_pubkey::Pubkey;
use wallet_core::AddressKind;
use wallet_flows::refresh::{read_tokens, TokenTurn, TokenTurns};

use super::balances::{
    apply_snapshot, current_owner, note_answered, note_reach, wallet_addresses, RpcFn,
};
use super::scheduler::{
    Cancelled, JobCtx, JobFn, JobOk, JobResult, Scheduler, CURRENT_TOKENS_JOB, TOKENS_JOB,
};

/// How often the token accounts of the wallet in use are read.
pub const CURRENT_TOKENS_INTERVAL: Duration = Duration::from_secs(15);

/// How often the token accounts of some of the other addresses are read.
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
        let turns = Arc::new(Mutex::new(TokenTurns::default()));
        let work: JobFn = Arc::new(move |ctx| {
            let (rpc, turns) = (rpc.clone(), turns.clone());
            Box::pin(async move { run_tokens(ctx, rpc, whose, turns).await })
        });
        scheduler.register(name, interval, work);
    }
}

async fn run_tokens(
    ctx: JobCtx,
    rpc: RpcFn,
    whose: Whose,
    turns: Arc<Mutex<TokenTurns>>,
) -> JobResult {
    let turn = turn_for(&ctx.core, whose, &lock(&turns));
    if turn.owners.is_empty() {
        return Ok(JobOk::default());
    }
    let rpc = rpc().map_err(|err| err.to_string())?;
    let read = match ctx
        .run_cancellable(read_tokens(rpc.as_ref(), &turn.owners))
        .await
    {
        Ok(Ok(read)) => read,
        Ok(Err(err)) => {
            note_reach(&ctx.core, &err);
            return Err(err.to_string());
        }
        Err(Cancelled) => return Err("sync job was cancelled.".into()),
    };
    if ctx.is_cancelled() {
        return Err("sync job was cancelled.".into());
    }
    apply_snapshot(&ctx.core, &read.snapshot);
    lock(&turns).advance(&turn, read.asked);
    if let Some(err) = &read.stopped {
        note_reach(&ctx.core, err);
        return Err(err.to_string());
    }
    note_answered(&ctx.core);
    let refusal = read.refusal();
    match refusal {
        // Nothing came back, which a run should not call done.
        Some(refusal) if read.refused == turn.owners.len() => Err(refusal),
        partial => Ok(JobOk {
            partial,
            ..JobOk::default()
        }),
    }
}

fn lock(turns: &Mutex<TokenTurns>) -> std::sync::MutexGuard<'_, TokenTurns> {
    turns.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whose token accounts this run reads.
fn turn_for(core: &PublicCore, whose: Whose, turns: &TokenTurns) -> TokenTurn {
    let wallets = core.slices().wallets.get();
    let current = current_owner(&wallets);
    match whose {
        Whose::Current => turns.next(&current.into_iter().collect::<Vec<_>>(), &[]),
        Whose::Others => {
            let mine: Vec<Pubkey> = wallet_addresses(&wallets)
                .into_iter()
                .filter(|owner| Some(*owner) != current)
                .collect();
            let book: Vec<Pubkey> = core
                .slices()
                .book
                .get()
                .iter()
                .filter(|entry| matches!(entry.kind, AddressKind::Account))
                .map(|entry| entry.pubkey)
                .filter(|owner| Some(*owner) != current && !mine.contains(owner))
                .collect();
            turns.next(&mine, &book)
        }
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
