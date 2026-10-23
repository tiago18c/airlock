//! One refresh of the market cache: prices for the mints held here, the asset
//! groups once a day, and a few token checks.

use solana_pubkey::Pubkey;
use wallet_core::portfolio::{MarketCache, MarketError, MarketSource, MarketUpdate};

/// Ask `source` for what `cache` lacks or holds stale for `mints`, at Unix
/// second `now`. Fails only when the prices could not be read; a failed group
/// list or check is kept in the update's `error`, and the cache keeps what it
/// had for them.
pub async fn refresh(
    source: &dyn MarketSource,
    cache: &MarketCache,
    mints: &[Pubkey],
    now: i64,
) -> Result<MarketUpdate, MarketError> {
    let plan = cache.plan(mints, now);
    let mut update = MarketUpdate::default();
    if !plan.quote.is_empty() {
        update.quotes = Some(source.quotes(&plan.quote).await?);
    }
    if plan.groups {
        match source.groups().await {
            Ok(groups) => update.groups = Some(groups),
            Err(err @ MarketError::Key) => return Err(err),
            Err(err) => update.error = Some(err.to_string()),
        }
    }
    for mint in &plan.check {
        match source.check(mint).await {
            Ok(check) => update.checks.push((*mint, check)),
            // The rest wait for the next run.
            Err(MarketError::RateLimited) => break,
            Err(err) => {
                update.error.get_or_insert(err.to_string());
                break;
            }
        }
    }
    Ok(update)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wallet_core::portfolio::{AssetGroup, MarketQuote, TokenCheck, WRAPPED_SOL_MINT};

    #[derive(Default)]
    struct Fake {
        asked: Mutex<Vec<String>>,
        limit_checks: bool,
    }

    #[async_trait::async_trait]
    impl MarketSource for Fake {
        async fn quotes(
            &self,
            mints: &[Pubkey],
        ) -> Result<Vec<(Pubkey, Option<MarketQuote>)>, MarketError> {
            self.asked
                .lock()
                .unwrap()
                .push(format!("quotes {}", mints.len()));
            Ok(mints
                .iter()
                .map(|mint| {
                    (
                        *mint,
                        Some(MarketQuote {
                            price_usd: Some(1.0),
                            ..Default::default()
                        }),
                    )
                })
                .collect())
        }

        async fn groups(&self) -> Result<Vec<(Pubkey, AssetGroup)>, MarketError> {
            self.asked.lock().unwrap().push("groups".into());
            Ok(Vec::new())
        }

        async fn check(&self, _mint: &Pubkey) -> Result<TokenCheck, MarketError> {
            self.asked.lock().unwrap().push("check".into());
            if self.limit_checks {
                return Err(MarketError::RateLimited);
            }
            Ok(TokenCheck {
                verified: true,
                ..Default::default()
            })
        }
    }

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    #[test]
    fn a_first_refresh_asks_for_everything_and_a_second_for_nothing_fresh() {
        let source = Fake::default();
        let mut cache = MarketCache::default();
        let update = pollster::block_on(refresh(&source, &cache, &[pk(1), pk(2)], 100)).unwrap();
        cache.apply(update, 100);
        assert!(cache.quote(&WRAPPED_SOL_MINT).is_some());
        assert!(cache.check(&pk(2)).is_some_and(|check| check.verified));
        assert_eq!(
            *source.asked.lock().unwrap(),
            ["quotes 3", "groups", "check", "check"]
        );

        let again = Fake::default();
        let update = pollster::block_on(refresh(&again, &cache, &[pk(1), pk(2)], 130)).unwrap();
        assert_eq!(update, MarketUpdate::default());
        assert!(again.asked.lock().unwrap().is_empty());
    }

    #[test]
    fn a_rate_limit_on_checks_keeps_the_prices() {
        let source = Fake {
            limit_checks: true,
            ..Default::default()
        };
        let update = pollster::block_on(refresh(
            &source,
            &MarketCache::default(),
            &[pk(1), pk(2)],
            0,
        ))
        .unwrap();
        assert!(update.quotes.is_some());
        assert!(update.checks.is_empty());
        // One check was tried, then the rest waited.
        assert_eq!(
            source
                .asked
                .lock()
                .unwrap()
                .iter()
                .filter(|a| *a == "check")
                .count(),
            1
        );
    }
}
