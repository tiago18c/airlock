//! Reading stake accounts.

use solana_pubkey::Pubkey;
use wallet_core::stake::{
    read_stake_history, stake_address, StakeAccount, StakeRole, StakeSnapshot, STAKE_ACCOUNT_LEN,
    STAKE_HISTORY_ID,
};
use wallet_core::{RpcError, SolanaRpc, STAKE_PROGRAM_ID};

/// How many seed addresses are checked in one request, and in all.
const SEED_BATCH: u32 = 8;
const SEED_LIMIT: u32 = 256;

/// Every stake account that names one of `owners` as an authority, with the
/// epoch and the stake history that say what state each is in.
///
/// An owner whose accounts could not be listed is named in the snapshot. The
/// call fails only when none could, or the epoch could not be read.
pub async fn fetch_stakes(
    rpc: &dyn SolanaRpc,
    owners: &[Pubkey],
) -> Result<StakeSnapshot, RpcError> {
    let epoch = rpc.get_epoch_info().await?;
    let history = rpc
        .get_account(&STAKE_HISTORY_ID)
        .await?
        .as_ref()
        .and_then(read_stake_history)
        .ok_or_else(|| RpcError::Message("The stake history could not be read.".into()))?;
    let rent = rpc
        .get_rent_exemption(STAKE_ACCOUNT_LEN as usize)
        .await
        .ok();

    let mut found = Vec::new();
    let mut failed = Vec::new();
    let mut first_error = None;
    for owner in owners {
        for role in [StakeRole::Staker, StakeRole::Withdrawer] {
            let listed = rpc
                .get_program_accounts(&STAKE_PROGRAM_ID, STAKE_ACCOUNT_LEN, role.offset(), owner)
                .await;
            match listed {
                Ok(rows) => found.extend(
                    rows.iter()
                        .filter_map(|(pubkey, account)| StakeAccount::parse(*pubkey, account)),
                ),
                Err(err) => {
                    if !failed.contains(owner) {
                        failed.push(*owner);
                    }
                    first_error.get_or_insert(err);
                }
            }
        }
    }
    if let Some(err) = first_error {
        if failed.len() == owners.len() {
            return Err(err);
        }
    }
    let mut snapshot = StakeSnapshot::new(epoch, &history, rent, found);
    snapshot.failed = failed;
    snapshot.min_delegation = rpc.get_stake_minimum_delegation().await.unwrap_or(0);
    Ok(snapshot)
}

/// The lowest stake seed of `owner` whose address holds no account.
pub async fn free_stake_index(rpc: &dyn SolanaRpc, owner: &Pubkey) -> Result<u32, RpcError> {
    for start in (0..SEED_LIMIT).step_by(SEED_BATCH as usize) {
        let addresses = (start..start + SEED_BATCH)
            .map(|index| stake_address(owner, index))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| RpcError::Message(err.to_string()))?;
        let held = rpc.get_lamports(&addresses).await?;
        if held.len() != addresses.len() {
            return Err(RpcError::Message(
                "The node did not answer for every address asked.".into(),
            ));
        }
        if let Some(free) = held.iter().position(Option::is_none) {
            return Ok(start + free as u32);
        }
    }
    Err(RpcError::Message(format!(
        "The first {SEED_LIMIT} stake addresses of {owner} are all in use."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, stake_account, stake_history_account, FakeRpc};

    fn with_history() -> FakeRpc {
        FakeRpc::default().with_account(STAKE_HISTORY_ID, stake_history_account())
    }

    #[test]
    fn an_account_found_under_both_authorities_is_listed_once() {
        let owner = pk(1);
        let stake = stake_account(owner, owner, 7);
        let rpc = with_history()
            .with_program_accounts(
                owner,
                StakeRole::Staker.offset(),
                vec![(pk(30), stake.clone())],
            )
            .with_program_accounts(owner, StakeRole::Withdrawer.offset(), vec![(pk(30), stake)]);
        let snapshot = pollster::block_on(fetch_stakes(&rpc, &[owner])).unwrap();
        assert_eq!(snapshot.addresses(), vec![pk(30)]);
        assert!(snapshot.failed.is_empty());
        assert_eq!(snapshot.min_delegation, 1_000_000_000);
    }

    #[test]
    fn an_owner_that_could_not_be_listed_is_named_and_the_rest_is_kept() {
        let (good, bad) = (pk(1), pk(2));
        let stake = stake_account(good, good, 7);
        let rpc = with_history()
            .with_program_accounts(good, StakeRole::Staker.offset(), vec![(pk(30), stake)])
            .failing_program_accounts(bad);
        let snapshot = pollster::block_on(fetch_stakes(&rpc, &[good, bad])).unwrap();
        assert_eq!(snapshot.addresses(), vec![pk(30)]);
        assert_eq!(snapshot.failed, vec![bad]);

        // When no owner could be listed, the read fails.
        let err = pollster::block_on(fetch_stakes(&rpc, &[bad])).unwrap_err();
        assert_eq!(err, RpcError::Timeout);
    }

    #[test]
    fn the_stake_history_is_needed() {
        let err = pollster::block_on(fetch_stakes(&FakeRpc::default(), &[pk(1)])).unwrap_err();
        assert!(err.to_string().contains("stake history"));
    }

    #[test]
    fn the_first_seed_address_without_an_account_is_free() {
        let owner = pk(1);
        let rpc = (0..10).fold(FakeRpc::default(), |rpc, index| {
            rpc.with_balance(stake_address(&owner, index).unwrap(), 1)
        });
        assert_eq!(pollster::block_on(free_stake_index(&rpc, &owner)), Ok(10));
        // Two batches of eight were asked.
        assert_eq!(rpc.asked(), 2);
        assert_eq!(
            pollster::block_on(free_stake_index(&FakeRpc::default(), &owner)),
            Ok(0)
        );
    }
}
