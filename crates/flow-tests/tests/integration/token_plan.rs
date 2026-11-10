//! What `wallet_core::token_plan` builds, against the real Token, Token-2022,
//! associated-token and memo programs.
//!
//! A wallet holds an empty account, tokens to burn, tokens to move, wrapped
//! SOL, a Token-2022 token that charges a transfer fee with fees withheld
//! in the account, and a frozen account. The plan's steps are sent one
//! after another as the wallet, and the chain is read back after.

use crate::support::{logs, Chain, SOL};
use solana_pubkey::Pubkey;
use wallet_core::spl::{associated_token_address, Mint, TokenAccount};
use wallet_core::token_plan::{
    plan, Blocked, Choice, Held, Limits, Outcome, Parties, Receiver, TokenPlan,
};
use wallet_core::{
    KeypairSigner, WalletSigner, NATIVE_MINT, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};

const MINT_LEN: usize = 82;
const ACCOUNT_LEN: usize = 165;
const FEE_BPS: u16 = 100;

/// A Token mint with `supply` and 6 decimals, and no authorities.
fn mint_data(supply: u64) -> Vec<u8> {
    let mut data = vec![0u8; MINT_LEN];
    data[36..44].copy_from_slice(&supply.to_le_bytes());
    data[44] = 6;
    data[45] = 1;
    data
}

/// A Token-2022 mint that charges `FEE_BPS` on every transfer.
fn fee_mint_data(supply: u64) -> Vec<u8> {
    let mut data = mint_data(supply);
    data.resize(ACCOUNT_LEN, 0);
    data.push(1); // a mint
    let mut config = vec![0u8; 108];
    for fee in [72usize, 90] {
        // epoch (8), then the maximum fee and the basis points.
        config[fee + 8..fee + 16].copy_from_slice(&u64::MAX.to_le_bytes());
        config[fee + 16..fee + 18].copy_from_slice(&FEE_BPS.to_le_bytes());
    }
    tlv(&mut data, 1, &config);
    data
}

fn tlv(data: &mut Vec<u8>, kind: u16, value: &[u8]) {
    data.extend_from_slice(&kind.to_le_bytes());
    data.extend_from_slice(&(value.len() as u16).to_le_bytes());
    data.extend_from_slice(value);
}

fn account_data(mint: &Pubkey, owner: &Pubkey, amount: u64, frozen: bool) -> Vec<u8> {
    let mut data = vec![0u8; ACCOUNT_LEN];
    data[0..32].copy_from_slice(mint.as_ref());
    data[32..64].copy_from_slice(owner.as_ref());
    data[64..72].copy_from_slice(&amount.to_le_bytes());
    data[108] = if frozen { 2 } else { 1 };
    data
}

/// A Token-2022 account of a fee mint, with `withheld` fees in it.
fn fee_account_data(mint: &Pubkey, owner: &Pubkey, amount: u64, withheld: u64) -> Vec<u8> {
    let mut data = account_data(mint, owner, amount, false);
    data.push(2); // an account
    tlv(&mut data, 2, &withheld.to_le_bytes());
    data
}

struct Desk {
    chain: Chain,
    wallet: KeypairSigner,
}

