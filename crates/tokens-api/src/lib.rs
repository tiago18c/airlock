//! The Tokens API (<https://docs.tokens.xyz>) as a [`MarketSource`]:
//!
//! - prices: `POST /v1/assets/market-snapshots`, up to 250 mints a call;
//! - asset groups: `GET /v1/assets/curated?list=all&groupBy=mint`, every
//!   curated mint with the canonical asset it is a variant of;
//! - checks: `GET /v2/resolve?q=<mint>`, whether a mint is verified and what
//!   the service warns about it.
//!
//! v2 has no call that prices many mints at once, so prices and groups come
//! from v1. Only mints are sent, never an address or an amount. The key is
//! never logged.

mod parse;

pub use parse::{parse_curated, parse_resolve, parse_snapshots};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use solana_pubkey::Pubkey;
use std::time::Duration;
use wallet_core::portfolio::{AssetGroup, MarketError, MarketQuote, MarketSource, TokenCheck};

/// Where the service is.
pub const BASE_URL: &str = "https://api.tokens.xyz";
/// Mints in one price call.
pub const SNAPSHOT_BATCH: usize = 250;
/// Rows in one page of the curated list.
const CURATED_PAGE: usize = 500;
/// Pages read before giving up on a list that does not end.
const CURATED_MAX_PAGES: usize = 40;
const TIMEOUT: Duration = Duration::from_secs(20);

/// A client for one key.
pub struct TokensApi {
    client: reqwest::Client,
    base: String,
    key: String,
}

impl TokensApi {
    pub fn new(key: impl Into<String>) -> Self {
        Self::with_base(BASE_URL, key)
    }

    /// The same client against another host, for tests.
    pub fn with_base(base: impl Into<String>, key: impl Into<String>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .unwrap_or_default();
        #[cfg(target_arch = "wasm32")]
        let client = {
            let _ = TIMEOUT;
            reqwest::Client::new()
        };
        Self {
            client,
            base: base.into().trim_end_matches('/').to_string(),
            key: key.into(),
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<String, MarketError> {
        let response = request
            .header("x-api-key", &self.key)
            .send()
            .await
            .map_err(|err| {
                MarketError::Message(format!("The Tokens API could not be reached: {err}"))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|err| {
            MarketError::Message(format!("The Tokens API answer was cut short: {err}"))
        })?;
        match status.as_u16() {
            200..=299 => Ok(body),
            401 | 403 => Err(MarketError::Key),
            429 => Err(MarketError::RateLimited),
            code => Err(MarketError::Message(format!(
                "The Tokens API answered {code}: {}",
                parse::error_message(&body)
            ))),
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, MarketError> {
        let body = self
            .send(self.client.get(format!("{}{path}", self.base)))
            .await?;
        read(&body)
    }
}

fn read<T: DeserializeOwned>(body: &str) -> Result<T, MarketError> {
    serde_json::from_str(body).map_err(|err| {
        MarketError::Message(format!("The Tokens API answer could not be read: {err}"))
    })
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl MarketSource for TokensApi {
    async fn quotes(
        &self,
        mints: &[Pubkey],
    ) -> Result<Vec<(Pubkey, Option<MarketQuote>)>, MarketError> {
        let mut out = Vec::with_capacity(mints.len());
        for batch in mints.chunks(SNAPSHOT_BATCH) {
            let addresses: Vec<String> = batch.iter().map(Pubkey::to_string).collect();
            let body = self
                .send(
                    self.client
                        .post(format!("{}/v1/assets/market-snapshots", self.base))
                        .json(&serde_json::json!({ "mints": addresses })),
                )
                .await?;
            out.extend(parse_snapshots(&read::<serde_json::Value>(&body)?, batch));
        }
        Ok(out)
    }

    async fn groups(&self) -> Result<Vec<(Pubkey, AssetGroup)>, MarketError> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        for _ in 0..CURATED_MAX_PAGES {
            let page: serde_json::Value = self
                .get(&format!(
                    "/v1/assets/curated?list=all&groupBy=mint&limit={CURATED_PAGE}&offset={offset}"
                ))
                .await?;
            let (rows, next) = parse_curated(&page);
            out.extend(rows);
            match next {
                Some(next) if next > offset => offset = next,
                _ => return Ok(out),
            }
        }
        Ok(out)
    }

    async fn check(&self, mint: &Pubkey) -> Result<TokenCheck, MarketError> {
        let answer: serde_json::Value = self
            .get(&format!("/v2/resolve?q={mint}&policy=degen"))
            .await?;
        Ok(parse_resolve(&answer, mint))
    }
}
