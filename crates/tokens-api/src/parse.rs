//! Reading the service's answers. Every field is optional here, and a row
//! that cannot be read is skipped, so a change on their side costs a row,
//! not the whole refresh.

use serde_json::Value;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use wallet_core::portfolio::{AssetGroup, MarketQuote, TokenCheck};

/// One quote per mint in `asked`, in that order, from a
/// `market-snapshots` answer. A mint missing from the answer, with no token,
/// or with `hasMarket: false` has no price: the service's numbers are `0`
/// when it has none, so they are not used.
pub fn parse_snapshots(answer: &Value, asked: &[Pubkey]) -> Vec<(Pubkey, Option<MarketQuote>)> {
    let rows: Vec<&Value> = match answer {
        Value::Array(rows) => rows.iter().collect(),
        Value::Object(map) => map
            .get("snapshots")
            .or_else(|| map.get("data"))
            .and_then(Value::as_array)
            .map(|rows| rows.iter().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    asked
        .iter()
        .map(|mint| {
            let address = mint.to_string();
            let row = rows
                .iter()
                .find(|row| text(row, "address").as_deref() == Some(address.as_str()));
            (*mint, row.and_then(|row| quote_of(row)))
        })
        .collect()
}

fn quote_of(row: &Value) -> Option<MarketQuote> {
    let token = row.get("token").filter(|token| token.is_object())?;
    let has_market = row
        .get("hasMarket")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let price = number(token, "price").filter(|price| has_market && *price > 0.0);
    Some(MarketQuote {
        symbol: text(token, "symbol").filter(|s| !s.is_empty()),
        name: text(token, "name").filter(|s| !s.is_empty()),
        price_usd: price,
        change_24h_pct: price.and(number(token, "priceChange24hPercent")),
    })
}

/// Rows of one page of `/v1/assets/curated?groupBy=mint`, and the offset of
/// the next page when there is one.
pub fn parse_curated(page: &Value) -> (Vec<(Pubkey, AssetGroup)>, Option<usize>) {
    let rows = page
        .get("assets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let found = rows
        .iter()
        .filter_map(|row| {
            let asset_id = text(row, "assetId")?;
            let variant = row.get("primaryVariant");
            let mint = variant
                .and_then(|variant| text(variant, "mint"))
                .or_else(|| text(row, "mint"))?;
            let mint = Pubkey::from_str(&mint).ok()?;
            // The service files liquid staking tokens under "solana", with
            // kind "yield" and the tag "curated:lsts".
            let lst = variant
                .and_then(|variant| variant.get("tags"))
                .and_then(Value::as_array)
                .is_some_and(|tags| tags.iter().any(|tag| tag == "curated:lsts"));
            Some((
                mint,
                AssetGroup {
                    asset_id,
                    name: text(row, "name"),
                    symbol: text(row, "symbol"),
                    lst,
                },
            ))
        })
        .collect();
    let pagination = page.get("pagination");
    let more = pagination
        .and_then(|p| p.get("hasMore"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next = pagination
        .and_then(|p| p.get("nextOffset"))
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .filter(|_| more);
    (found, next)
}

/// What `/v2/resolve?q=<mint>` says of `mint`. A mint the service does not
/// match is unverified, with no warning.
pub fn parse_resolve(answer: &Value, mint: &Pubkey) -> TokenCheck {
    let best = answer
        .get("best")
        .filter(|best| text(best, "mint").as_deref() == Some(mint.to_string().as_str()));
    let Some(best) = best else {
        return TokenCheck::default();
    };
    TokenCheck {
        verified: best
            .get("verified")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        warnings: best
            .get("warnings")
            .and_then(Value::as_array)
            .map(|codes| {
                codes
                    .iter()
                    .filter_map(|code| code.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        checked_at: 0,
    }
}

/// The message of the service's error envelope, or the start of the body.
pub(crate) fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| text(value.get("error")?, "message"))
        .unwrap_or_else(|| body.chars().take(200).collect())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn number(value: &Value, key: &str) -> Option<f64> {
    let found = value.get(key)?;
    found
        .as_f64()
        .or_else(|| found.as_str().and_then(|s| s.parse().ok()))
        .filter(|n| n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SOL: &str = "So11111111111111111111111111111111111111112";
    const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const JITO: &str = "J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn";

    fn pk(s: &str) -> Pubkey {
        Pubkey::from_str(s).unwrap()
    }

    #[test]
    fn snapshots_keep_the_asked_order_and_trust_only_real_markets() {
        let answer = json!([
            {
                "address": USDC,
                "token": { "address": USDC, "symbol": "USDC", "name": "USD Coin", "decimals": 6,
                           "price": 0.9998, "priceChange24hPercent": 0.01, "liquidity": 1.0e9 },
                "hasMarket": true
            },
            {
                "address": SOL,
                "token": { "address": SOL, "symbol": "SOL", "name": "Wrapped SOL", "decimals": 9,
                           "price": 0, "priceChange24hPercent": 0 },
                "hasMarket": false
            }
        ]);
        let quotes = parse_snapshots(&answer, &[pk(SOL), pk(USDC), pk(JITO)]);
        assert_eq!(quotes.len(), 3);
        assert_eq!(quotes[0].0, pk(SOL));
        // Identity only: the zero is not a price.
        let sol = quotes[0].1.as_ref().unwrap();
        assert_eq!(sol.symbol.as_deref(), Some("SOL"));
        assert_eq!(sol.price_usd, None);
        assert_eq!(sol.change_24h_pct, None);
        let usdc = quotes[1].1.as_ref().unwrap();
        assert_eq!(usdc.price_usd, Some(0.9998));
        assert_eq!(usdc.change_24h_pct, Some(0.01));
        // Not in the answer at all.
        assert!(quotes[2].1.is_none());
    }

    #[test]
    fn a_null_token_is_no_quote() {
        let answer = json!([{ "address": JITO, "token": null, "hasMarket": false }]);
        assert!(parse_snapshots(&answer, &[pk(JITO)])[0].1.is_none());
    }

    #[test]
    fn curated_rows_name_their_asset_and_mark_liquid_staking() {
        let page = json!({
            "listId": "all",
            "primaryVariantStrategy": "liquidity",
            "pagination": { "offset": 0, "limit": 500, "total": 3, "hasMore": true, "nextOffset": 500 },
            "assets": [
                { "assetId": "usd", "name": "US Dollar", "symbol": "USD", "category": "stablecoin",
                  "primaryVariant": { "mint": USDC, "chain": "solana", "kind": "stablecoin" } },
                { "assetId": "solana", "name": "Solana", "symbol": "SOL", "category": "crypto",
                  "primaryVariant": { "mint": JITO, "kind": "yield", "tags": ["curated:lsts", "source:sanctum"] } },
                { "assetId": "broken" }
            ]
        });
        let (rows, next) = parse_curated(&page);
        assert_eq!(next, Some(500));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, pk(USDC));
        assert_eq!(rows[0].1.asset_id, "usd");
        assert!(!rows[0].1.lst);
        assert!(rows[1].1.lst);
    }

    #[test]
    fn the_last_curated_page_has_no_next() {
        let page = json!({ "assets": [], "pagination": { "hasMore": false, "nextOffset": null } });
        assert_eq!(parse_curated(&page).1, None);
    }

    #[test]
    fn resolve_reads_the_best_match_for_the_same_mint_only() {
        let answer = json!({
            "status": "resolved",
            "best": { "mint": USDC, "verified": true, "warnings": [], "confidence": 1 },
            "candidates": []
        });
        let check = parse_resolve(&answer, &pk(USDC));
        assert!(check.verified);
        assert!(!check.impersonates());

        let fake = json!({
            "status": "resolved",
            "best": { "mint": JITO, "verified": false, "warnings": ["possible_impersonation", "low_liquidity"] }
        });
        assert!(parse_resolve(&fake, &pk(JITO)).impersonates());
        // An answer about another mint says nothing of this one.
        assert_eq!(parse_resolve(&fake, &pk(USDC)), TokenCheck::default());
        let none = json!({ "status": "no_confident_match", "best": null, "candidates": [] });
        assert_eq!(parse_resolve(&none, &pk(USDC)), TokenCheck::default());
    }

    #[test]
    fn errors_say_what_the_service_said() {
        let body =
            r#"{ "error": { "_tag": "BadRequestError", "message": "mints must be base58" } }"#;
        assert_eq!(error_message(body), "mints must be base58");
        assert_eq!(error_message("gateway timeout"), "gateway timeout");
    }
}
