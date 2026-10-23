//! SOL balances for addresses in the book and the wallets' addresses, and
//! the token accounts of an address the activity feed saw move. Public data:
//! runs while the vault is locked.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use app_core::{PublicCore, Reach, WalletsView};
use solana_pubkey::Pubkey;
use wallet_core::{AddressBook, AddressKind, ChainSnapshot, Cluster, RpcError, SolanaRpc};
use wallet_flows::refresh::{read_balances_since, read_tokens_since};

use crate::network::Monitor;

use super::activity::Moved;
use super::scheduler::{
    Cancelled, JobCtx, JobFn, JobOk, JobResult, Scheduler, Trigger, BALANCES_JOB,
};

/// How often balances are read while the activity feed is down, so nothing
/// says when they change.
pub const BALANCES_INTERVAL: Duration = Duration::from_secs(30);

/// How often balances are read while the activity feed is up, in case it
/// missed something.
pub const FEED_UP_BALANCES_INTERVAL: Duration = Duration::from_secs(300);

/// How many times a node behind the slot the feed told of is asked again.
pub const BEHIND_TRIES: u32 = 5;

/// How long to wait before asking a node that is behind again: about a slot.
pub const BEHIND_WAIT: Duration = Duration::from_millis(400);

pub(super) type RpcFn = Arc<dyn Fn() -> Result<Arc<dyn SolanaRpc>, RpcError> + Send + Sync>;

/// Register the balances job on `scheduler`. `rpc` is usually [`Monitor::rpc`].
pub fn register_balances(scheduler: &Scheduler, rpc: RpcFn, moved: Moved) {
    let work: JobFn = Arc::new(move |ctx| {
        let (rpc, moved) = (rpc.clone(), moved.clone());
        Box::pin(async move { run_balances(ctx, rpc, moved).await })
    });
    scheduler.register(BALANCES_JOB, BALANCES_INTERVAL, work);
}

/// Read the balance of every address in one request per hundred, and the
/// token accounts of each address the feed saw move. Pressing Refresh reads
/// the token accounts of every address too. Both are read from a node that
/// has seen the newest slot the feed told of, and balances are shown before
/// the token accounts are read. Returns the Refresh note.
pub async fn run_balances(ctx: JobCtx, rpc: RpcFn, moved: Moved) -> JobResult {
    let rpc = rpc().map_err(|err| err.to_string())?;
    let cluster = ctx.core.slices().network.get().cluster.clone();
    let owners = refresh_targets(
        &ctx.core.slices().book.get(),
        &ctx.core.slices().wallets.get(),
    );
    let token_owners = match ctx.trigger {
        Trigger::User => {
            moved.take();
            owners.clone()
        }
        Trigger::Schedule => moved.take(),
    };
    let since = moved.slot(&cluster);

    let read = ctx
        .run_cancellable(read_since(&ctx, since, |min| {
            read_balances_since(rpc.as_ref(), &owners, min)
        }))
        .await;
    let (slot, balances) = match read {
        Ok(Ok(read)) => read,
        Ok(Err(err)) => {
            moved.mark_all(&token_owners);
            note_reach(&ctx.core, &err);
            return Err(err.to_string());
        }
        Err(Cancelled) => return Err("sync job was cancelled.".into()),
    };
    if ctx.is_cancelled() {
        return Err("sync job was cancelled.".into());
    }
    if !owners.is_empty() {
        note_answered(&ctx.core);
    }
    apply_snapshot(
        &ctx.core,
        &ChainSnapshot {
            balances: moved.fresher(&cluster, slot, balances),
            ..ChainSnapshot::default()
        },
    );

    let read = ctx
        .run_cancellable(read_since(&ctx, since, |min| {
            read_tokens_since(rpc.as_ref(), &token_owners, min)
        }))
        .await;
    let tokens = match read {
        Ok(Ok(tokens)) => tokens,
        Ok(Err(err)) => {
            moved.mark_all(&token_owners);
            note_reach(&ctx.core, &err);
            return Err(err.to_string());
        }
        Err(Cancelled) => return Err("sync job was cancelled.".into()),
    };
    if ctx.is_cancelled() {
        return Err("sync job was cancelled.".into());
    }
    apply_snapshot(&ctx.core, &tokens);

    let note = format!(
        "Refreshed {} owners, {} token accounts.",
        owners.len(),
        tokens.tokens.len(),
    );
    Ok(JobOk { note: Some(note) })
}

