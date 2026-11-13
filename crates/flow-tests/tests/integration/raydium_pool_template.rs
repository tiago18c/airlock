//! The shipped template "Create a Raydium pool", against the build of
//! Raydium's constant-product program that devnet runs.
//!
//! The chain carries copies of two devnet accounts: fee-tier config 0, which
//! the template fills in, and the token account the program collects its
//! pool creation fee in. The pool is created once with token A's mint
//! sorting first and once with token B's, and the vaults, the wallet's token
//! accounts, its pool tokens and the fee are read back.

use crate::support::templates::TemplateDesk;
use solana_pubkey::Pubkey;
use wallet_core::{IdlNetwork, KeypairSigner, WalletSigner, TOKEN_PROGRAM_ID};

const RAYDIUM: Pubkey = Pubkey::from_str_const("DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb");
const CONFIG: Pubkey = Pubkey::from_str_const("5MxLgy9oPdTC3YgkiePHqr3EoCRD9uLVYRQS2ANAs7wy");
const POOL_FEE_ACCOUNT: Pubkey =
    Pubkey::from_str_const("3oE58BKVt8KuYkGxx8zBojugnymWmBiyafWgMrnb6eYy");
/// What config 0 charges to create a pool, in lamports: 0.15 SOL.
const POOL_FEE: u64 = 150_000_000;
/// The pool tokens the program keeps back from the first deposit.
const LOCKED_POOL_TOKENS: u64 = 100;

fn token_balance(desk: &TemplateDesk, account: &Pubkey) -> u64 {
    let data = desk.data(account);
    u64::from_le_bytes(data[64..72].try_into().unwrap())
}

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &RAYDIUM).0
}

/// Two new mint keys, the first sorting before the second or after it.
fn mints(a_first: bool) -> (KeypairSigner, KeypairSigner) {
    loop {
        let (a, b) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        if (a.pubkey() < b.pubkey()) == a_first {
            return (a, b);
        }
    }
}

/// Creates a pool of 1.5 of token A (6 decimals, of a supply of 10) and 2 of
/// token B (9 decimals, of a supply of 5), and reads everything back.
fn a_pool_is_created(a_first: bool) {
    let desk = TemplateDesk::new(
        IdlNetwork::Devnet,
        &[(RAYDIUM, "raydium_cp_swap.so")],
        &["raydium-cp-swap.json"],
    );
    let me = desk.me();
    let (mint_a, mint_b) = mints(a_first);
    let (a, b) = (mint_a.pubkey(), mint_b.pubkey());
    let fee_before = desk.balance(&POOL_FEE_ACCOUNT);
    let landed = desk
        .land(
            "create-raydium-pool",
            &[],
            &[
                ("decimals_a", "6".to_string()),
                ("decimals_b", "9".to_string()),
                ("supply_a", "10".to_string()),
                ("supply_b", "5".to_string()),
                ("deposit_a", "1.5".to_string()),
                ("deposit_b", "2".to_string()),
            ],
            &[("mint_a", &mint_a), ("mint_b", &mint_b)],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));

    // The pool's address comes from the config and the mints in address
    // order, whichever the form asked for first.
    let (token_0, token_1) = if a < b { (a, b) } else { (b, a) };
    let pool = pda(&[b"pool", CONFIG.as_ref(), token_0.as_ref(), token_1.as_ref()]);
    assert!(desk.exists(&pool), "the pool at its derived address");
    assert!(desk.exists(&pda(&[b"observation", pool.as_ref()])));

    // Each vault holds its own token's deposit, in that token's base units.
    let vault = |mint: &Pubkey| pda(&[b"pool_vault", pool.as_ref(), mint.as_ref()]);
    assert_eq!(token_balance(&desk, &vault(&a)), 1_500_000);
    assert_eq!(token_balance(&desk, &vault(&b)), 2_000_000_000);

    // The wallet keeps the rest of each supply.
    let mine =
        |mint: &Pubkey| wallet_core::spl::associated_token_address(&me, mint, &TOKEN_PROGRAM_ID);
    assert_eq!(token_balance(&desk, &mine(&a)), 8_500_000);
    assert_eq!(token_balance(&desk, &mine(&b)), 3_000_000_000);

    // And holds the pool's tokens: the square root of the product of the
    // two deposits, less what the program keeps back.
    let lp_mint = pda(&[b"pool_lp_mint", pool.as_ref()]);
    let liquidity = (1_500_000u128 * 2_000_000_000u128).isqrt() as u64;
    assert_eq!(
        token_balance(&desk, &mine(&lp_mint)),
        liquidity - LOCKED_POOL_TOKENS
    );

    // The program took config 0's pool creation fee from the wallet.
    assert_eq!(desk.balance(&POOL_FEE_ACCOUNT) - fee_before, POOL_FEE);

    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
    assert_eq!(sentences.len(), 9);
    let create = &sentences[8];
    assert!(create.contains(&pool.to_string()), "{create}");
    assert!(
        create.find(&token_0.to_string()) < create.find(&token_1.to_string()),
        "{create}"
    );
}

#[test]
fn a_pool_is_created_when_token_a_sorts_first() {
    a_pool_is_created(true);
}

#[test]
fn a_pool_is_created_when_token_b_sorts_first() {
    a_pool_is_created(false);
}
