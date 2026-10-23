//! Checking what was typed for a stake operation before anything is built.

use super::prepare::StakeRequest;
use solana_pubkey::Pubkey;
use wallet_core::format_sol;
use wallet_core::stake::{
    sol_to_lamports, ListedStake, StakeRole, StakeSnapshot, STAKE_RENT_LAMPORTS,
};

/// Which stake operation a form is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StakeOp {
    New,
    Delegate,
    Deactivate,
    Withdraw,
    Split,
    Merge,
    Authorize,
}

/// What a stake form holds when it is submitted.
pub struct StakeInput<'a> {
    pub op: StakeOp,
    /// The amount as typed, in SOL. Blank on a withdrawal means all that is free.
    pub amount: &'a str,
    /// The validator, the destination or the new authority, by `op`.
    pub address: Option<Pubkey>,
    /// The stake account the operation is on. `None` only for a new stake.
    pub target: Option<&'a ListedStake>,
    /// What was read from the chain, when it was. Without it the cluster's
    /// minimum delegation is not checked.
    pub snapshot: Option<&'a StakeSnapshot>,
    /// The account to merge into the target.
    pub merge_source: Option<Pubkey>,
    /// Which authority a change of authority replaces.
    pub role: StakeRole,
}

/// Turn a submitted form into a request, or say what is wrong with it.
pub fn checked_request(input: StakeInput<'_>) -> Result<StakeRequest, String> {
    let StakeInput {
        op,
        amount,
        address,
        target,
        snapshot,
        merge_source,
        role,
    } = input;
    let amount = amount.trim();
    let on_target = || target.ok_or_else(|| "No stake account was chosen.".to_string());
    let epoch = snapshot.map_or(0, |found| found.epoch.epoch);
    // Whether an account holding this much could be delegated.
    let delegable = |lamports: u64| snapshot.is_none_or(|found| found.can_delegate(lamports));
    let too_small = || {
        format!(
            "A delegation needs at least {} on top of the rent reserve of {}.",
            format_sol(snapshot.map_or(0, |found| found.min_delegation).max(1)),
            format_sol(snapshot.map_or(STAKE_RENT_LAMPORTS, |found| found.rent_reserve))
        )
    };
    Ok(match op {
        StakeOp::New => {
            let lamports = sol_to_lamports(amount).map_err(|err| err.to_string())?;
            if address.is_some() && !delegable(lamports) {
                return Err(too_small());
            }
            StakeRequest::New {
                lamports,
                vote: address,
            }
        }
        StakeOp::Delegate => {
            let target = on_target()?;
            if !delegable(target.account.lamports) {
                return Err(too_small());
            }
            StakeRequest::Delegate {
                stake: target.account.pubkey,
                vote: address.ok_or("Enter the validator's vote account.")?,
            }
        }
        StakeOp::Deactivate => StakeRequest::Deactivate {
            stake: on_target()?.account.pubkey,
        },
        StakeOp::Withdraw => {
            let target = on_target()?;
            let requested = if amount.is_empty() {
                None
            } else {
                Some(sol_to_lamports(amount).map_err(|err| err.to_string())?)
            };
            let lamports = target
                .account
                .withdraw_amount(epoch, &target.status, requested)
                .map_err(|err| err.to_string())?;
            StakeRequest::Withdraw {
                stake: target.account.pubkey,
                lamports,
                to: address.ok_or("Enter the address that receives the SOL.")?,
            }
        }
        StakeOp::Split => {
            let target = on_target()?;
            let lamports = sol_to_lamports(amount).map_err(|err| err.to_string())?;
            if lamports == 0 || lamports >= target.account.lamports {
                return Err("Enter less than the account holds. To move all of it, change its authorities instead.".into());
            }
            StakeRequest::Split {
                account: target.account.clone(),
                lamports,
            }
        }
        StakeOp::Merge => StakeRequest::Merge {
            stake: on_target()?.account.pubkey,
            source: merge_source.ok_or("No other stake account can be merged into this one.")?,
        },
        StakeOp::Authorize => StakeRequest::Authorize {
            stake: on_target()?.account.pubkey,
            new_authority: address.ok_or("Enter the new authority's address.")?,
            role,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, stake_account};
    use wallet_core::stake::{EpochInfo, StakeAccount, StakeHistory};

    fn snapshot() -> StakeSnapshot {
        let account = StakeAccount::parse(pk(30), &stake_account(pk(1), pk(1), 7)).unwrap();
        let mut snapshot = StakeSnapshot::new(
            EpochInfo {
                epoch: 20,
                slot_index: 0,
                slots_in_epoch: 432_000,
            },
            &StakeHistory::default(),
            Some(STAKE_RENT_LAMPORTS),
            vec![account],
        );
        snapshot.min_delegation = 1_000_000_000;
        snapshot
    }

    fn input<'a>(op: StakeOp, amount: &'a str, snapshot: &'a StakeSnapshot) -> StakeInput<'a> {
        StakeInput {
            op,
            amount,
            address: None,
            target: snapshot.stakes.first(),
            snapshot: Some(snapshot),
            merge_source: None,
            role: StakeRole::Staker,
        }
    }

    #[test]
    fn a_new_stake_below_the_minimum_is_refused_only_when_it_is_delegated() {
        let snapshot = snapshot();
        let small = StakeInput {
            target: None,
            ..input(StakeOp::New, "0.5", &snapshot)
        };
        assert!(matches!(
            checked_request(small),
            Ok(StakeRequest::New { vote: None, .. })
        ));
        let delegated = StakeInput {
            target: None,
            address: Some(pk(9)),
            ..input(StakeOp::New, "0.5", &snapshot)
        };
        assert!(checked_request(delegated)
            .unwrap_err()
            .contains("A delegation needs at least 1 SOL"));
    }

    #[test]
    fn a_split_has_to_leave_something_behind() {
        let snapshot = snapshot();
        assert!(checked_request(input(StakeOp::Split, "7", &snapshot)).is_err());
        assert!(checked_request(input(StakeOp::Split, "0", &snapshot)).is_err());
        assert!(matches!(
            checked_request(input(StakeOp::Split, "2", &snapshot)),
            Ok(StakeRequest::Split {
                lamports: 2_000_000_000,
                ..
            })
        ));
    }

    #[test]
    fn what_is_missing_is_named() {
        let snapshot = snapshot();
        let no_target = StakeInput {
            target: None,
            ..input(StakeOp::Deactivate, "", &snapshot)
        };
        assert_eq!(
            checked_request(no_target).unwrap_err(),
            "No stake account was chosen."
        );
        assert!(checked_request(input(StakeOp::Delegate, "", &snapshot))
            .unwrap_err()
            .contains("vote account"));
        assert!(checked_request(input(StakeOp::Merge, "", &snapshot))
            .unwrap_err()
            .contains("merged"));
        assert!(checked_request(input(StakeOp::Authorize, "", &snapshot))
            .unwrap_err()
            .contains("new authority"));
    }
}
