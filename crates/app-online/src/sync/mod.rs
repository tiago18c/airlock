//! Background sync: a scheduler that runs jobs on an interval with backoff,
//! cancels them when the cluster or RPC URL changes, and publishes [`SyncStatus`](app_core::SyncStatus).
//!
//! Three jobs read the chain: SOL balances, the token accounts of the wallet
//! in use, and those of every other address. The activity feed listens on
//! the node's WebSocket for each wallet's balance, shown as it changes, and
//! for transactions that mention a wallet, which have the balances job read
//! again. So does a transaction the app sent landing.

mod activity;
mod balances;
mod prices;
mod scheduler;
mod tokens;

pub use activity::{
    connect_to_node, start_activity_feed, ActivityEvents, ConnectFn, Moved, CONNECT_TIMEOUT,
    FIRST_RETRY, MAX_RETRY,
};
pub use balances::{
    current_owner, kick_after_probe, read_after_landing, refresh_from_chain_fn, refresh_note,
    refresh_targets, register_balances, wallet_addresses, BALANCES_INTERVAL, BEHIND_TRIES,
    BEHIND_WAIT, FEED_UP_BALANCES_INTERVAL,
};
pub use prices::{
    portfolio_addresses, register_prices, register_prices_with, SourceFn, PRICES_INTERVAL,
    PRICES_JOB,
};
pub use scheduler::{
    Cancelled, JobCtx, JobFn, JobOk, JobResult, Scheduler, Trigger, BALANCES_JOB,
    CURRENT_TOKENS_JOB, INITIAL_BACKOFF, MAX_BACKOFF, TOKENS_JOB,
};
pub use tokens::{register_tokens, watch_selection, CURRENT_TOKENS_INTERVAL, TOKENS_INTERVAL};

#[cfg(test)]
mod tests;
