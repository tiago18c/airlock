use super::*;
use crate::book::{AddressBook, Holdings, TokenFlag};
use crate::chain::DiscoveredTokenAccount;

fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

const USDC: u8 = 50;
const USDT: u8 = 51;
const JITO: u8 = 52;
const MEME: u8 = 53;
const FAKE: u8 = 54;
const NFT: u8 = 55;

fn quote(symbol: &str, price: f64, change: f64) -> MarketQuote {
    MarketQuote {
        symbol: Some(symbol.into()),
        name: None,
        price_usd: Some(price),
        change_24h_pct: Some(change),
    }
}

fn group(asset: &str, lst: bool) -> AssetGroup {
    AssetGroup {
        asset_id: asset.into(),
        name: None,
        symbol: Some(asset.to_uppercase()),
        lst,
    }
}

fn market() -> MarketCache {
    let mut cache = MarketCache::default();
    cache.apply(
        MarketUpdate {
            quotes: Some(vec![
                (WRAPPED_SOL_MINT, Some(quote("SOL", 100.0, 10.0))),
                (pk(USDC), Some(quote("USDC", 1.0, 0.0))),
                (pk(USDT), Some(quote("USDT", 1.0, 0.0))),
                (pk(JITO), Some(quote("JitoSOL", 120.0, 10.0))),
                (pk(FAKE), Some(quote("USDC", 1.0, 0.0))),
                (pk(MEME), None),
            ]),
            groups: Some(vec![
                (pk(USDC), group("usd", false)),
                (pk(USDT), group("usd", false)),
                (pk(JITO), group("jitosol", true)),
            ]),
            checks: vec![(
                pk(FAKE),
                TokenCheck {
                    verified: false,
                    warnings: vec!["possible_impersonation".into()],
                    checked_at: 0,
                },
            )],
            error: None,
        },
        1_000,
    );
    cache
}

fn token(owner: u8, account: u8, mint: u8, amount: u64, decimals: u8) -> DiscoveredTokenAccount {
    DiscoveredTokenAccount {
        pubkey: pk(account),
        owner: pk(owner),
        mint: pk(mint),
        amount,
        decimals,
    }
}

/// Owner 1: 2 SOL, 10 USDC, 1 JitoSOL, a memecoin, a fake USDC and an NFT.
/// Owner 2: 1 SOL, 5 USDT and 3 SOL staked.
fn setup() -> (Vec<Owner>, AddressBook, Holdings, Vec<HeldStake>) {
    let owners = vec![
        Owner {
            address: pk(1),
            label: "One".into(),
            watched: false,
        },
        Owner {
            address: pk(2),
            label: "Two".into(),
            watched: true,
        },
    ];
    let mut book = AddressBook::new();
    book.insert_contact(pk(1), "One");
    book.insert_contact(pk(2), "Two");
    book.set_lamports(&pk(1), 2 * LAMPORTS_PER_SOL);
    book.set_lamports(&pk(2), LAMPORTS_PER_SOL);
    let mut holdings = Holdings::default();
    holdings.replace_owners(
        &[pk(1), pk(2)],
        &[
            token(1, 10, USDC, 10_000_000, 6),
            token(1, 11, JITO, LAMPORTS_PER_SOL, 9),
            token(1, 12, MEME, 5, 0),
            token(1, 13, FAKE, 1_000_000, 6),
            token(1, 14, NFT, 1, 0),
            token(2, 20, USDT, 5_000_000, 6),
        ],
    );
    let stakes = vec![
        HeldStake {
            owner: pk(2),
            account: pk(30),
            lamports: 3 * LAMPORTS_PER_SOL,
        },
        // The same account found under the first owner too: counted once.
        HeldStake {
            owner: pk(1),
            account: pk(30),
            lamports: 3 * LAMPORTS_PER_SOL,
        },
    ];
    (owners, book, holdings, stakes)
}

#[test]
fn related_tokens_are_grouped_and_the_total_adds_up() {
    let (owners, book, holdings, stakes) = setup();
    let portfolio = build(&owners, &book, &holdings, &stakes, &market(), None);

    // SOL 3 x 100 + staked 3 x 100 + JitoSOL 120 = 720; dollars 10 + 5 = 15.
    assert_eq!(portfolio.groups.len(), 2);
    let sol = &portfolio.groups[0];
    assert_eq!(sol.key, "sol");
    assert_eq!(sol.lines.len(), 3);
    assert!((sol.usd - 720.0).abs() < 1e-9);
    let usd = &portfolio.groups[1];
    assert_eq!(usd.key, "asset:usd");
    assert_eq!(usd.lines.len(), 2);
    assert!((usd.usd - 15.0).abs() < 1e-9);
    assert!((portfolio.total_usd - 735.0).abs() < 1e-9);
}

#[test]
fn a_stake_account_under_two_owners_counts_once() {
    let (owners, book, holdings, stakes) = setup();
    let portfolio = build(&owners, &book, &holdings, &stakes, &market(), None);
    let staked = portfolio.groups[0]
        .lines
        .iter()
        .find(|line| line.asset == Asset::StakedSol)
        .unwrap();
    assert_eq!(staked.raw, u128::from(3 * LAMPORTS_PER_SOL));
}

