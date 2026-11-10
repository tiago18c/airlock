use super::*;
use crate::{NATIVE_MINT, TOKEN_PROGRAM_ID};

fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

const OWNER: u8 = 1;
const PAYER: u8 = 2;
const RECEIVER: u8 = 3;
const RENT: u64 = 2_039_280;

fn parties() -> Parties {
    Parties {
        owner: pk(OWNER),
        fee_payer: pk(PAYER),
        rent_payer: pk(PAYER),
        rent_to: pk(OWNER),
    }
}

fn mint(program: Pubkey) -> Mint {
    Mint {
        program,
        decimals: 6,
        transfer_fee_bps: None,
        transfer_hook: None,
        non_transferable: false,
        paused: false,
        frozen_by_default: false,
        burn_authority: None,
    }
}

fn account(program: Pubkey, mint: Pubkey, amount: u64) -> TokenAccount {
    TokenAccount {
        program,
        mint,
        owner: pk(OWNER),
        amount,
        frozen: false,
        requires_memo: false,
        close_authority: None,
        withheld: 0,
        confidential: false,
    }
}

/// A Token account at `address` of mint `mint_n`, holding `amount`.
fn held(address: u8, mint_n: u8, amount: u64) -> Held {
    Held {
        address: pk(address),
        lamports: RENT,
        account: account(TOKEN_PROGRAM_ID, pk(mint_n), amount),
        mint: Some(mint(TOKEN_PROGRAM_ID)),
    }
}

fn to_receiver() -> Choice {
    Choice::Move(Receiver {
        owner: pk(RECEIVER),
        existing: None,
    })
}

fn one(held: Held, choice: Choice) -> TokenPlan {
    plan(parties(), vec![(held, choice)], Limits::PLAIN)
}

/// The tag byte of each instruction of the plan's steps, with its program.
fn tags(plan: &TokenPlan) -> Vec<Vec<(Pubkey, u8)>> {
    plan.steps
        .iter()
        .map(|step| {
            step.instructions
                .iter()
                .map(|ix| (ix.program_id, ix.data.first().copied().unwrap_or(0)))
                .collect()
        })
        .collect()
}

const T: Pubkey = TOKEN_PROGRAM_ID;

#[test]
fn an_empty_account_is_closed_and_its_rent_comes_back() {
    let plan = one(held(10, 20, 0), Choice::Close);
    assert_eq!(plan.accounts[0].outcome, Outcome::Closed);
    assert_eq!(plan.returned(), RENT);
    assert_eq!(tags(&plan), vec![vec![(T, 9)]]);
    let close = &plan.steps[0].instructions[0];
    assert_eq!(close.accounts[0].pubkey, pk(10));
    // The rent goes back to the wallet, which signs.
    assert_eq!(close.accounts[1].pubkey, pk(OWNER));
    assert_eq!(close.accounts[2].pubkey, pk(OWNER));
    assert!(close.accounts[2].is_signer);
}

#[test]
fn an_empty_account_is_closed_whatever_was_chosen_but_leave() {
    for choice in [Choice::Burn, to_receiver()] {
        assert_eq!(
            one(held(10, 20, 0), choice).accounts[0].outcome,
            Outcome::Closed
        );
    }
    let left = one(held(10, 20, 0), Choice::Leave);
    assert_eq!(left.accounts[0].outcome, Outcome::Left(Blocked::Chosen));
    assert!(left.steps.is_empty());
    assert_eq!(left.returned(), 0);
}

#[test]
fn close_is_refused_while_the_account_holds_tokens() {
    let plan = one(held(10, 20, 5), Choice::Close);
    assert_eq!(
        plan.accounts[0].outcome,
        Outcome::Left(Blocked::HoldsTokens)
    );
    assert!(plan.steps.is_empty());
}

#[test]
fn burning_burns_the_whole_balance_then_closes() {
    let plan = one(held(10, 20, 1234), Choice::Burn);
    assert_eq!(plan.accounts[0].outcome, Outcome::Burned { amount: 1234 });
    assert_eq!(tags(&plan), vec![vec![(T, 15), (T, 9)]]);
    let burn = &plan.steps[0].instructions[0];
    assert_eq!(&burn.data[1..9], &1234u64.to_le_bytes());
    assert_eq!(burn.data[9], 6);
    assert_eq!(burn.accounts[1].pubkey, pk(20));
    assert!(burn.accounts[1].is_writable);
}

