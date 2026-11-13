//! The shipped template "Create a vesting or payment stream", against
//! Streamflow's mainnet build.
//!
//! The chain carries copies of the mainnet accounts Streamflow's SDK names
//! as fixed (`packages/stream/solana/constants.ts` at fb236fff): its fee
//! oracle, its treasury and its automatic-withdrawal service. A token made
//! with Create a token is locked, and the contract account, the escrow and
//! the fee accounts are read back.

use crate::support::templates::{Reader, TemplateDesk};
use solana_pubkey::Pubkey;
use wallet_core::{IdlNetwork, KeypairSigner, WalletSigner, TOKEN_PROGRAM_ID};

const STREAMFLOW: Pubkey = Pubkey::from_str_const("strmRqUCoQUgGUan5YhzUZa6KqdzwX5L6FpUxfmKg5m");
const TREASURY: Pubkey = Pubkey::from_str_const("5SEpbdjFK5FxwTvfsGMXVQTD2v4M2c5tyRTxhdsPkgDw");
/// 2026-11-12 00:00 UTC.
const NOW: i64 = 1_794_441_600;
const DAY: u64 = 86_400;

/// The schedule and terms a contract account holds.
#[derive(Debug, PartialEq, Eq)]
struct Contract {
    sender: Pubkey,
    recipient: Pubkey,
    recipient_tokens: Pubkey,
    mint: Pubkey,
    escrow: Pubkey,
    fee_total: u64,
    partner: Pubkey,
    partner_fee_total: u64,
    start: u64,
    deposited: u64,
    period: u64,
    per_period: u64,
    cliff: u64,
    at_cliff: u64,
    /// Sender cancels, recipient cancels, automatic withdrawal, sender
    /// transfers, recipient transfers, top-up.
    flags: [bool; 6],
    name: String,
    withdraw_frequency: u64,
}

fn read_contract(data: &[u8]) -> Contract {
    let mut r = Reader::new(data, 49);
    let sender = r.pubkey();
    let _sender_tokens = r.pubkey();
    let recipient = r.pubkey();
    let recipient_tokens = r.pubkey();
    let mint = r.pubkey();
    let escrow = r.pubkey();
    let _treasury = r.pubkey();
    let _treasury_tokens = r.pubkey();
    let fee_total = r.u64();
    let _fee_withdrawn = r.u64();
    r.take(4); // fee percent, f32
    let partner = r.pubkey();
    let _partner_tokens = r.pubkey();
    let partner_fee_total = r.u64();
    let _partner_fee_withdrawn = r.u64();
    r.take(4);
    let start = r.u64();
    let deposited = r.u64();
    let period = r.u64();
    let per_period = r.u64();
    let cliff = r.u64();
    let at_cliff = r.u64();
    let flags = [r.bool(), r.bool(), r.bool(), r.bool(), r.bool(), r.bool()];
    let name = String::from_utf8(r.take(64).to_vec())
        .unwrap()
        .trim_end_matches('\0')
        .to_string();
    let withdraw_frequency = r.u64();
    Contract {
        sender,
        recipient,
        recipient_tokens,
        mint,
        escrow,
        fee_total,
        partner,
        partner_fee_total,
        start,
        deposited,
        period,
        per_period,
        cliff,
        at_cliff,
        flags,
        name,
        withdraw_frequency,
    }
}

fn token_balance(desk: &TemplateDesk, account: &Pubkey) -> u64 {
    let data = desk.data(account);
    u64::from_le_bytes(data[64..72].try_into().unwrap())
}

