//! Moving what a wallet holds to another address, beyond its token accounts
//! (those are [`token_plan`](crate::token_plan)'s): handing its stake
//! accounts' authorities over, closing the nonce accounts it controls, and
//! sweeping its SOL last.
//!
//! Nothing here reads the chain. `wallet_flows::migrate` reads it, and runs
//! what is planned here one step at a time.

use crate::chain::RawAccount;
use crate::tx::sol_transfer;
use crate::wallets::stake::{authorize_stake, StakeAccount, StakeRole};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use solana_system_interface::instruction::withdraw_nonce_account;

/// Which of a stake account's authorities to hand over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StakeChoice {
    pub staker: bool,
    pub withdrawer: bool,
}

impl StakeChoice {
    pub const BOTH: StakeChoice = StakeChoice {
        staker: true,
        withdrawer: true,
    };
}

/// What happens to a stake account's authorities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StakeOutcome {
    /// The authorities named go to the destination. The account stays as
    /// it is otherwise, delegated or not.
    Handed {
        staker: bool,
        withdrawer: bool,
    },
    Left(StakeBlocked),
}

/// Why a stake account's authorities stay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StakeBlocked {
    Chosen,
    /// The wallet is not the authority asked for: only the staker or the
    /// withdrawer may change the staker, only the withdrawer the withdrawer.
    NotAuthority,
    /// Its lockup is in force, so a new withdrawer needs `custodian`.
    Lockup {
        custodian: Pubkey,
    },
}

impl StakeBlocked {
    pub fn reason(&self) -> String {
        match self {
            StakeBlocked::Chosen => "Left as it is.".into(),
            StakeBlocked::NotAuthority => {
                "This wallet is not the authority that may hand it over.".into()
            }
            StakeBlocked::Lockup { custodian } => format!(
                "Its lockup is in force, so a new withdrawer needs its custodian {custodian} to sign."
            ),
        }
    }
}

/// What the chain says now, for a lockup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    pub epoch: u64,
    pub unix_time: i64,
}

/// Hand `stake`'s authorities named in `choice` from `owner` to `to`. The
/// staker goes first, while `owner` still signs as whichever authority it
/// is; then the withdrawer. A withdrawer that cannot be handed over leaves
/// the staker handed over alone when that was asked for too.
pub fn hand_over_stake(
    owner: Pubkey,
    to: Pubkey,
    stake: &StakeAccount,
    choice: StakeChoice,
    now: Now,
) -> (StakeOutcome, Vec<Instruction>) {
    if !choice.staker && !choice.withdrawer {
        return (StakeOutcome::Left(StakeBlocked::Chosen), Vec::new());
    }
    let staker = choice.staker && (stake.staker == owner || stake.withdrawer == owner);
    let mut why = None;
    let withdrawer = if !choice.withdrawer {
        false
    } else if stake.withdrawer != owner {
        why = Some(StakeBlocked::NotAuthority);
        false
    } else if stake.lockup.in_force(now.epoch, now.unix_time) && stake.lockup.custodian != owner {
        why = Some(StakeBlocked::Lockup {
            custodian: stake.lockup.custodian,
        });
        false
    } else {
        true
    };
    if choice.staker && !staker {
        why.get_or_insert(StakeBlocked::NotAuthority);
    }
    if !staker && !withdrawer {
        return (
            StakeOutcome::Left(why.unwrap_or(StakeBlocked::NotAuthority)),
            Vec::new(),
        );
    }
    let mut instructions = Vec::new();
    // Nothing to sign for when it is already `to`'s.
    if staker && stake.staker != to {
        instructions.extend(authorize_stake(
            &stake.pubkey,
            &owner,
            &to,
            StakeRole::Staker,
        ));
    }
    if withdrawer && stake.withdrawer != to {
        instructions.extend(authorize_stake(
            &stake.pubkey,
            &owner,
            &to,
            StakeRole::Withdrawer,
        ));
    }
    (StakeOutcome::Handed { staker, withdrawer }, instructions)
}

/// A durable nonce account the wallet is the authority of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonceHeld {
    pub address: Pubkey,
    pub lamports: u64,
}

/// What happens to a nonce account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NonceOutcome {
    /// Closed: all its lamports go to the wallet, and leave with the sweep.
    Closed {
        lamports: u64,
    },
    Left(NonceBlocked),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NonceBlocked {
    Chosen,
    /// The wallet's own transactions live on it, the sweep among them.
    InUse,
}

impl NonceBlocked {
    pub fn reason(&self) -> String {
        match self {
            NonceBlocked::Chosen => "Left as it is.".into(),
            NonceBlocked::InUse => {
                "This wallet's transactions live on it, so it stays until the move is done.".into()
            }
        }
    }
}

/// Close `nonce` into `owner`, its authority, unless it is `in_use`.
pub fn close_nonce(
    owner: Pubkey,
    nonce: &NonceHeld,
    close: bool,
    in_use: Option<Pubkey>,
) -> (NonceOutcome, Vec<Instruction>) {
    if in_use == Some(nonce.address) {
        return (NonceOutcome::Left(NonceBlocked::InUse), Vec::new());
    }
    if !close {
        return (NonceOutcome::Left(NonceBlocked::Chosen), Vec::new());
    }
    (
        NonceOutcome::Closed {
            lamports: nonce.lamports,
        },
        vec![withdraw_nonce_account(
            &nonce.address,
            &owner,
            &owner,
            nonce.lamports,
        )],
    )
}

/// What the wallet's own account must keep: nothing for an account of the
/// System program with no data, which a sweep empties and so deletes; its
/// rent for any other (a Vector account), which must stay rent exempt.
pub fn must_keep(account: &RawAccount, rent_exempt: u64) -> u64 {
    if account.owner == crate::SYSTEM_PROGRAM_ID && account.data.is_empty() {
        0
    } else {
        rent_exempt
    }
}

/// The lamports a sweep sends: what the account holds, plus what closing
/// nonce accounts in the same transaction brings in, less the fee when the
/// wallet pays it, less what it must keep. 0 when nothing is left to send.
pub fn sweep_amount(balance: u64, incoming: u64, fee: u64, keep: u64) -> u64 {
    balance
        .saturating_add(incoming)
        .saturating_sub(fee.saturating_add(keep))
}

/// The sweep itself, from `owner` to `to`.
pub fn sweep(owner: Pubkey, to: Pubkey, lamports: u64) -> Instruction {
    sol_transfer(&owner, &to, lamports)
}

/// Split `ops`, each a stake hand-over or a nonce close kept whole, into
/// steps of at most `per_step` instructions. The last step is where the
/// sweep goes.
pub fn chunk_ops(ops: Vec<Vec<Instruction>>, per_step: usize) -> Vec<Vec<Instruction>> {
    let mut steps: Vec<Vec<Instruction>> = Vec::new();
    for op in ops.into_iter().filter(|op| !op.is_empty()) {
        match steps.last_mut() {
            Some(step) if step.len() + op.len() <= per_step.max(1) => step.extend(op),
            _ => steps.push(op),
        }
    }
    steps
}

#[cfg(test)]
mod tests;
