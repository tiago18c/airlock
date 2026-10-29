//! Against the real service. The key comes from Doppler (see "Secrets" in
//! the README):
//! `doppler run -- cargo test -p tokens-api --test live -- --ignored --nocapture`.

#![cfg(not(target_arch = "wasm32"))]

use solana_pubkey::Pubkey;
use std::str::FromStr;
use tokens_api::TokensApi;
use wallet_core::portfolio::MarketSource;

const SOL: &str = "So11111111111111111111111111111111111111112";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const JITOSOL: &str = "J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn";

fn api() -> TokensApi {
    TokensApi::new(std::env::var("TOKENS_API_KEY").expect("set TOKENS_API_KEY"))
}

fn mint(s: &str) -> Pubkey {
    Pubkey::from_str(s).unwrap()
}

#[tokio::test]
#[ignore = "needs TOKENS_API_KEY and the network"]
async fn quotes() {
    let mints = [mint(SOL), mint(USDC), mint(USDT), Pubkey::new_unique()];
    let quotes = api().quotes(&mints).await.unwrap();
    for (m, q) in &quotes {
        println!("{m}: {q:?}");
    }
    assert_eq!(quotes.len(), mints.len());
    let usdc = quotes[1].1.as_ref().expect("USDC has a market");
    assert!((usdc.price_usd.unwrap() - 1.0).abs() < 0.05);
    assert!(quotes[0].1.as_ref().unwrap().price_usd.unwrap() > 1.0);
    assert!(quotes[3].1.is_none());
}

#[tokio::test]
#[ignore = "needs TOKENS_API_KEY and the network"]
async fn groups() {
    let groups = api().groups().await.unwrap();
    println!("{} curated mints", groups.len());
    let usdc = groups
        .iter()
        .find(|(m, _)| *m == mint(USDC))
        .expect("USDC is curated");
    println!("USDC: {:?}", usdc.1);
    assert_eq!(usdc.1.asset_id, "usd");
    let jitosol = groups
        .iter()
        .find(|(m, _)| *m == mint(JITOSOL))
        .expect("JitoSOL is curated");
    assert!(
        jitosol.1.lst,
        "JitoSOL is a liquid staking token: {:?}",
        jitosol.1
    );
    println!("LSTs: {}", groups.iter().filter(|(_, g)| g.lst).count());
}

#[tokio::test]
#[ignore = "needs TOKENS_API_KEY and the network"]
async fn check() {
    let check = api().check(&mint(USDC)).await.unwrap();
    println!("USDC: {check:?}");
    assert!(check.verified);
    assert!(!check.impersonates());
}

#[tokio::test]
#[ignore = "needs the network"]
async fn bad_key() {
    let err = TokensApi::new("not-a-key")
        .quotes(&[mint(USDC)])
        .await
        .unwrap_err();
    assert_eq!(err, wallet_core::portfolio::MarketError::Key);
}
