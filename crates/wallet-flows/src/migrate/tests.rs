use super::*;
use crate::testing::{mint_account, nonce_account, pk, stake_account, token_account, FakeRpc};
use wallet_core::{RouteStep, WalletId, TOKEN_PROGRAM_ID};

const SOURCE: u8 = 1;
const TO: u8 = 2;
const PAYER: u8 = 3;
const NOW: Now = Now {
    epoch: 20,
    unix_time: 1_000_000,
};

fn plain() -> SigningRoute {
    SigningRoute {
        address: pk(SOURCE),
        steps: vec![RouteStep::Key {
            wallet: WalletId::new(),
            pubkey: pk(SOURCE),
        }],
        fee_payer: None,
        durable_nonce: None,
    }
}

fn system(lamports: u64) -> RawAccount {
    RawAccount {
        lamports,
        data: Vec::new(),
        owner: SYSTEM_PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    }
}

fn held(address: u8, mint: u8, amount: u64) -> Held {
    let raw = token_account(TOKEN_PROGRAM_ID, pk(mint), pk(SOURCE), amount);
    Held {
        address: pk(address),
        lamports: 2_039_280,
        account: TokenAccount::parse(&raw).unwrap(),
        mint: Mint::parse(&mint_account(TOKEN_PROGRAM_ID, 6)),
    }
}

/// A wallet holding 1 SOL, a token account with tokens, an empty one, a
/// stake account and a nonce account, all its own.
fn snapshot(rpc: &FakeRpc, route: &SigningRoute) -> Snapshot {
    let mut snapshot = pollster::block_on(snapshot_with(
        rpc,
        route,
        pk(TO),
        vec![held(10, 20, 500), held(11, 21, 0)],
        NOW,
    ))
    .unwrap();
    snapshot.stakes =
        vec![StakeAccount::parse(pk(30), &stake_account(pk(SOURCE), pk(SOURCE), 3)).unwrap()];
    snapshot.nonces = vec![NonceHeld {
        address: pk(40),
        lamports: 1_447_680,
    }];
    snapshot
}

fn node() -> FakeRpc {
    FakeRpc::default().with_account(pk(SOURCE), system(1_000_000_000))
}

#[test]
fn a_destination_that_cannot_hold_what_moves_is_refused() {
    let program = RawAccount {
        executable: true,
        owner: pk(99),
        ..system(1)
    };
    let foreign = RawAccount {
        owner: pk(98),
        data: vec![1; 40],
        ..system(5)
    };
    let rpc = FakeRpc::default()
        .with_account(pk(50), program)
        .with_account(pk(51), mint_account(TOKEN_PROGRAM_ID, 6))
        .with_account(pk(52), token_account(TOKEN_PROGRAM_ID, pk(20), pk(9), 0))
        .with_account(pk(53), stake_account(pk(9), pk(9), 1))
        .with_account(pk(54), foreign);
    let check = |to: u8, known: bool| {
        pollster::block_on(check_destination(&rpc, pk(SOURCE), pk(to), known))
            .map_err(|err| err.to_string())
    };
    assert!(check(SOURCE, true)
        .unwrap_err()
        .contains("the wallet being moved"));
    assert!(check(50, true).unwrap_err().contains("program"));
    assert!(check(51, true).unwrap_err().contains("mint"));
    assert!(check(52, true).unwrap_err().contains("token account"));
    assert!(check(53, true).unwrap_err().contains("stake account"));
    assert!(check(54, false).unwrap_err().contains("Add it as a wallet"));
    // A wallet of this vault that a program owns, such as a Vector account.
    assert_eq!(check(54, true), Ok(DestinationCheck { unused: false }));
}

#[test]
fn an_address_with_no_sol_and_no_history_is_flagged_unused() {
    let mut rpc = FakeRpc::default().with_account(pk(61), system(5));
    rpc.used = vec![pk(62)];
    let check =
        |to: u8| pollster::block_on(check_destination(&rpc, pk(SOURCE), pk(to), false)).unwrap();
    assert!(check(60).unused);
    assert!(!check(61).unused);
    // Used before, though empty now.
    assert!(!check(62).unused);
}

#[test]
fn the_screen_starts_by_moving_everything_and_burning_what_is_flagged() {
    let rpc = node();
    let mut route = plain();
    let snap = snapshot(&rpc, &route);
    let choices = default_choices(&route, &snap, &BTreeSet::from([pk(10), pk(11)]));
    assert_eq!(choices.tokens[&pk(10)], TokenMove::Burn);
    // Nothing to burn in an empty one: it is just closed.
    assert_eq!(choices.tokens[&pk(11)], TokenMove::Move);
    assert_eq!(choices.stakes[&pk(30)], StakeChoice::BOTH);
    assert!(choices.nonces.contains(&pk(40)));
    // The wallet's own durable nonce stays.
    route.durable_nonce = Some(pk(40));
    assert!(default_choices(&route, &snap, &BTreeSet::new())
        .nonces
        .is_empty());
}

