//! Token logos for the portfolio, downloaded by the prices job once its
//! prices are applied. Each logo is downloaded once, ever, and kept in the
//! data folder; the holdings cache records that it was asked for, so it is
//! never asked again. A logo never fails or holds up a price refresh. The
//! browser build downloads none and keeps the lettered avatars.

#[cfg(not(target_arch = "wasm32"))]
pub(super) use native::{fetch_logos, LogoTries};
#[cfg(target_arch = "wasm32")]
pub(super) use web::{fetch_logos, LogoTries};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, PoisonError};

    use solana_pubkey::Pubkey;
    use tracing::warn;
    use wallet_core::portfolio::{
        LogoFetch, LogoState, MarketSource, LOGOS_PER_RUN, WRAPPED_SOL_MINT,
    };

    use crate::sync::scheduler::JobCtx;

    /// Tries a logo gets while the app runs when no answer can be had: the
    /// first, and one more on a later run.
    const UNREACHABLE_TRIES: u8 = 2;

    /// The logos that could not be reached, by mint, with how often. Kept by
    /// the job for as long as the app runs, and never saved: the next start
    /// tries them again.
    #[derive(Clone, Default)]
    pub(in crate::sync) struct LogoTries(Arc<Mutex<HashMap<Pubkey, u8>>>);

    impl LogoTries {
        fn tries_left(&self, mint: &Pubkey) -> bool {
            let tries = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            tries.get(mint).copied().unwrap_or(0) < UNREACHABLE_TRIES
        }

        fn missed(&self, mint: Pubkey) {
            let mut tries = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            *tries.entry(mint).or_default() += 1;
        }
    }

    /// Download the logos of `mints`, and of wrapped SOL, that the holdings
    /// cache names and has never asked for: at most [`LOGOS_PER_RUN`], one at
    /// a time, in the order the mints are held. Each is recorded as it comes,
    /// so the Portfolio shows it without waiting for the rest. Stops when the
    /// run is cancelled.
    pub(in crate::sync) async fn fetch_logos(
        ctx: &JobCtx,
        source: &dyn MarketSource,
        mints: &[Pubkey],
        tries: &LogoTries,
    ) {
        let considered: Vec<Pubkey> = std::iter::once(WRAPPED_SOL_MINT)
            .chain(mints.iter().copied())
            .filter(|mint| tries.tries_left(mint))
            .collect();
        let wanted = ctx
            .core
            .slices()
            .holdings
            .get()
            .market
            .logos_wanted(&considered, LOGOS_PER_RUN);
        for (mint, url) in wanted {
            let Ok(fetched) = ctx.run_cancellable(source.logo(&url)).await else {
                return;
            };
            if ctx.is_cancelled() {
                return;
            }
            let state = match fetched {
                LogoFetch::Image { kind, bytes } => {
                    let storage = ctx.core.ports().storage.as_ref();
                    // The entry is written only once the file is there, so
                    // one that failed is asked for again.
                    if let Err(err) = app_core::storage::save_logo(storage, &mint, kind, &bytes) {
                        warn!(target: "airlock::sync", %mint, error = %err, "could not keep a token logo");
                        tries.missed(mint);
                        continue;
                    }
                    LogoState::Kept { kind }
                }
                LogoFetch::Unusable => LogoState::Nothing,
                LogoFetch::Unreachable => {
                    tries.missed(mint);
                    continue;
                }
            };
            ctx.core
                .holdings_deferred("logo", |holdings| holdings.market.set_logo(&mint, state));
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use solana_pubkey::Pubkey;
    use wallet_core::portfolio::MarketSource;

    use crate::sync::scheduler::JobCtx;

    /// Nothing to remember: the browser build downloads no logos. Not a unit
    /// struct, so the job makes it as it does the native one.
    #[derive(Clone, Default)]
    pub(in crate::sync) struct LogoTries(());

    /// The browser build keeps no files, so it downloads no logos.
    pub(in crate::sync) async fn fetch_logos(
        _ctx: &JobCtx,
        _source: &dyn MarketSource,
        _mints: &[Pubkey],
        _tries: &LogoTries,
    ) {
    }
}
