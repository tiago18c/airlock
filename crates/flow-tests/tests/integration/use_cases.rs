//! The use cases of `wallet-flows`, with the local chain as the node.
//!
//! The other files here send what `wallet-core` builds. This one goes through
//! the functions a screen calls: the use case asks the node what it needs,
//! compiles, and the signed bytes are submitted through the same port.

use crate::support::node::Node;
use crate::support::{Chain, SOL};
use solana_pubkey::Pubkey;
use std::sync::Mutex;
use wallet_core::stake::{stake_address, StakeAccount, STAKE_ACCOUNT_LEN};
use wallet_core::{
    anchor_idl_account_data, anchor_idl_address, apply_local_signatures,
    program_metadata_account_data, program_metadata_idl_address, signing_route, IdlOrigin,
    KeypairSigner, WalletRecord, WalletSigner,
};
use wallet_flows::idl::{counterpart, published};
use wallet_flows::stake::{prepare, StakeRequest};
use wallet_flows::FlowError;

/// A hot wallet with funds, the way the vault holds one.
struct Desk {
    node: Node,
    wallets: Vec<WalletRecord>,
    signer: KeypairSigner,
    rent: u64,
}

impl Desk {
    fn new() -> Self {
        let mut chain = Chain::new();
        let signer = KeypairSigner::new_random();
        chain.fund(&signer.pubkey(), 100 * SOL);
        let rent = chain.rent(STAKE_ACCOUNT_LEN as usize);
        let wallet = WalletRecord::from_keypair_seed("Hot", signer.seed_bytes());
        Self {
            node: Node(Mutex::new(chain)),
            wallets: vec![wallet],
            signer,
            rent,
        }
    }

    fn owner(&self) -> Pubkey {
        self.signer.pubkey()
    }

    /// Prepare `request` for the wallet, sign what comes back and submit it.
    fn run(&self, request: StakeRequest) -> Result<wallet_core::Phrase, FlowError> {
        let route = signing_route(&self.wallets, self.wallets[0].id).expect("a route");
        let prepared = pollster::block_on(prepare(&self.node, &route, request, self.rent))?;
        let signed = apply_local_signatures(&prepared.wire, &[&self.signer as &dyn WalletSigner])
            .expect("signs");
        pollster::block_on(wallet_flows::send::submit(&self.node, &signed))?;
        Ok(prepared.note)
    }

    fn stake(&self, index: u32) -> Option<StakeAccount> {
        let address = stake_address(&self.owner(), index).unwrap();
        let account = self.node.chain().account(&address)?;
        // As a snapshot lists it: with the cluster's current rent.
        let mut stake = StakeAccount::parse(address, &account)?;
        stake.rent_reserve = self.rent;
        Some(stake)
    }
}

#[test]
fn each_new_stake_account_lands_at_the_next_free_address() {
    let desk = Desk::new();

    let note = desk
        .run(StakeRequest::New {
            lamports: 10 * SOL,
            vote: None,
        })
        .unwrap();
    let first = desk.stake(0).expect("the first stake account");
    assert_eq!(first.lamports, 10 * SOL);
    assert_eq!(
        (first.staker, first.withdrawer),
        (desk.owner(), desk.owner())
    );
    assert!(
        note.0.contains(&wallet_core::Part::Address(first.pubkey)),
        "{}",
        note.plain()
    );

    // The use case reads which addresses are taken from the chain itself.
    desk.run(StakeRequest::New {
        lamports: 3 * SOL,
        vote: None,
    })
    .unwrap();
    assert_eq!(desk.stake(1).expect("the second").lamports, 3 * SOL);

    desk.run(StakeRequest::Split {
        account: first,
        lamports: 4 * SOL,
    })
    .unwrap();
    assert_eq!(desk.stake(0).unwrap().lamports, 6 * SOL);
    assert_eq!(
        desk.stake(2).expect("the split").lamports,
        4 * SOL + desk.rent
    );
    assert!(desk.stake(3).is_none());
}

#[test]
fn what_the_program_refuses_comes_back_as_the_node_error() {
    let desk = Desk::new();
    desk.run(StakeRequest::New {
        lamports: 2 * SOL,
        vote: None,
    })
    .unwrap();
    let stake = desk.stake(0).unwrap();

    // More than the account holds.
    let err = desk
        .run(StakeRequest::Withdraw {
            stake: stake.pubkey,
            lamports: 5 * SOL,
            to: desk.owner(),
        })
        .unwrap_err();
    assert!(matches!(err, FlowError::Rpc(_)), "{err}");
    assert_eq!(desk.stake(0).unwrap().lamports, 2 * SOL);
}

/// A program's IDL comes from the accounts the program's own tooling writes:
/// read through the node port, from accounts that exist on the chain.
#[test]
fn an_idl_is_read_from_the_accounts_a_program_publishes_it_in() {
    let desk = Desk::new();
    let program = Pubkey::new_unique();
    let metadata_program: Pubkey = "ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S"
        .parse()
        .unwrap();
    let old = br#"{"name":"old","instructions":[]}"#.to_vec();
    let new = br#"{"name":"new","instructions":[]}"#.to_vec();
    let read = || pollster::block_on(published(&desk.node, &program)).unwrap();

    assert_eq!(read(), None);

    desk.node.chain().plant(
        anchor_idl_address(&program).unwrap(),
        program,
        anchor_idl_account_data(&old),
    );
    assert_eq!(read(), Some((old.clone(), IdlOrigin::Anchor)));

    // Once the program publishes through program metadata, that is what is read.
    desk.node.chain().plant(
        program_metadata_idl_address(&program).unwrap(),
        metadata_program,
        program_metadata_account_data(&new),
    );
    assert_eq!(read(), Some((new, IdlOrigin::ProgramMetadata)));

    // A file imported by hand is still compared with the Anchor account.
    let compared = pollster::block_on(counterpart(&desk.node, &program, &IdlOrigin::File)).unwrap();
    assert_eq!(compared, old);
}
