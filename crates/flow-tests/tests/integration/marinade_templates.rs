//! The shipped Marinade templates, against Marinade's mainnet build and
//! copies of the mainnet accounts its one instance keeps: its state, the
//! mSOL mint, the reserve, the liquidity pool's two legs and the treasury's
//! mSOL account (read at the slot `fixtures/marinade.json` records).
//!
//! The program checks every one of the addresses the templates fix: the
//! PDAs by their seeds, the rest against what its state records. Each run
//! lands, then reads back the balances that moved.

use crate::support::templates::TemplateDesk;
use crate::support::SOL;
use solana_pubkey::Pubkey;
use wallet_core::{IdlNetwork, KeypairSigner, WalletSigner, TOKEN_PROGRAM_ID};

const MARINADE: Pubkey = Pubkey::from_str_const("MarBmsSgKXdrN1egZf5sqe1TMai9K1rChYNDJgjq7aD");
const STATE: Pubkey = Pubkey::from_str_const("8szGkuLTAux9XMgZ2vtY39jVSowEcpBfFfD8hXSEqdGC");
const MSOL: Pubkey = Pubkey::from_str_const("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So");
const RESERVE: Pubkey = Pubkey::from_str_const("Du3Ysj1wKbxPKkuPPnvzQLQh8oMSVifs3jGZjJWXFmHN");
const SOL_LEG: Pubkey = Pubkey::from_str_const("UefNb6z6yvArqe4cJHTXCqStRsKmWhGxnZzuHbikP5Q");
const MSOL_LEG: Pubkey = Pubkey::from_str_const("7GgPYjS5Dza89wV6FpZ23kUJRG5vbQ1GM25ezspYFSoE");
const MSOL_LEG_AUTHORITY: Pubkey =
    Pubkey::from_str_const("EyaSjUtSgo9aRD1f8LWXwdvkpDTmXAW54yoSHZRF14WL");
const MINT_AUTHORITY: Pubkey =
    Pubkey::from_str_const("3JLPCS1qM2zRw3Dp6V4hZnYHd4toMNPkNesXdX9tg6KM");
const TREASURY_MSOL: Pubkey =
    Pubkey::from_str_const("B1aLzaNMeFVAyQ6f3XbbUyKcH2YPHu2fqiEagmiF23VR");
/// The epoch of the slot the fixture was read at, and a time in it.
const EPOCH: u64 = 880;
const NOW: i64 = 1_794_441_600;

fn desk() -> TemplateDesk {
    let desk = TemplateDesk::new(
        IdlNetwork::Mainnet,
        &[(MARINADE, "marinade.so")],
        &["marinade.json"],
    );
    desk.node.chain().set_time(NOW, EPOCH, NOW - 7_200);
    desk
}

fn msol_balance(desk: &TemplateDesk, owner: &Pubkey) -> u64 {
    let account = wallet_core::spl::associated_token_address(owner, &MSOL, &TOKEN_PROGRAM_ID);
    let data = desk.data(&account);
    if data.is_empty() {
        return 0;
    }
    u64::from_le_bytes(data[64..72].try_into().unwrap())
}

/// The delayed-unstake fee Marinade's state sets, in millionths
/// (`delayed_unstake_fee`, a `FeeCents`: hundredths of a basis point).
fn delayed_unstake_fee(desk: &TemplateDesk) -> u32 {
    let state = desk.data(&STATE);
    u32::from_le_bytes(state[609..613].try_into().unwrap())
}

/// Stakes `sol` from the wallet with the template.
fn stake(desk: &TemplateDesk, sol: &str) -> Vec<String> {
    let landed = desk
        .land(
            "stake-marinade",
            &[],
            &[("amount", sol.to_string())],
            &[],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));
    desk.sentences(&landed.instructions)
}

#[test]
fn the_fixed_accounts_are_marinades_own() {
    // The PDAs from the state's address, as the program's seeds make them.
    let pda = |seed: &[u8]| Pubkey::find_program_address(&[STATE.as_ref(), seed], &MARINADE).0;
    assert_eq!(pda(b"reserve"), RESERVE);
    assert_eq!(pda(b"st_mint"), MINT_AUTHORITY);
    assert_eq!(pda(b"liq_sol"), SOL_LEG);
    assert_eq!(pda(b"liq_st_sol_authority"), MSOL_LEG_AUTHORITY);
    // The rest as the state records them.
    let desk = desk();
    let state = desk.data(&STATE);
    assert_eq!(Pubkey::try_from(&state[8..40]).unwrap(), MSOL);
    assert_eq!(Pubkey::try_from(&state[104..136]).unwrap(), TREASURY_MSOL);
    assert!(
        state.windows(32).any(|window| window == MSOL_LEG.as_ref()),
        "the state names the pool's mSOL leg"
    );
}

#[test]
fn the_templates_name_the_delayed_unstake_fee_the_state_keeps() {
    let desk = desk();
    let fee = delayed_unstake_fee(&desk);
    // Millionths to a percentage as the summaries write it: 2000 is 0.2 %.
    let percent = format!("{} %", fee as f64 / 10_000.0);
    for id in ["stake-marinade", "order-unstake-marinade"] {
        let template = wallet_core::builtin_template(id)
            .unwrap()
            .template()
            .unwrap();
        let summary = template.summary.unwrap_or_default();
        assert!(
            summary.contains("delayed-unstake fee") && summary.contains(&percent),
            "{id} names the {percent} fee: {summary}"
        );
        assert!(
            summary.contains("Marinade sets it"),
            "{id} says the fee can change: {summary}"
        );
        for promise in ["no fee", "without one", "without a fee"] {
            assert!(!summary.contains(promise), "{id}: {summary}");
        }
    }
}

