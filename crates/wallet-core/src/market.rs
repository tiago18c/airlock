//! What a market data service says about the tokens held here: a price, the
//! canonical asset a mint belongs to, and whether the mint is the token it
//! claims to be. Kept in the holdings cache, refreshed by a sync job.

use crate::cluster::{Cluster, KnownCluster};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;

/// The mint that prices SOL.
pub const WRAPPED_SOL_MINT: Pubkey = crate::ids::NATIVE_MINT;

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
    /// Where the service says the token's logo is: an `https://` URL, or
    /// `None` when it named none that could be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,
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

/// The image formats a logo is kept in. Each is one the page can show from a
/// `data:` URL without anything decoding it here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogoKind {
    Png,
    Jpeg,
    Webp,
}

impl LogoKind {
    /// The end of the logo's file name.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    /// The media type of a `data:` URL holding it.
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}

/// What came of asking for a mint's logo. Either way it is not asked again:
/// a logo is downloaded once, ever.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum LogoState {
    /// The image is in the logos folder, in this format. The bytes are not
    /// in the cache.
    Kept { kind: LogoKind },
    /// The answer was not an image this app keeps.
    Nothing,
}

/// What one logo download found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogoFetch {
    Image {
        kind: LogoKind,
        bytes: Vec<u8>,
    },
    /// There was an answer, and it is not a logo this app keeps. Recorded,
    /// and never asked again.
    Unusable,
    /// No answer could be had. Not recorded, so a later run asks again.
    Unreachable,
}

/// Logos downloaded in one refresh, one at a time, so a refresh with many new
/// tokens does not keep the job busy for long.
pub const LOGOS_PER_RUN: usize = 8;

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
    /// The mints whose logo was asked for, and what came of it. Never
    /// dropped: an entry here is never downloaded again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub logos: BTreeMap<String, LogoState>,
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

    pub fn logo(&self, mint: &Pubkey) -> Option<LogoState> {
        self.logos.get(&mint.to_string()).copied()
    }

    /// Record what came of asking for `mint`'s logo.
    pub fn set_logo(&mut self, mint: &Pubkey, state: LogoState) {
        self.logos.insert(mint.to_string(), state);
    }

    /// The logos to download, at most `limit`, in the order of `mints`: those
    /// whose quote names a logo and that were never asked for. A mint the
    /// service thinks passes itself off as another is left out, since its
    /// logo would be the other token's; one not checked yet is not.
    pub fn logos_wanted(&self, mints: &[Pubkey], limit: usize) -> Vec<(Pubkey, String)> {
        let mut wanted: Vec<(Pubkey, String)> = Vec::new();
        for mint in mints {
            if wanted.len() >= limit {
                break;
            }
            if wanted.iter().any(|(seen, _)| seen == mint) || self.logo(mint).is_some() {
                continue;
            }
            if self.check(mint).is_some_and(TokenCheck::impersonates) {
                continue;
            }
            if let Some(url) = self.quote(mint).and_then(|quote| quote.logo_uri.clone()) {
                wanted.push((*mint, url));
            }
        }
        wanted
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
    /// their old price, since the service no longer has one. What was asked
    /// about their logos stays.
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
    /// Download the logo at `url`, which the service named. A source that
    /// downloads none finds nothing usable.
    async fn logo(&self, _url: &str) -> LogoFetch {
        LogoFetch::Unusable
    }
}

