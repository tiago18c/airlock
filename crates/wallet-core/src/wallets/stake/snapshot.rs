//! Every stake account read in one go, with the epoch they were read in.

use super::*;

/// Read the stake history out of its sysvar account.
pub fn read_stake_history(account: &RawAccount) -> Option<StakeHistory> {
    wincode::deserialize(&account.data).ok()
}

/// A stake account with its state in the epoch it was read in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedStake {
    pub account: StakeAccount,
    pub status: StakeStatus,
}

impl ListedStake {
    pub fn withdrawable(&self, epoch: u64) -> u64 {
        self.account.withdrawable(epoch, &self.status)
    }
}

/// Every stake account found for a set of addresses, in one epoch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StakeSnapshot {
    pub epoch: EpochInfo,
    /// Lamports a stake account has to keep to exist.
    pub rent_reserve: u64,
    /// The least a delegation may be, on top of the rent reserve. Zero when
    /// the node was not asked.
    pub min_delegation: u64,
    /// Largest first.
    pub stakes: Vec<ListedStake>,
    /// Addresses whose stake accounts could not be listed.
    pub failed: Vec<Pubkey>,
}

impl StakeSnapshot {
    /// Put together what was read. An account found under both of its
    /// authorities is listed once. `rent_reserve` is the answer of
    /// `getMinimumBalanceForRentExemption` for [`STAKE_ACCOUNT_LEN`] bytes.
    pub fn new(
        epoch: EpochInfo,
        history: &StakeHistory,
        rent_reserve: Option<u64>,
        found: Vec<StakeAccount>,
    ) -> Self {
        let mut stakes: Vec<ListedStake> = Vec::new();
        for mut account in found {
            if stakes
                .iter()
                .any(|known| known.account.pubkey == account.pubkey)
            {
                continue;
            }
            if let Some(rent_reserve) = rent_reserve {
                account.rent_reserve = rent_reserve;
            }
            let status = account.status(epoch.epoch, history);
            stakes.push(ListedStake { account, status });
        }
        stakes.sort_by(|a, b| {
            b.account
                .lamports
                .cmp(&a.account.lamports)
                .then(a.account.pubkey.cmp(&b.account.pubkey))
        });
        Self {
            epoch,
            rent_reserve: rent_reserve.unwrap_or(STAKE_RENT_LAMPORTS),
            min_delegation: 0,
            stakes,
            failed: Vec::new(),
        }
    }

    /// Whether an account holding `lamports` has enough to be delegated.
    pub fn can_delegate(&self, lamports: u64) -> bool {
        lamports.saturating_sub(self.rent_reserve) >= self.min_delegation.max(1)
    }

    /// The accounts `address` is an authority of.
    pub fn of(&self, address: &Pubkey) -> Vec<ListedStake> {
        self.stakes
            .iter()
            .filter(|stake| {
                stake.account.staker == *address || stake.account.withdrawer == *address
            })
            .cloned()
            .collect()
    }

    pub fn addresses(&self) -> Vec<Pubkey> {
        self.stakes
            .iter()
            .map(|stake| stake.account.pubkey)
            .collect()
    }

    /// Whether this lists other stake accounts than `before` did. What was
    /// loaded for the old list, such as rewards, does not describe this one.
    pub fn lists_other_accounts_than(&self, before: Option<&Self>) -> bool {
        before.map(Self::addresses) != Some(self.addresses())
    }
}