/// A desk whose wallet holds 1,000 of a token of 6 decimals, made with
/// Create a token.
fn desk_with_token() -> (TemplateDesk, Pubkey) {
    let desk = TemplateDesk::new(
        IdlNetwork::Mainnet,
        &[(STREAMFLOW, "streamflow.so")],
        &["streamflow.json"],
    );
    desk.node.chain().set_time(NOW, 900, NOW - 3_600);
    let mint = KeypairSigner::new_random();
    desk.land(
        "create-token",
        &["first_supply"],
        &[
            ("decimals", "6".to_string()),
            ("amount", "1000".to_string()),
        ],
        &[("mint", &mint)],
        &[],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    (desk, mint.pubkey())
}

#[test]
fn a_vesting_stream_holds_its_schedule_and_its_tokens_in_escrow() {
    let (desk, mint) = desk_with_token();
    let me = desk.me();
    let recipient = KeypairSigner::new_random().pubkey();
    let stream = KeypairSigner::new_random();
    let mine = wallet_core::spl::associated_token_address(&me, &mint, &TOKEN_PROGRAM_ID);
    let before = token_balance(&desk, &mine);
    let landed = desk
        .land(
            "create-stream",
            &[],
            &[
                ("recipient", recipient.to_string()),
                ("mint", mint.to_string()),
                ("amount", "100".to_string()),
                ("cliff_amount", "10".to_string()),
                ("per_period", "7.5".to_string()),
                ("period", DAY.to_string()),
                ("start", (NOW as u64 + DAY).to_string()),
                ("cliff", (NOW as u64 + 30 * DAY).to_string()),
                ("stream_name", "Payroll".to_string()),
            ],
            &[("stream", &stream)],
            &[],
        )
        .unwrap_or_else(|logs| panic!("{logs}"));

    let escrow = Pubkey::find_program_address(&[b"strm", stream.pubkey().as_ref()], &STREAMFLOW).0;
    let contract = read_contract(&desk.data(&stream.pubkey()));
    let recipient_tokens =
        wallet_core::spl::associated_token_address(&recipient, &mint, &TOKEN_PROGRAM_ID);
    assert_eq!(
        (contract.sender, contract.recipient, contract.mint),
        (me, recipient, mint)
    );
    assert_eq!(contract.escrow, escrow);
    assert_eq!(contract.recipient_tokens, recipient_tokens);
    assert_eq!(contract.partner, me);
    assert_eq!(
        (contract.start, contract.cliff),
        (NOW as u64 + DAY, NOW as u64 + 30 * DAY)
    );
    assert_eq!(
        (contract.deposited, contract.at_cliff, contract.per_period),
        (100_000_000, 10_000_000, 7_500_000)
    );
    assert_eq!((contract.period, contract.withdraw_frequency), (DAY, DAY));
    assert_eq!(contract.flags, [true, false, false, false, false, false]);
    assert_eq!(contract.name, "Payroll");

    // The escrow holds the deposit and Streamflow's fee, both taken from
    // this wallet; the recipient's token account was made, empty.
    let fees = contract.fee_total + contract.partner_fee_total;
    assert_eq!(token_balance(&desk, &escrow), 100_000_000 + fees);
    assert_eq!(before - token_balance(&desk, &mine), 100_000_000 + fees);
    assert_eq!(token_balance(&desk, &recipient_tokens), 0);
    let fee_tokens =
        wallet_core::spl::associated_token_address(&TREASURY, &mint, &TOKEN_PROGRAM_ID);
    assert!(desk.exists(&fee_tokens), "the treasury's token account");
    eprintln!(
        "fee {} + partner fee {}",
        contract.fee_total, contract.partner_fee_total
    );

    let sentences = desk.sentences(&landed.instructions);
    eprintln!("{sentences:#?}");
    assert!(sentences[0].contains("locks 100 of"), "{}", sentences[0]);
    assert!(
        sentences[0].contains("10 unlocks at the cliff"),
        "{}",
        sentences[0]
    );
    assert!(sentences[0].contains("7.5 every"), "{}", sentences[0]);
}

#[test]
fn a_stream_with_no_start_or_cliff_starts_when_it_lands_and_names_who_may_act() {
    let (desk, mint) = desk_with_token();
    let recipient = KeypairSigner::new_random().pubkey();
    let stream = KeypairSigner::new_random();
    desk.land(
        "create-stream",
        &[],
        &[
            ("recipient", recipient.to_string()),
            ("mint", mint.to_string()),
            ("amount", "12".to_string()),
            ("per_period", "1".to_string()),
            ("period", "3600".to_string()),
            ("sender_cancels", "false".to_string()),
            ("recipient_cancels", "true".to_string()),
            ("sender_transfers", "true".to_string()),
            ("recipient_transfers", "true".to_string()),
        ],
        &[("stream", &stream)],
        &[],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    let contract = read_contract(&desk.data(&stream.pubkey()));
    // A blank start is the moment it lands, and a blank cliff the start.
    assert_eq!(contract.start, NOW as u64);
    assert_eq!(contract.cliff, NOW as u64);
    assert_eq!(contract.at_cliff, 0);
    assert_eq!(contract.flags, [false, true, false, true, true, false]);
    assert_eq!(contract.name, "");
}

#[test]
fn a_stream_of_more_than_the_wallet_holds_is_refused() {
    let (desk, mint) = desk_with_token();
    let stream = KeypairSigner::new_random();
    let refused = desk
        .land(
            "create-stream",
            &[],
            &[
                (
                    "recipient",
                    KeypairSigner::new_random().pubkey().to_string(),
                ),
                ("mint", mint.to_string()),
                ("amount", "1000".to_string()),
                ("per_period", "1".to_string()),
                ("period", "60".to_string()),
            ],
            &[("stream", &stream)],
            &[],
        )
        .err()
        .expect("refused: the fee comes on top of all the wallet holds");
    assert!(!desk.exists(&stream.pubkey()), "{refused}");
}