#[test]
fn each_share_names_its_accounts_largest_first() {
    let (owners, book, mut holdings, stakes) = setup();
    holdings.replace_owners(
        &[pk(1)],
        &[
            token(1, 10, USDC, 1_000_000, 6),
            token(1, 15, USDC, 9_000_000, 6),
        ],
    );
    let portfolio = build(&owners, &book, &holdings, &stakes, &market(), Some(pk(1)));
    let lines = portfolio.groups.iter().flat_map(|group| &group.lines);
    let usdc = lines
        .clone()
        .find(|line| line.asset == Asset::Token(pk(USDC)))
        .unwrap();
    assert_eq!(usdc.shares[0].raw, 10_000_000);
    assert_eq!(usdc.shares[0].accounts, vec![pk(15), pk(10)]);
    let sol = lines.clone().find(|line| line.asset == Asset::Sol).unwrap();
    assert!(sol.shares[0].accounts.is_empty());
    let staked = lines
        .clone()
        .find(|line| line.asset == Asset::StakedSol)
        .unwrap();
    assert_eq!(staked.shares[0].accounts, vec![pk(30)]);
}

#[test]
fn unpriced_collectibles_and_impostors_stay_out_of_the_total() {
    let (owners, book, mut holdings, stakes) = setup();
    holdings.set_flag(&pk(2), &pk(20), Some(TokenFlag::Red));
    let portfolio = build(&owners, &book, &holdings, &stakes, &market(), None);
    assert_eq!(portfolio.unpriced.len(), 1);
    assert_eq!(portfolio.unpriced[0].asset, Asset::Token(pk(MEME)));
    assert_eq!(portfolio.collectibles.len(), 1);
    let left: Vec<Asset> = portfolio.left_out.iter().map(|line| line.asset).collect();
    assert!(left.contains(&Asset::Token(pk(FAKE))));
    assert!(left.contains(&Asset::Token(pk(USDT))));
    assert!((portfolio.total_usd - 730.0).abs() < 1e-9);
}

#[test]
fn one_owner_shows_only_its_holdings() {
    let (owners, book, holdings, stakes) = setup();
    let portfolio = build(&owners, &book, &holdings, &stakes, &market(), Some(pk(2)));
    // 1 SOL + 3 staked = 400; 5 USDT.
    assert!((portfolio.total_usd - 405.0).abs() < 1e-9);
    assert!(portfolio.unpriced.is_empty());
}

#[test]
fn the_daily_change_follows_each_price() {
    let (owners, book, holdings, stakes) = setup();
    let portfolio = build(&owners, &book, &holdings, &stakes, &market(), None);
    // 720 after +10% was 654.5, so +65.45; the stablecoins did not move.
    let change = portfolio.change_24h_usd.unwrap();
    assert!((change - (720.0 - 720.0 / 1.1)).abs() < 1e-6);
}

#[test]
fn a_refresh_asks_only_for_what_is_stale() {
    let cache = market();
    let mints = [pk(USDC), pk(MEME), pk(99)];
    let plan = cache.plan(&mints, 1_010);
    // Prices are fresh: only the mints with no quote.
    assert_eq!(plan.quote, vec![pk(MEME), pk(99)]);
    assert!(!plan.groups);
    assert_eq!(plan.check, vec![pk(USDC), pk(MEME), pk(99)]);

    let later = cache.plan(&mints, 1_000 + GROUPS_TTL_SECS);
    assert_eq!(later.quote[0], WRAPPED_SOL_MINT);
    assert_eq!(later.quote.len(), 4);
    assert!(later.groups);
}

#[test]
fn a_mint_the_service_stops_pricing_loses_its_price() {
    let mut cache = market();
    cache.apply(
        MarketUpdate {
            quotes: Some(vec![(pk(USDC), None)]),
            ..Default::default()
        },
        2_000,
    );
    assert!(cache.quote(&pk(USDC)).is_none());
    assert!(cache.group(&pk(USDC)).is_some());
}

#[test]
fn dollars_and_amounts_read_well() {
    assert_eq!(format_usd(12431.084), "$12,431.08");
    assert_eq!(format_usd(0.0), "$0.00");
    assert_eq!(format_usd(-5.5), "-$5.50");
    assert_eq!(format_usd(0.00042137), "$0.0004214");
    assert_eq!(format_amount(1_234_567_890_000, 9, 4), "1,234.5678");
    assert_eq!(format_amount(5, 0, 4), "5");
    assert_eq!(format_amount(1_000_000, 6, 2), "1");
    assert_eq!(format_amount(1, 6, 4), "<0.0001");
    assert_eq!(format_value(0.0000008), "<$0.01");
    assert_eq!(format_value(3.5), "$3.50");
}

#[test]
fn prices_are_for_mainnet_only() {
    use crate::book::{Cluster, KnownCluster};
    assert!(prices_apply_to(&Cluster::MainnetBeta));
    assert!(!prices_apply_to(&Cluster::Devnet));
    assert!(prices_apply_to(&Cluster::Custom {
        name: "helius".into(),
        rpc_url: "https://x".into(),
        known: Some(KnownCluster::MainnetBeta),
    }));
}
