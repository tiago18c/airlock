//! Prices for the portfolio: what the Tokens API says about the mints held by
//! the wallets and the watched addresses. Public data: runs while the vault is
//! locked, as long as a key is set and the cluster is mainnet.

use std::sync::Arc;
use std::time::Duration;

use solana_pubkey::Pubkey;
use wallet_core::portfolio::{prices_apply_to, MarketSource};
use wallet_core::{AddressBook, AddressKind};

use super::scheduler::{Cancelled, JobCtx, JobFn, JobOk, JobResult, Scheduler};

/// Job id for portfolio prices.
pub const PRICES_JOB: &str = "prices";

/// How often prices are refreshed.
pub const PRICES_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Makes the service for a key. The app uses the Tokens API; tests a fake.
pub type SourceFn = Arc<dyn Fn(String) -> Arc<dyn MarketSource> + Send + Sync>;

/// Register the prices job on `scheduler`, over the Tokens API.
pub fn register_prices(scheduler: &Scheduler) {
    register_prices_with(
        scheduler,
        Arc::new(|key| Arc::new(tokens_api::TokensApi::new(key)) as Arc<dyn MarketSource>),
    );
}

/// [`register_prices`] with another service.
pub fn register_prices_with(scheduler: &Scheduler, source: SourceFn) {
    let work: JobFn = Arc::new(move |ctx| {
        let source = source.clone();
        Box::pin(async move { run_prices(ctx, source).await })
    });
    scheduler.register(PRICES_JOB, PRICES_INTERVAL, work);
}

async fn run_prices(ctx: JobCtx, source: SourceFn) -> JobResult {
    let slices = ctx.core.slices();
    let Some(key) = slices.settings.get().tokens_api_key.clone() else {
        return Ok(JobOk {
            note: Some("No Tokens API key: prices are off.".into()),
        });
    };
    if !prices_apply_to(&slices.network.get().cluster) {
        return Ok(JobOk {
            note: Some("Prices are for mainnet only.".into()),
        });
    }
    let owners = portfolio_addresses(&slices.book.get(), &slices.wallets.get());
    let holdings = slices.holdings.get();
    let mints = holdings.mints_of(&owners);
    let now = (ctx.core.ports().clock.now_ms() / 1000) as i64;
    let source = source(key.0);
    let refreshed = ctx
        .run_cancellable(wallet_flows::market::refresh(
            source.as_ref(),
            &holdings.market,
            &mints,
            now,
        ))
        .await;
    if ctx.is_cancelled() {
        return Err("sync job was cancelled.".into());
    }
    match refreshed {
        Ok(Ok(update)) => {
            let priced = update.quotes.as_ref().map_or(0, Vec::len);
            ctx.core
                .holdings_deferred("market", |holdings| holdings.market.apply(update, now));
            Ok(JobOk {
                note: Some(format!("Priced {priced} tokens.")),
            })
        }
        Ok(Err(err)) => {
            let message = err.to_string();
            let shown = message.clone();
            ctx.core.holdings_deferred("market_error", |holdings| {
                holdings.market.error = Some(shown)
            });
            Err(message)
        }
        Err(Cancelled) => Err("sync job was cancelled.".into()),
    }
}

/// The addresses the portfolio counts: every wallet with an address, and
/// every account in the book marked as watched.
pub fn portfolio_addresses(book: &AddressBook, wallets: &app_core::WalletsView) -> Vec<Pubkey> {
    let mut out: Vec<Pubkey> = wallets
        .summaries
        .iter()
        .filter_map(|wallet| wallet.address)
        .collect();
    for entry in book.iter() {
        if entry.watched && entry.kind == AddressKind::Account && !out.contains(&entry.pubkey) {
            out.push(entry.pubkey);
        }
    }
    out
}
