//! What a market data service says about the tokens held here: a price, the
//! canonical asset a mint belongs to, and whether the mint is the token it
//! claims to be. Kept in the holdings cache, refreshed by a sync job.

use crate::cluster::{Cluster, KnownCluster};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;

/// The mint that prices SOL.
pub const WRAPPED_SOL_MINT: Pubkey =
    Pubkey::from_str_const("So11111111111111111111111111111111111111112");

/// How long a list of asset groups is used before it is read again.
pub const GROUPS_TTL_SECS: i64 = 24 * 60 * 60;
/// How long a mint's check stays good.
pub const CHECK_TTL_SECS: i64 = 7 * 24 * 60 * 60;
/// A price younger than this is not asked again.
pub const QUOTE_FRESH_SECS: i64 = 60;
/// Mints checked in one refresh. Checks are one call each, and the service
/// allows about 120 every 10 minutes.
pub const CHECKS_PER_RUN: usize = 20;

/// Price and identity of one mint. Fields the service did not give are
/// `None`, never zero.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketQuote {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// USD for one whole token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_24h_pct: Option<f64>,
}

// Prices come from JSON numbers, never NaN.
impl Eq for MarketQuote {}

/// The canonical asset a curated mint is a variant of, such as "usd" for
/// USDC and USDT.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetGroup {
    pub asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// A liquid staking token. It is shown with SOL.
    #[serde(default)]
    pub lst: bool,
}

/// What the service found when asked whether a mint is what it claims.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCheck {
    pub verified: bool,
    /// The service's warning codes, such as `possible_impersonation`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub checked_at: i64,
}

impl TokenCheck {
    /// The service thinks this mint passes itself off as another token.
    pub fn impersonates(&self) -> bool {
        self.warnings.iter().any(|w| w == "possible_impersonation")
    }
}

/// Everything the service said, by mint, with when it was said.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketCache {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quotes: BTreeMap<String, MarketQuote>,
    /// Unix seconds of the last price call that answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, AssetGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grouped_at: Option<i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checks: BTreeMap<String, TokenCheck>,
    /// The last refresh's failure, for the screen. Cleared by a success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl MarketCache {
    pub fn quote(&self, mint: &Pubkey) -> Option<&MarketQuote> {
        self.quotes.get(&mint.to_string())
    }

    pub fn group(&self, mint: &Pubkey) -> Option<&AssetGroup> {
        self.groups.get(&mint.to_string())
    }

    pub fn check(&self, mint: &Pubkey) -> Option<&TokenCheck> {
        self.checks.get(&mint.to_string())
    }

    /// What a refresh at `now` should ask for, for the mints held here.
    pub fn plan(&self, mints: &[Pubkey], now: i64) -> MarketPlan {
        let mut quote: Vec<Pubkey> = Vec::new();
        let fresh = self.quoted_at.is_some_and(|at| now - at < QUOTE_FRESH_SECS);
        for mint in std::iter::once(&WRAPPED_SOL_MINT).chain(mints) {
            if quote.contains(mint) {
                continue;
            }
            if fresh && self.quote(mint).is_some() {
                continue;
            }
            quote.push(*mint);
        }
        let groups = self.grouped_at.is_none_or(|at| now - at >= GROUPS_TTL_SECS);
        let mut check: Vec<Pubkey> = mints
            .iter()
            .filter(|mint| **mint != WRAPPED_SOL_MINT)
            .filter(|mint| {
                self.check(mint)
                    .is_none_or(|found| now - found.checked_at >= CHECK_TTL_SECS)
            })
            .copied()
            .collect();
        check.dedup();
        check.truncate(CHECKS_PER_RUN);
        MarketPlan {
            quote,
            groups,
            check,
        }
    }

    /// Take in what a refresh read. Mints asked for and not answered lose
    /// their old price, since the service no longer has one.
    pub fn apply(&mut self, update: MarketUpdate, now: i64) {
        if let Some(quotes) = update.quotes {
            for (mint, quote) in quotes {
                match quote {
                    Some(quote) => self.quotes.insert(mint.to_string(), quote),
                    None => self.quotes.remove(&mint.to_string()),
                };
            }
            self.quoted_at = Some(now);
        }
        if let Some(groups) = update.groups {
            self.groups = groups
                .into_iter()
                .map(|(mint, group)| (mint.to_string(), group))
                .collect();
            self.grouped_at = Some(now);
        }
        for (mint, mut check) in update.checks {
            check.checked_at = now;
            self.checks.insert(mint.to_string(), check);
        }
        self.error = update.error;
    }
}

/// What one refresh asks the service for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketPlan {
    /// Mints to price. Always starts with wrapped SOL.
    pub quote: Vec<Pubkey>,
    /// Read the asset groups again.
    pub groups: bool,
    /// Mints to check.
    pub check: Vec<Pubkey>,
}

/// What one refresh read. A part that was not asked, or failed, is `None`
/// or empty, and the cache keeps what it had.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarketUpdate {
    pub quotes: Option<Vec<(Pubkey, Option<MarketQuote>)>>,
    pub groups: Option<Vec<(Pubkey, AssetGroup)>>,
    pub checks: Vec<(Pubkey, TokenCheck)>,
    pub error: Option<String>,
}

/// Why the service did not answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MarketError {
    #[error("The Tokens API refused the key. Check it in Settings.")]
    Key,
    #[error("The Tokens API asks to slow down. Prices refresh again later.")]
    RateLimited,
    #[error("{0}")]
    Message(String),
}

/// A market data service. The app has one, over the Tokens API; tests use a
/// fake.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait MarketSource: Send + Sync {
    /// A quote for each mint, in the order asked. `None` for a mint the
    /// service has no market for.
    async fn quotes(
        &self,
        mints: &[Pubkey],
    ) -> Result<Vec<(Pubkey, Option<MarketQuote>)>, MarketError>;
    /// Every curated mint and the asset it belongs to.
    async fn groups(&self) -> Result<Vec<(Pubkey, AssetGroup)>, MarketError>;
    /// Whether `mint` is the token it claims to be. `checked_at` is set by
    /// the cache.
    async fn check(&self, mint: &Pubkey) -> Result<TokenCheck, MarketError>;
}

/// Prices are for mainnet: the service knows no other cluster.
pub fn prices_apply_to(cluster: &Cluster) -> bool {
    match cluster {
        Cluster::MainnetBeta => true,
        Cluster::Custom { known, .. } => *known == Some(KnownCluster::MainnetBeta),
        _ => false,
    }
}