/// `read` from a node that has seen `since`. One that is behind it is asked
/// again [`BEHIND_TRIES`] times, [`BEHIND_WAIT`] apart, then read from as it
/// is: a balance it reads older than one shown is not shown.
async fn read_since<T, F, Fut>(ctx: &JobCtx, since: u64, read: F) -> Result<T, RpcError>
where
    F: Fn(u64) -> Fut,
    Fut: Future<Output = Result<T, RpcError>>,
{
    if since > 0 {
        for _ in 0..BEHIND_TRIES {
            match read(since).await {
                Err(RpcError::Behind) => ctx.core.ports().clock.sleep(BEHIND_WAIT).await,
                other => return other,
            }
        }
    }
    read(0).await
}

/// The node answered, whatever the last probe said.
pub(super) fn note_answered(core: &PublicCore) {
    if core.slices().network.get().reach != Reach::Answered {
        core.set_reach(Reach::Answered);
    }
}

/// A node that did not answer at all is shown as unreachable.
pub(super) fn note_reach(core: &PublicCore, err: &RpcError) {
    if err.is_down() && core.slices().network.get().reach != Reach::Unreachable {
        core.set_reach(Reach::Unreachable);
    }
}

pub(super) fn apply_snapshot(core: &PublicCore, snap: &ChainSnapshot) {
    core.workspace_deferred("apply_snapshot", |ws| ws.book.apply_snapshot(snap));
    if !snap.token_owners.is_empty() {
        core.holdings_deferred("replace_owners", |holdings| {
            holdings.replace_owners(&snap.token_owners, &snap.tokens)
        });
    }
}

/// The addresses read from the chain: accounts in the book, then each
/// wallet's address.
pub fn refresh_targets(book: &AddressBook, wallets: &WalletsView) -> Vec<Pubkey> {
    let mut owners: Vec<Pubkey> = book
        .iter()
        .filter(|e| matches!(e.kind, AddressKind::Account))
        .map(|e| e.pubkey)
        .collect();
    for pk in wallet_addresses(wallets) {
        if !owners.contains(&pk) {
            owners.push(pk);
        }
    }
    owners
}

/// Each wallet's address, once.
pub fn wallet_addresses(wallets: &WalletsView) -> Vec<Pubkey> {
    let mut addresses = Vec::new();
    for pk in wallets.summaries.iter().filter_map(|wallet| wallet.address) {
        if !addresses.contains(&pk) {
            addresses.push(pk);
        }
    }
    addresses
}

/// The address of the wallet in use.
pub fn current_owner(wallets: &WalletsView) -> Option<Pubkey> {
    let selected = wallets.selected?;
    wallets
        .summaries
        .iter()
        .find(|wallet| wallet.id == selected)
        .and_then(|wallet| wallet.address)
}

/// Format a `run_now` result the way Address Book's Refresh expects.
pub fn refresh_note(result: JobResult) -> Result<String, String> {
    match result {
        Ok(ok) => Ok(ok
            .note
            .unwrap_or_else(|| "Refreshed 0 owners, 0 token accounts.".into())),
        Err(err) => Err(sentence(&err)),
    }
}

/// Match `app_ui_kit::actions::sentence` so Refresh errors stay worded the same.
fn sentence(err: &str) -> String {
    let mut chars = err.chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return err.to_string(),
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// Build the async work OnlineHooks plugs in for "Refresh from chain".
pub fn refresh_from_chain_fn(
    scheduler: Scheduler,
) -> impl Fn() -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync {
    move || {
        let scheduler = scheduler.clone();
        Box::pin(async move { refresh_note(scheduler.run_now(BALANCES_JOB).await) })
    }
}

/// What the tracker runs when a transaction it follows lands. On the
/// cluster in use, the balances job reads again from a node that has seen
/// the transaction, with the token accounts of the wallet in use.
pub fn read_after_landing(
    core: PublicCore,
    scheduler: Scheduler,
    moved: Moved,
) -> impl Fn(&Cluster, Option<u64>) + Send + Sync + 'static {
    move |cluster, slot| {
        if core.slices().network.get().cluster != *cluster {
            return;
        }
        if let Some(slot) = slot {
            moved.saw(cluster, slot);
        }
        if let Some(owner) = current_owner(&core.slices().wallets.get()) {
            moved.mark(owner);
        }
        scheduler.kick(BALANCES_JOB);
    }
}

/// After a successful probe, or when asked to read again, run every job.
pub fn kick_after_probe(monitor: &Monitor, scheduler: &Scheduler) {
    let scheduler = scheduler.clone();
    monitor.set_on_online(move || scheduler.kick_all());
}