#[test]
fn staking_sol_gives_mint_worth_of_msol_in_the_wallets_token_account() {
    let desk = desk();
    let me = desk.me();
    let before = desk.balance(&me);
    let pooled = desk.balance(&RESERVE) + desk.balance(&SOL_LEG);
    let sentences = stake(&desk, "10");
    let msol = msol_balance(&desk, &me);
    // mSOL is worth more than SOL: fewer than 10 come back, and more than 7.
    assert!(msol > 7 * SOL && msol < 10 * SOL, "{msol}");
    let account_rent = desk.node.chain().rent(165);
    // The SOL, the rent of the mSOL account, and the transaction's fee.
    assert_eq!(before - desk.balance(&me), 10 * SOL + account_rent + 5_000);
    assert_eq!(
        desk.balance(&RESERVE) + desk.balance(&SOL_LEG),
        pooled + 10 * SOL
    );
    eprintln!("{sentences:#?}");
    assert!(sentences[1].contains("stakes 10 SOL"), "{}", sentences[1]);
}

#[test]
fn unstaking_now_swaps_msol_for_sol_less_the_pools_fee() {
    let desk = desk();
    let me = desk.me();
    stake(&desk, "20");
    let held = msol_balance(&desk, &me);
    let to = KeypairSigner::new_random().pubkey();
    let treasury = desk.data(&TREASURY_MSOL);
    let treasury_before = u64::from_le_bytes(treasury[64..72].try_into().unwrap());
    let landed = desk
        .land(
            "unstake-marinade",
            &[],
            &[("amount", "5".to_string()), ("to", to.to_string())],
            &[],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));
    assert_eq!(msol_balance(&desk, &me), held - 5 * SOL);
    // 5 mSOL are worth more than 5 SOL; the pool keeps a fee of at most 3 %.
    let got = desk.balance(&to);
    assert!(got > 5 * SOL * 97 / 100, "{got}");
    let treasury = desk.data(&TREASURY_MSOL);
    let treasury_after = u64::from_le_bytes(treasury[64..72].try_into().unwrap());
    assert!(
        treasury_after > treasury_before,
        "the treasury takes its cut"
    );
    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
    assert!(sentences[0].contains("unstakes 5 mSOL"), "{}", sentences[0]);
}

#[test]
fn a_delayed_unstake_makes_a_ticket_claimable_from_the_next_epoch() {
    let desk = desk();
    let me = desk.me();
    stake(&desk, "20");
    let held = msol_balance(&desk, &me);
    let ticket = KeypairSigner::new_random();
    let landed = desk
        .land(
            "order-unstake-marinade",
            &[],
            &[("amount", "4".to_string())],
            &[("ticket", &ticket)],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));
    assert_eq!(msol_balance(&desk, &me), held - 4 * SOL, "burned");
    let data = desk.data(&ticket.pubkey());
    assert_eq!(data.len(), 88);
    assert_eq!(Pubkey::try_from(&data[8..40]).unwrap(), STATE);
    assert_eq!(Pubkey::try_from(&data[40..72]).unwrap(), me, "beneficiary");
    let worth = u64::from_le_bytes(data[72..80].try_into().unwrap());
    // The ticket is worth the SOL the 4 mSOL stood for, at the rate the
    // stake above got, less the delayed-unstake fee Marinade's state sets.
    let fee = delayed_unstake_fee(&desk);
    assert!(fee > 0, "the fixture's state keeps a delayed-unstake fee");
    let at_rate = (4 * SOL as u128 * 20 * SOL as u128 / held as u128) as u64;
    let less_fee = at_rate - (at_rate as u128 * fee as u128 / 1_000_000) as u64;
    assert!(
        worth.abs_diff(less_fee) <= 2,
        "the ticket holds {worth}; {at_rate} less a fee of {fee} millionths is {less_fee}"
    );
    assert_eq!(u64::from_le_bytes(data[80..88].try_into().unwrap()), EPOCH);
    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
    assert!(
        sentences[1].contains("delayed unstake of 4 mSOL"),
        "{}",
        sentences[1]
    );

    let claim = |desk: &TemplateDesk| {
        desk.land(
            "claim-marinade-ticket",
            &[],
            &[("ticket", ticket.pubkey().to_string())],
            &[],
            &[],
        )
    };
    // Not in the epoch it was made in, nor in the first 30 minutes of the
    // next.
    assert!(claim(&desk).is_err());
    desk.node
        .chain()
        .set_time(NOW + 3_600, EPOCH + 1, NOW + 3_000);
    assert!(claim(&desk).is_err());
    desk.node
        .chain()
        .set_time(NOW + 7_200, EPOCH + 1, NOW + 3_000);
    let before = desk.balance(&me);
    let ticket_rent = desk.balance(&ticket.pubkey());
    let landed = claim(&desk).unwrap_or_else(|logs| panic!("{logs}"));
    assert!(!desk.exists(&ticket.pubkey()), "the ticket is closed");
    assert_eq!(desk.balance(&me), before + worth + ticket_rent - 5_000);
    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
}