impl Desk {
    fn owner(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    fn new_mint(&mut self, program: Pubkey, data: Vec<u8>) -> Pubkey {
        let mint = KeypairSigner::new_random().pubkey();
        self.chain.plant(mint, program, data);
        mint
    }

    fn new_account(&mut self, program: Pubkey, data: Vec<u8>) -> Pubkey {
        let account = KeypairSigner::new_random().pubkey();
        self.chain.plant(account, program, data);
        account
    }

    /// The account as the app reads it, with its mint.
    fn held(&self, address: Pubkey) -> Held {
        let raw = self.chain.account(&address).expect("the account exists");
        let account = TokenAccount::parse(&raw).expect("a token account");
        let mint = self
            .chain
            .account(&account.mint)
            .and_then(|mint| Mint::parse(&mint));
        Held {
            address,
            lamports: raw.lamports,
            account,
            mint,
        }
    }

    fn token(&self, address: &Pubkey) -> Option<TokenAccount> {
        TokenAccount::parse(&self.chain.account(address)?)
    }

    fn supply(&self, mint: &Pubkey) -> u64 {
        u64::from_le_bytes(self.chain.data(mint)[36..44].try_into().unwrap())
    }

    fn run(&mut self, plan: &TokenPlan) {
        for step in &plan.steps {
            self.chain
                .send(&[&self.wallet], step.instructions.clone())
                .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
        }
    }
}

#[test]
fn a_wallet_closes_burns_moves_and_unwraps_its_token_accounts() {
    let wallet = KeypairSigner::new_random();
    let mut chain = Chain::new();
    chain.fund(&wallet.pubkey(), 10 * SOL);
    let mut desk = Desk { chain, wallet };
    let owner = desk.owner();
    let receiver = KeypairSigner::new_random().pubkey();

    let empty_mint = desk.new_mint(TOKEN_PROGRAM_ID, mint_data(0));
    let empty = desk.new_account(
        TOKEN_PROGRAM_ID,
        account_data(&empty_mint, &owner, 0, false),
    );

    let spam_mint = desk.new_mint(TOKEN_PROGRAM_ID, mint_data(1_000));
    let spam = desk.new_account(
        TOKEN_PROGRAM_ID,
        account_data(&spam_mint, &owner, 500, false),
    );

    let kept_mint = desk.new_mint(TOKEN_PROGRAM_ID, mint_data(700));
    let kept = desk.new_account(
        TOKEN_PROGRAM_ID,
        account_data(&kept_mint, &owner, 700, false),
    );

    // Wrapped SOL: the wrapped amount sits above the account's rent, and
    // `is_native` records that rent.
    let rent = desk.chain.rent(ACCOUNT_LEN);
    let mut wrapped_data = account_data(&NATIVE_MINT, &owner, SOL, false);
    wrapped_data[109] = 1;
    wrapped_data[113..121].copy_from_slice(&rent.to_le_bytes());
    let wrapped = desk.new_account(TOKEN_PROGRAM_ID, wrapped_data);
    desk.chain.fund(&wrapped, SOL);

    let fee_mint = desk.new_mint(TOKEN_2022_PROGRAM_ID, fee_mint_data(1_300));
    let fee_burn = desk.new_account(
        TOKEN_2022_PROGRAM_ID,
        fee_account_data(&fee_mint, &owner, 300, 5),
    );
    let fee_move = desk.new_account(
        TOKEN_2022_PROGRAM_ID,
        fee_account_data(&fee_mint, &owner, 1_000, 0),
    );

    let frozen_mint = desk.new_mint(TOKEN_PROGRAM_ID, mint_data(1));
    let frozen = desk.new_account(
        TOKEN_PROGRAM_ID,
        account_data(&frozen_mint, &owner, 1, true),
    );

    let to_receiver = Choice::Move(Receiver {
        owner: receiver,
        existing: None,
    });
    let items = vec![
        (desk.held(empty), Choice::Close),
        (desk.held(spam), Choice::Burn),
        (desk.held(kept), to_receiver.clone()),
        (desk.held(wrapped), Choice::Close),
        (desk.held(fee_burn), Choice::Burn),
        (desk.held(fee_move), to_receiver.clone()),
        (desk.held(frozen), Choice::Burn),
    ];
    let returned_expected: u64 = items[..6].iter().map(|(held, _)| held.lamports).sum();
    let parties = Parties {
        owner,
        fee_payer: owner,
        rent_payer: owner,
        rent_to: owner,
    };
    let plan = plan(parties, items, Limits::PLAIN);
    assert_eq!(plan.returned(), returned_expected);
    assert_eq!(
        plan.left()
            .map(|(planned, why)| (planned.address, why.clone()))
            .collect::<Vec<_>>(),
        vec![(frozen, Blocked::Frozen)]
    );
    assert!(matches!(plan.accounts[3].outcome, Outcome::Unwrapped { lamports } if lamports == SOL));

    let before = desk.chain.balance(&owner);
    desk.run(&plan);

    // Every account but the frozen one is gone.
    for gone in [empty, spam, kept, wrapped, fee_burn, fee_move] {
        assert!(desk.chain.account(&gone).is_none(), "{gone} was closed");
    }
    assert!(desk.token(&frozen).is_some());

    // The burns took the tokens out of supply, and the harvest put the
    // withheld fees in the mint.
    assert_eq!(desk.supply(&spam_mint), 500);
    assert_eq!(desk.supply(&fee_mint), 1_000);
    let fee_mint_data = desk.chain.data(&fee_mint);
    let mint_withheld = &fee_mint_data[ACCOUNT_LEN + 1 + 4 + 64..ACCOUNT_LEN + 1 + 4 + 72];
    assert_eq!(u64::from_le_bytes(mint_withheld.try_into().unwrap()), 5);

    // The receiver's new accounts hold what moved, less the transfer fee.
    let kept_to = associated_token_address(&receiver, &kept_mint, &TOKEN_PROGRAM_ID);
    assert_eq!(desk.token(&kept_to).unwrap().amount, 700);
    let fee_to = associated_token_address(&receiver, &fee_mint, &TOKEN_2022_PROGRAM_ID);
    let fee_to = desk.token(&fee_to).unwrap();
    assert_eq!(fee_to.amount, 1_000 - 1_000 * u64::from(FEE_BPS) / 10_000);
    assert_eq!(fee_to.owner, receiver);

    // The wallet got every closed account's lamports back, the unwrapped
    // SOL among them, less the fees and the rent of the two accounts it
    // created for the receiver.
    let created_rent = desk.chain.rent(ACCOUNT_LEN)
        + desk.chain.rent(
            desk.chain
                .data(&associated_token_address(
                    &receiver,
                    &fee_mint,
                    &TOKEN_2022_PROGRAM_ID,
                ))
                .len(),
        );
    let fees = 5_000 * plan.steps.len() as u64;
    assert_eq!(
        desk.chain.balance(&owner),
        before + returned_expected - created_rent - fees
    );
}

#[test]
fn many_accounts_close_in_several_steps_that_each_land() {
    let wallet = KeypairSigner::new_random();
    let mut chain = Chain::new();
    chain.fund(&wallet.pubkey(), 10 * SOL);
    let mut desk = Desk { chain, wallet };
    let owner = desk.owner();
    let receiver = KeypairSigner::new_random().pubkey();

    let mut items = Vec::new();
    for _ in 0..60 {
        let mint = desk.new_mint(TOKEN_PROGRAM_ID, mint_data(10));
        let account = desk.new_account(TOKEN_PROGRAM_ID, account_data(&mint, &owner, 10, false));
        items.push((
            desk.held(account),
            Choice::Move(Receiver {
                owner: receiver,
                existing: None,
            }),
        ));
    }
    let accounts: Vec<Pubkey> = items.iter().map(|(held, _)| held.address).collect();
    let parties = Parties {
        owner,
        fee_payer: owner,
        rent_payer: owner,
        rent_to: owner,
    };
    let plan = plan(parties, items, Limits::PLAIN);
    assert!(plan.steps.len() > 1, "{} steps", plan.steps.len());
    desk.run(&plan);
    for account in accounts {
        assert!(desk.chain.account(&account).is_none());
    }
}