#[test]
fn tokens_go_first_then_one_last_step_hands_over_closes_and_sweeps_exactly() {
    let rpc = node();
    let route = plain();
    let snap = snapshot(&rpc, &route);
    let choices = default_choices(&route, &snap, &BTreeSet::new());
    let made = plan(&route, &snap, &choices).unwrap();
    assert_eq!(made.tokens.steps.len(), 1);
    assert_eq!(made.final_steps(), 1);
    assert_eq!(made.steps(), 2);
    // 1 SOL, both accounts' rent and the nonce's, less the receiving
    // account it creates.
    assert_eq!(
        made.sweep_estimate,
        1_000_000_000 + 2 * 2_039_280 + 1_447_680 - 2_039_280
    );

    let (first, _) = pollster::block_on(prepare(&rpc, &route, &snap, &made, 0))
        .unwrap()
        .unwrap();
    let note = first.note.plain();
    assert!(note.starts_with("Step 1 of 2. This moves the tokens of one token account, and closes one token account with nothing in it of"), "{note}");

    // Once the token step landed, the tokens are gone and the sweep is next.
    let mut after = snap.clone();
    after.tokens.clear();
    let made = plan(&route, &after, &choices).unwrap();
    assert_eq!(made.steps(), 1);
    let (last, _) = pollster::block_on(prepare(&rpc, &route, &after, &made, 1))
        .unwrap()
        .unwrap();
    let tx = decode_v1_transaction(&last.wire).unwrap();
    let programs: Vec<Pubkey> = tx.instructions.iter().map(|ix| ix.program_id).collect();
    assert_eq!(
        programs,
        vec![
            STAKE_PROGRAM_ID,
            STAKE_PROGRAM_ID,
            SYSTEM_PROGRAM_ID,
            SYSTEM_PROGRAM_ID
        ]
    );
    // The transfer: tag 2, then everything less the one signature's fee.
    let transfer = &tx.instructions[3].data;
    assert_eq!(u32::from_le_bytes(transfer[0..4].try_into().unwrap()), 2);
    assert_eq!(
        u64::from_le_bytes(transfer[4..12].try_into().unwrap()),
        1_000_000_000 + 1_447_680 - 5_000
    );
    let note = last.note.plain();
    assert!(
        note.starts_with(
            "Step 2 of 2. This hands a stake account over, and closes a nonce account of"
        ),
        "{note}"
    );
    assert!(note.contains("empty."), "{note}");

    // Then nothing is left.
    let mut done = after.clone();
    done.stakes.clear();
    done.nonces.clear();
    let gone = FakeRpc::default();
    let made = plan(&route, &done, &Choices::default()).unwrap();
    assert!(pollster::block_on(prepare(&gone, &route, &done, &made, 2))
        .unwrap()
        .is_none());
}

#[test]
fn a_wallet_another_pays_for_sends_everything() {
    let rpc = node();
    let mut route = plain();
    route.fee_payer = Some((WalletId::new(), pk(PAYER)));
    let mut snap = snapshot(&rpc, &route);
    snap.tokens.clear();
    snap.stakes.clear();
    snap.nonces.clear();
    let made = plan(&route, &snap, &Choices::default()).unwrap();
    let (last, _) = pollster::block_on(prepare(&rpc, &route, &snap, &made, 0))
        .unwrap()
        .unwrap();
    let tx = decode_v1_transaction(&last.wire).unwrap();
    assert_eq!(tx.fee_payer(), Some(pk(PAYER)));
    let transfer = &tx.instructions[0].data;
    assert_eq!(
        u64::from_le_bytes(transfer[4..12].try_into().unwrap()),
        1_000_000_000
    );
    assert!(!last.note.plain().contains("Step"));
}

#[test]
fn an_account_a_program_owns_keeps_its_rent() {
    let vector = RawAccount {
        owner: pk(77),
        data: vec![0; 100],
        ..system(5_000_000)
    };
    let rpc = FakeRpc::default().with_account(pk(SOURCE), vector);
    let mut route = plain();
    route.fee_payer = Some((WalletId::new(), pk(PAYER)));
    let snap = pollster::block_on(snapshot_with(&rpc, &route, pk(TO), Vec::new(), NOW)).unwrap();
    // The fake node's rent for any size.
    assert_eq!(snap.rent_exempt, 2_282_880);
    let made = plan(&route, &snap, &Choices::default()).unwrap();
    let (last, _) = pollster::block_on(prepare(&rpc, &route, &snap, &made, 0))
        .unwrap()
        .unwrap();
    let tx = decode_v1_transaction(&last.wire).unwrap();
    let transfer = &tx.instructions[0].data;
    assert_eq!(
        u64::from_le_bytes(transfer[4..12].try_into().unwrap()),
        5_000_000 - 2_282_880
    );
    assert!(last
        .note
        .plain()
        .contains("the rent its account has to keep"));
}

#[test]
fn the_wallets_own_nonce_stays_and_the_rest_reads_from_the_node() {
    let (nonce, _) = nonce_account(pk(SOURCE));
    let rpc =
        node().with_program_accounts(pk(SOURCE), NONCE_AUTHORITY_OFFSET, vec![(pk(40), nonce)]);
    let mut route = plain();
    route.durable_nonce = Some(pk(40));
    let snap = pollster::block_on(read(&rpc, &route, pk(TO), NOW.unix_time)).unwrap();
    assert_eq!(snap.nonces.len(), 1);
    assert!(snap.nonces_unread.is_none());
    let made = plan(
        &route,
        &snap,
        &default_choices(&route, &snap, &BTreeSet::new()),
    )
    .unwrap();
    assert_eq!(
        made.nonces,
        vec![(
            pk(40),
            NonceOutcome::Left(wallet_core::migrate::NonceBlocked::InUse)
        )]
    );
}
