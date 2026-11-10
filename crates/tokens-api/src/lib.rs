//! The Tokens API (<https://docs.tokens.xyz>) as a [`MarketSource`]:
//!
//! - prices: `POST /v1/assets/market-snapshots`, up to 250 mints a call;
//! - asset groups: `GET /v1/assets/curated?list=all&groupBy=mint`, every
//!   curated mint with the canonical asset it is a variant of;
//! - checks: `GET /v2/resolve?q=<mint>`, whether a mint is verified and what
//!   the service warns about it;
//! - logos: a `GET` of the `logoURI` a price answer names, once per mint.
//!
//! v2 has no call that prices many mints at once, so prices and groups come
//! from v1. Only mints are sent, never an address or an amount. The key is
//! never logged.
//!
//! A logo is downloaded from whatever host the service names for it, its own
//! storage or a third party's, with a client of its own: only the URL is
//! sent, with no key, cookie or header of ours, so the key never goes to a
//! logo host. Only `https://` is followed, and only a PNG, JPEG or WebP of at
//! most 128 KiB is kept. The browser build downloads no logos.

mod logo;
mod parse;

pub use logo::{sniff_logo, LOGO_MAX_BYTES};
pub use parse::{parse_curated, parse_resolve, parse_snapshots};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use solana_pubkey::Pubkey;
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use wallet_core::portfolio::LogoFetch;
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
    /// For the Tokens API only: [`send`](Self::send) puts the key on its
    /// requests.
    client: reqwest::Client,
    base: String,
    key: String,
    /// For logos, apart from `client`, so nothing sent with it carries the
    /// key.
    #[cfg(not(target_arch = "wasm32"))]
    logos: logo::LogoClient,
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
            #[cfg(not(target_arch = "wasm32"))]
            logos: logo::LogoClient::new(),
        }
    }

    /// The same client, downloading logos over plain HTTP too, for a test
    /// server.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    fn with_plain_http_logos(mut self) -> Self {
        self.logos = logo::LogoClient::plain_http_for_tests();
        self
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

    #[cfg(not(target_arch = "wasm32"))]
    async fn logo(&self, url: &str) -> LogoFetch {
        self.logos.fetch(url).await
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    const KEY: &str = "the-secret-tokens-key";
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];

    /// A plain HTTP server on a loopback port that answers by path and keeps
    /// every request it was sent, whole.
    struct Server {
        base: String,
        seen: Arc<Mutex<Vec<String>>>,
    }

    fn serve(answer: fn(&str, &str) -> Vec<u8>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = seen.clone();
        let own = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let request = String::from_utf8_lossy(&request).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
                kept.lock().unwrap().push(request);
                let _ = stream.write_all(&answer(&own, &path));
            }
        });
        Server { base, seen }
    }

    fn reply(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\nConnection: close\r\n{headers}Content-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn api() -> TokensApi {
        TokensApi::with_base("http://127.0.0.1:9", KEY).with_plain_http_logos()
    }

    #[tokio::test]
    async fn a_logo_request_carries_nothing_of_ours() {
        let server = serve(|base, path| match path {
            "/moved" => reply("302 Found", &format!("Location: {base}/logo.png\r\n"), b""),
            _ => reply("200 OK", "Content-Type: text/plain\r\n", PNG),
        });
        let api = api();
        assert_eq!(
            api.logo(&format!("{}/moved", server.base)).await,
            LogoFetch::Image {
                kind: wallet_core::portfolio::LogoKind::Png,
                bytes: PNG.to_vec()
            }
        );
        let seen = server.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "the redirect was followed");
        for request in &seen {
            assert!(!request.contains(KEY), "{request}");
            let lower = request.to_ascii_lowercase();
            assert!(!lower.contains("x-api-key"), "{request}");
            assert!(!lower.contains("cookie"), "{request}");
            // Only what any HTTP client sends.
            for line in request.lines().skip(1).filter(|line| !line.is_empty()) {
                let name = line.split(':').next().unwrap().to_ascii_lowercase();
                assert!(["host", "accept"].contains(&name.as_str()), "{line}");
            }
        }
    }

    #[tokio::test]
    async fn what_a_logo_host_answers_decides_whether_it_is_asked_again() {
        let server = serve(|_, path| match path {
            "/svg" => reply("200 OK", "", b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            "/gone" => reply("404 Not Found", "", b""),
            "/busy" => reply("503 Service Unavailable", "", b""),
            "/slow-down" => reply("429 Too Many Requests", "", b""),
            "/big" => {
                let mut body = PNG.to_vec();
                body.resize(LOGO_MAX_BYTES + 1, 0);
                reply("200 OK", "", &body)
            }
            "/loop" => reply("302 Found", "Location: /loop\r\n", b""),
            _ => reply("200 OK", "", PNG),
        });
        let api = api();
        let at = |path: &str| format!("{}{path}", server.base);
        assert!(matches!(
            api.logo(&at("/ok")).await,
            LogoFetch::Image { .. }
        ));
        assert_eq!(api.logo(&at("/svg")).await, LogoFetch::Unusable);
        assert_eq!(api.logo(&at("/gone")).await, LogoFetch::Unusable);
        assert_eq!(api.logo(&at("/big")).await, LogoFetch::Unusable);
        assert_eq!(api.logo(&at("/loop")).await, LogoFetch::Unusable);
        assert_eq!(api.logo(&at("/busy")).await, LogoFetch::Unreachable);
        assert_eq!(api.logo(&at("/slow-down")).await, LogoFetch::Unreachable);
        // Nothing listens on port 9.
        assert_eq!(
            api.logo("http://127.0.0.1:9/logo.png").await,
            LogoFetch::Unreachable
        );
    }

    #[tokio::test]
    async fn the_real_client_asks_nothing_but_https() {
        let server = serve(|_, _| reply("200 OK", "", PNG));
        let api = TokensApi::new(KEY);
        assert_eq!(
            api.logo(&format!("{}/logo.png", server.base)).await,
            LogoFetch::Unusable
        );
        assert_eq!(
            api.logo("ftp://example.com/logo.png").await,
            LogoFetch::Unusable
        );
        assert!(server.seen.lock().unwrap().is_empty());
    }
}