/// Prices are for mainnet: the service knows no other cluster.
pub fn prices_apply_to(cluster: &Cluster) -> bool {
    match cluster {
        Cluster::MainnetBeta => true,
        Cluster::Custom { known, .. } => *known == Some(KnownCluster::MainnetBeta),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn with_logo(url: &str) -> MarketQuote {
        MarketQuote {
            symbol: Some("TOK".into()),
            logo_uri: Some(url.into()),
            ..Default::default()
        }
    }

    fn url(n: u8) -> String {
        format!("https://logos.example/{n}.png")
    }

    #[test]
    fn logos_wanted_are_those_named_and_never_asked() {
        let mut cache = MarketCache::default();
        for n in 1..=6 {
            cache.quotes.insert(pk(n).to_string(), with_logo(&url(n)));
        }
        // No URL.
        cache
            .quotes
            .insert(pk(7).to_string(), MarketQuote::default());
        cache.set_logo(
            &pk(2),
            LogoState::Kept {
                kind: LogoKind::Png,
            },
        );
        cache.set_logo(&pk(3), LogoState::Nothing);
        cache.checks.insert(
            pk(4).to_string(),
            TokenCheck {
                warnings: vec!["possible_impersonation".into()],
                ..Default::default()
            },
        );
        // Checked and fine.
        cache
            .checks
            .insert(pk(5).to_string(), TokenCheck::default());
        let mints = [
            pk(7),
            pk(6),
            pk(1),
            pk(2),
            pk(3),
            pk(4),
            pk(5),
            pk(6),
            pk(8),
        ];
        assert_eq!(
            cache.logos_wanted(&mints, 10),
            vec![(pk(6), url(6)), (pk(1), url(1)), (pk(5), url(5))]
        );
        assert_eq!(
            cache.logos_wanted(&mints, 2),
            vec![(pk(6), url(6)), (pk(1), url(1))]
        );
        assert!(cache.logos_wanted(&mints, 0).is_empty());
    }

    #[test]
    fn apply_keeps_the_logos_even_of_a_mint_that_lost_its_quote() {
        let mut cache = MarketCache::default();
        cache.quotes.insert(pk(1).to_string(), with_logo(&url(1)));
        cache.set_logo(
            &pk(1),
            LogoState::Kept {
                kind: LogoKind::Webp,
            },
        );
        cache.set_logo(&pk(2), LogoState::Nothing);
        cache.apply(
            MarketUpdate {
                quotes: Some(vec![(pk(1), None)]),
                groups: Some(Vec::new()),
                ..Default::default()
            },
            100,
        );
        assert!(cache.quote(&pk(1)).is_none());
        assert_eq!(
            cache.logo(&pk(1)),
            Some(LogoState::Kept {
                kind: LogoKind::Webp
            })
        );
        assert_eq!(cache.logo(&pk(2)), Some(LogoState::Nothing));
        // And a mint the cache has a logo for is not asked again, whatever
        // its quote says now.
        cache.quotes.insert(pk(1).to_string(), with_logo(&url(9)));
        assert!(cache.logos_wanted(&[pk(1)], 8).is_empty());
    }

    #[test]
    fn a_cache_from_before_logos_still_reads() {
        let old = r#"{
            "quotes": { "So11111111111111111111111111111111111111112": { "symbol": "SOL", "priceUsd": 150.0 } },
            "quotedAt": 100
        }"#;
        let cache: MarketCache = serde_json::from_str(old).unwrap();
        assert_eq!(cache.quote(&WRAPPED_SOL_MINT).unwrap().logo_uri, None);
        assert!(cache.logos.is_empty());
        // Nothing new is written for what is not there.
        let written = serde_json::to_string(&cache).unwrap();
        assert!(!written.contains("logo"), "{written}");
    }

    #[test]
    fn logos_round_trip_in_the_cache() {
        let mut cache = MarketCache::default();
        cache.quotes.insert(pk(1).to_string(), with_logo(&url(1)));
        cache.set_logo(
            &pk(1),
            LogoState::Kept {
                kind: LogoKind::Jpeg,
            },
        );
        cache.set_logo(&pk(2), LogoState::Nothing);
        let written = serde_json::to_string(&cache).unwrap();
        assert!(
            written.contains(r#""logoUri":"https://logos.example/1.png""#),
            "{written}"
        );
        assert!(
            written.contains(r#"{"state":"kept","kind":"jpeg"}"#),
            "{written}"
        );
        assert!(written.contains(r#"{"state":"nothing"}"#), "{written}");
        let read: MarketCache = serde_json::from_str(&written).unwrap();
        assert_eq!(read, cache);
    }
}