#[test]
fn moving_creates_the_receiving_account_sends_everything_and_closes() {
    let plan = one(held(10, 20, 77), to_receiver());
    let to = associated_token_address(&pk(RECEIVER), &pk(20), &T);
    assert_eq!(
        plan.accounts[0].outcome,
        Outcome::Moved {
            amount: 77,
            to,
            created: true,
            closed: true
        }
    );
    let ata = crate::ASSOCIATED_TOKEN_PROGRAM_ID;
    assert_eq!(tags(&plan), vec![vec![(ata, 1), (T, 12), (T, 9)]]);
    // The rent payer pays for the new account.
    assert_eq!(plan.steps[0].instructions[0].accounts[0].pubkey, pk(PAYER));
    assert_eq!(plan.steps[0].instructions[1].accounts[2].pubkey, to);
    assert_eq!(plan.returned(), RENT);
}

#[test]
fn moving_into_an_account_that_wants_a_memo_puts_one_right_before_the_transfer() {
    let mut receiving = account(T, pk(20), 0);
    receiving.owner = pk(RECEIVER);
    receiving.requires_memo = true;
    let plan = one(
        held(10, 20, 77),
        Choice::Move(Receiver {
            owner: pk(RECEIVER),
            existing: Some(receiving),
        }),
    );
    let memo_program = crate::MEMO_PROGRAM_ID;
    let ixs = &plan.steps[0].instructions;
    assert_eq!(ixs[0].program_id, memo_program);
    assert_eq!(ixs[0].data, MOVE_MEMO.as_bytes());
    assert_eq!((ixs[1].program_id, ixs[1].data[0]), (T, 12));
    assert!(matches!(
        plan.accounts[0].outcome,
        Outcome::Moved { created: false, .. }
    ));
}

#[test]
fn a_receiver_that_is_or_would_be_frozen_leaves_the_account() {
    let mut frozen = account(T, pk(20), 0);
    frozen.frozen = true;
    let plan = one(
        held(10, 20, 5),
        Choice::Move(Receiver {
            owner: pk(RECEIVER),
            existing: Some(frozen),
        }),
    );
    assert_eq!(
        plan.accounts[0].outcome,
        Outcome::Left(Blocked::ReceiverFrozen)
    );

    let mut item = held(10, 20, 5);
    item.mint.as_mut().unwrap().frozen_by_default = true;
    let plan = one(item, to_receiver());
    assert_eq!(
        plan.accounts[0].outcome,
        Outcome::Left(Blocked::ReceiverFrozen)
    );
}

#[test]
fn moving_to_the_same_account_is_refused() {
    let mut item = held(10, 20, 5);
    item.address = associated_token_address(&pk(RECEIVER), &pk(20), &T);
    item.account.owner = pk(RECEIVER);
    let parties = Parties {
        owner: pk(RECEIVER),
        ..parties()
    };
    let plan = plan(parties, vec![(item, to_receiver())], Limits::PLAIN);
    assert_eq!(
        plan.accounts[0].outcome,
        Outcome::Left(Blocked::ReceiverIsSource)
    );
}

#[test]
fn wrapped_sol_is_unwrapped_by_closing_it() {
    let mut item = held(10, 0, 5_000_000);
    item.account.mint = NATIVE_MINT;
    item.lamports = RENT + 5_000_000;
    for choice in [Choice::Close, Choice::Burn, to_receiver()] {
        let plan = one(item.clone(), choice);
        assert_eq!(
            plan.accounts[0].outcome,
            Outcome::Unwrapped {
                lamports: 5_000_000
            }
        );
        assert_eq!(tags(&plan), vec![vec![(T, 9)]]);
        assert_eq!(plan.returned(), RENT + 5_000_000);
    }
}

#[test]
fn withheld_fees_are_harvested_to_the_mint_before_the_close() {
    let t22 = TOKEN_2022_PROGRAM_ID;
    let mut item = held(10, 20, 9);
    item.account.program = t22;
    item.account.withheld = 3;
    item.mint = Some(mint(t22));
    let plan = one(item, Choice::Burn);
    assert_eq!(tags(&plan), vec![vec![(t22, 15), (t22, 26), (t22, 9)]]);
    let harvest = &plan.steps[0].instructions[1];
    assert_eq!(harvest.data, vec![26, 4]);
    assert_eq!(harvest.accounts[0].pubkey, pk(20));
    assert_eq!(harvest.accounts[1].pubkey, pk(10));
}

