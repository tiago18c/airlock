//! The instructions of each stake operation.

use super::*;

/// The seed of the `index`-th stake account an address creates.
pub fn stake_seed(index: u32) -> String {
    format!("stake:{index}")
}

/// The stake account `owner` creates under [`stake_seed`]. The address needs
/// no key of its own, so the owner's signature is the only one.
pub fn stake_address(owner: &Pubkey, index: u32) -> Result<Pubkey, StakeError> {
    Pubkey::create_with_seed(owner, &stake_seed(index), &STAKE_PROGRAM_ID)
        .map_err(|_| StakeError::Seed)
}

/// A stake account that does not exist yet, and what creates it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewStake {
    pub address: Pubkey,
    pub instructions: Vec<Instruction>,
}

/// Create the `index`-th stake account of `owner` holding `lamports`, with
/// `owner` as both authorities, and delegate it when `vote` is given.
/// `rent_reserve` is the part of `lamports` that is never staked.
pub fn create_stake(
    owner: &Pubkey,
    index: u32,
    lamports: u64,
    rent_reserve: u64,
    vote: Option<&Pubkey>,
) -> Result<NewStake, StakeError> {
    if lamports <= rent_reserve {
        return Err(StakeError::BelowRent {
            reserve: rent_reserve,
        });
    }
    let address = stake_address(owner, index)?;
    let mut instructions = stake_ix::create_account_with_seed(
        owner,
        &address,
        owner,
        &stake_seed(index),
        &Authorized::auto(owner),
        &Lockup::default(),
        lamports,
    );
    if let Some(vote) = vote {
        instructions.push(stake_ix::delegate_stake(&address, owner, vote));
    }
    Ok(NewStake {
        address,
        instructions,
    })
}

pub fn delegate_stake(stake: &Pubkey, staker: &Pubkey, vote: &Pubkey) -> Vec<Instruction> {
    vec![stake_ix::delegate_stake(stake, staker, vote)]
}

pub fn deactivate_stake(stake: &Pubkey, staker: &Pubkey) -> Vec<Instruction> {
    vec![stake_ix::deactivate_stake(stake, staker)]
}

pub fn withdraw_stake(
    stake: &Pubkey,
    withdrawer: &Pubkey,
    to: &Pubkey,
    lamports: u64,
) -> Vec<Instruction> {
    vec![stake_ix::withdraw(stake, withdrawer, to, lamports, None)]
}

/// Move `lamports` of `source` into the `index`-th stake account of its
/// staker. The new account starts with its own rent reserve, paid by the
/// staker, which the stake program requires when the source is active.
pub fn split_stake(
    source: &StakeAccount,
    lamports: u64,
    index: u32,
) -> Result<NewStake, StakeError> {
    if lamports == 0 {
        return Err(StakeError::Amount);
    }
    let staker = &source.staker;
    let address = stake_address(staker, index)?;
    let mut instructions = vec![solana_system_interface::instruction::transfer(
        staker,
        &address,
        source.rent_reserve,
    )];
    instructions.extend(stake_ix::split_with_seed(
        &source.pubkey,
        staker,
        lamports,
        &address,
        staker,
        &stake_seed(index),
    ));
    Ok(NewStake {
        address,
        instructions,
    })
}

/// Fold `source` into `destination`. The source account is closed.
pub fn merge_stake(destination: &Pubkey, source: &Pubkey, staker: &Pubkey) -> Vec<Instruction> {
    stake_ix::merge(destination, source, staker)
}

/// Hand one authority of `stake` to `new_authority`. `signer` is the current
/// holder of that role, or the withdrawer, who may replace either.
pub fn authorize_stake(
    stake: &Pubkey,
    signer: &Pubkey,
    new_authority: &Pubkey,
    role: StakeRole,
) -> Vec<Instruction> {
    let role = match role {
        StakeRole::Staker => StakeAuthorize::Staker,
        StakeRole::Withdrawer => StakeAuthorize::Withdrawer,
    };
    vec![stake_ix::authorize(
        stake,
        signer,
        new_authority,
        role,
        None,
    )]
}
