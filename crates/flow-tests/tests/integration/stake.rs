//! What the Stake screen builds, against the real stake program.
//!
//! A wallet creates a stake account at an address derived from its own, so
//! its signature is the only one. It then splits the account, merges the
//! halves back, delegates to a validator, deactivates and withdraws. After
//! each step the account is read back the way the screen reads it.

use crate::support::{logs, Chain, SOL};
use solana_pubkey::Pubkey;
use solana_vote_interface::instruction::{create_account_with_config, CreateVoteAccountConfig};
use solana_vote_interface::state::VoteInit;
use wallet_core::stake::{
    authorize_stake, create_stake, deactivate_stake, delegate_stake, merge_stake, split_stake,
    stake_address, withdraw_stake, StakeAccount, StakeHistory, StakePhase, StakeRole,
    STAKE_ACCOUNT_LEN,
};
use wallet_core::{KeypairSigner, WalletSigner};

struct Desk {
    chain: Chain,
    wallet: KeypairSigner,
    rent: u64,
}

impl Desk {
    fn new() -> Self {
        let mut chain = Chain::new();
        let wallet = KeypairSigner::new_random();
        chain.fund(&wallet.pubkey(), 100 * SOL);
        let rent = chain.rent(STAKE_ACCOUNT_LEN as usize);
        Self {
            chain,
            wallet,
            rent,
        }
    }

    fn owner(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    /// Send as the wallet, which also pays the fee.
    fn send(&mut self, instructions: Vec<wallet_core::Instruction>) {
        self.chain
            .send(&[&self.wallet], instructions)
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    }

    /// The account as a snapshot lists it: with the cluster's current rent.
    /// The reserve an account records is an older figure the program no
    /// longer goes by.
    fn read(&self, address: &Pubkey) -> Option<StakeAccount> {
        let account = self.chain.account(address)?;
        let mut stake = StakeAccount::parse(*address, &account)?;
        stake.rent_reserve = self.rent;
        Some(stake)
    }

    /// A validator's vote account, created by the vote program itself.
    fn vote_account(&mut self) -> Pubkey {
        let node = KeypairSigner::new_random();
        let vote = KeypairSigner::new_random();
        let config = CreateVoteAccountConfig::default();
        let lamports = self.chain.rent(config.space as usize);
        let init = VoteInit {
            node_pubkey: node.pubkey(),
            authorized_voter: node.pubkey(),
            authorized_withdrawer: node.pubkey(),
            commission: 5,
        };
        let create =
            create_account_with_config(&self.owner(), &vote.pubkey(), &init, lamports, config);
        self.chain
            .send(&[&self.wallet, &vote, &node], create)
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
        vote.pubkey()
    }
}

#[test]
fn a_wallet_creates_splits_merges_and_empties_a_stake_account() {
    let mut desk = Desk::new();
    let owner = desk.owner();
    let history = StakeHistory::default();

    let created = create_stake(&owner, 0, 10 * SOL, desk.rent, None).unwrap();
    desk.send(created.instructions);
    let stake = desk.read(&created.address).expect("a stake account");
    assert_eq!(created.address, stake_address(&owner, 0).unwrap());
    assert_eq!((stake.staker, stake.withdrawer), (owner, owner));
    assert_eq!(stake.lamports, 10 * SOL);
    assert_eq!(stake.status(0, &history).phase, StakePhase::Undelegated);

    // The split's own rent reserve comes from the wallet, on top of the amount.
    let split = split_stake(&stake, 4 * SOL, 1).unwrap();
    desk.send(split.instructions);
    assert_eq!(desk.read(&created.address).unwrap().lamports, 6 * SOL);
    let half = desk.read(&split.address).expect("the split account");
    assert_eq!(half.lamports, 4 * SOL + desk.rent);
    assert_eq!((half.staker, half.withdrawer), (owner, owner));

    let stake = desk.read(&created.address).unwrap();
    let status = stake.status(0, &history);
    assert!(stake.can_absorb(&status, &half, &half.status(0, &history), 0));
    desk.send(merge_stake(&created.address, &split.address, &owner));
    assert!(desk.read(&split.address).is_none());
    let stake = desk.read(&created.address).unwrap();
    assert_eq!(stake.lamports, 10 * SOL + desk.rent);

    // A partial withdrawal leaves the account open, a full one closes it.
    let status = stake.status(0, &history);
    let part = stake.withdraw_amount(0, &status, Some(SOL)).unwrap();
    desk.send(withdraw_stake(&created.address, &owner, &owner, part));
    let stake = desk.read(&created.address).unwrap();
    let before = desk.chain.balance(&owner);
    let rest = stake.withdraw_amount(0, &status, None).unwrap();
    assert_eq!(rest, stake.lamports);
    desk.send(withdraw_stake(&created.address, &owner, &owner, rest));
    assert!(desk.read(&created.address).is_none());
    assert!(desk.chain.balance(&owner) > before + rest - SOL / 100);
}

#[test]
fn a_stake_is_delegated_deactivated_and_withdrawn() {
    let mut desk = Desk::new();
    let owner = desk.owner();
    let vote = desk.vote_account();
    let history = StakeHistory::default();

    let created = create_stake(&owner, 0, 5 * SOL, desk.rent, Some(&vote)).unwrap();
    desk.send(created.instructions);
    let stake = desk.read(&created.address).unwrap();
    let delegation = stake.delegation.expect("a delegation");
    assert_eq!(delegation.voter, vote);
    assert_eq!(delegation.stake, 5 * SOL - desk.rent);
    let epoch = delegation.activation_epoch;
    let status = stake.status(epoch, &history);
    assert_eq!(status.phase, StakePhase::Activating);
    // Nothing leaves a delegated account until it is deactivated.
    assert_eq!(stake.withdrawable(epoch, &status), 0);

    // A second account that began activating in the same epoch folds in.
    let second = create_stake(&owner, 1, 2 * SOL, desk.rent, Some(&vote)).unwrap();
    desk.send(second.instructions);
    let other = desk.read(&second.address).unwrap();
    assert!(stake.can_absorb(&status, &other, &other.status(epoch, &history), epoch));
    desk.send(merge_stake(&created.address, &second.address, &owner));
    assert!(desk.read(&second.address).is_none());

    desk.send(deactivate_stake(&created.address, &owner));
    let stake = desk.read(&created.address).unwrap();
    let status = stake.status(epoch, &history);
    // Deactivated in the epoch it was delegated in, it never earned.
    assert_eq!(status.phase, StakePhase::Inactive);
    assert_eq!(stake.withdrawable(epoch, &status), stake.lamports);

    // It can be delegated again, and the stake authority can be handed over.
    desk.send(delegate_stake(&created.address, &owner, &vote));
    let friend = KeypairSigner::new_random().pubkey();
    desk.send(authorize_stake(
        &created.address,
        &owner,
        &friend,
        StakeRole::Staker,
    ));
    let stake = desk.read(&created.address).unwrap();
    assert!(stake.has_role(&friend, StakeRole::Staker));
    assert!(stake.has_role(&owner, StakeRole::Withdrawer));
    // The owner no longer deactivates it.
    let refused = desk
        .chain
        .send(&[&desk.wallet], deactivate_stake(&created.address, &owner))
        .unwrap_err();
    assert!(
        logs(&refused).contains("MissingRequiredSignature"),
        "{}",
        logs(&refused)
    );
}