#[test]
fn what_cannot_be_done_says_why() {
    let mut frozen = held(10, 20, 5);
    frozen.account.frozen = true;
    assert_eq!(
        one(frozen, Choice::Burn).accounts[0].outcome,
        Outcome::Left(Blocked::Frozen)
    );

    let mut other = held(10, 20, 5);
    other.account.owner = pk(9);
    assert_eq!(
        one(other, Choice::Burn).accounts[0].outcome,
        Outcome::Left(Blocked::NotOwned { owner: pk(9) })
    );

    let mut unknown = held(10, 20, 5);
    unknown.mint = None;
    assert_eq!(
        one(unknown, Choice::Burn).accounts[0].outcome,
        Outcome::Left(Blocked::MintUnknown)
    );

    let mut confidential = held(10, 20, 0);
    confidential.account.confidential = true;
    assert_eq!(
        one(confidential, Choice::Close).accounts[0].outcome,
        Outcome::Left(Blocked::Confidential)
    );

    let mut locked = held(10, 20, 5);
    locked.account.close_authority = Some(pk(9));
    assert_eq!(
        one(locked, Choice::Burn).accounts[0].outcome,
        Outcome::Left(Blocked::CloseAuthority { authority: pk(9) })
    );
}

#[test]
fn a_token_that_cannot_be_sent_can_still_be_burned() {
    let mut item = held(10, 20, 5);
    item.mint.as_mut().unwrap().non_transferable = true;
    let moved = one(item.clone(), to_receiver());
    assert!(matches!(
        moved.accounts[0].outcome,
        Outcome::Left(Blocked::CannotMove(why)) if why.contains("non-transferable")
    ));
    assert_eq!(
        one(item, Choice::Burn).accounts[0].outcome,
        Outcome::Burned { amount: 5 }
    );
}

#[test]
fn a_mint_that_controls_burns_is_not_burned() {
    let mut paused = held(10, 20, 5);
    paused.mint.as_mut().unwrap().paused = true;
    assert!(matches!(
        one(paused, Choice::Burn).accounts[0].outcome,
        Outcome::Left(Blocked::CannotBurn(_))
    ));
    let mut permissioned = held(10, 20, 5);
    permissioned.mint.as_mut().unwrap().burn_authority = Some(pk(9));
    assert!(matches!(
        one(permissioned, Choice::Burn).accounts[0].outcome,
        Outcome::Left(Blocked::CannotBurn(_))
    ));
}

#[test]
fn a_move_out_of_an_account_someone_else_closes_leaves_it_open() {
    let mut item = held(10, 20, 5);
    item.account.close_authority = Some(pk(9));
    let plan = one(item, to_receiver());
    assert!(matches!(
        plan.accounts[0].outcome,
        Outcome::Moved { closed: false, .. }
    ));
    assert_eq!(plan.returned(), 0);
    let ata = crate::ASSOCIATED_TOKEN_PROGRAM_ID;
    assert_eq!(tags(&plan), vec![vec![(ata, 1), (T, 12)]]);
}

#[test]
fn many_accounts_are_packed_into_steps_that_fit() {
    let items: Vec<_> = (0..90u8)
        .map(|n| (held(n + 40, n + 160, 1), to_receiver()))
        .collect();
    let plan = plan(parties(), items, Limits::PLAIN);
    assert!(plan.steps.len() > 1);
    let mut seen = 0;
    for step in &plan.steps {
        seen += step.accounts.len();
        // Each account creates an associated token account (7 trace
        // entries), transfers and closes.
        assert!(step.accounts.len() * 9 <= Limits::PLAIN.max_trace);
        let wire = compile_v1(
            pk(PAYER),
            step.instructions.clone(),
            None,
            Some(Hash::default()),
        )
        .unwrap()
        .wire;
        assert!(wire.len() <= Limits::PLAIN.max_wire);
        // An account's instructions are never split between steps.
        assert_eq!(step.instructions.len(), step.accounts.len() * 3);
    }
    assert_eq!(seen, 90);
    // A wrapped wallet gets smaller steps.
    let items: Vec<_> = (0..90u8)
        .map(|n| (held(n + 40, n + 160, 1), to_receiver()))
        .collect();
    let wrapped = super::plan(parties(), items, Limits::WRAPPED);
    assert!(wrapped.steps.len() > plan.steps.len());
}

#[test]
fn accounts_left_alone_are_listed_with_their_reason() {
    let plan = plan(
        parties(),
        vec![
            (held(10, 20, 0), Choice::Close),
            (held(11, 21, 5), Choice::Leave),
            (held(12, 22, 5), Choice::Close),
        ],
        Limits::PLAIN,
    );
    let left: Vec<_> = plan
        .left()
        .map(|(planned, why)| (planned.address, why.clone()))
        .collect();
    assert_eq!(
        left,
        vec![(pk(11), Blocked::Chosen), (pk(12), Blocked::HoldsTokens)]
    );
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].accounts, vec![pk(10)]);
}
