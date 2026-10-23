//! A stake account: who controls it, what it is delegated to, and what state
//! the delegation is in.

use super::*;

/// One of the two authorities of a stake account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StakeRole {
    /// Delegates, deactivates, splits and merges.
    Staker,
    /// Withdraws, and can replace both authorities.
    Withdrawer,
}

impl StakeRole {
    /// Where this authority's address is in a stake account's data.
    pub fn offset(self) -> usize {
        match self {
            Self::Staker => STAKER_OFFSET,
            Self::Withdrawer => WITHDRAWER_OFFSET,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StakeLockup {
    pub unix_timestamp: i64,
    pub epoch: u64,
    pub custodian: Pubkey,
}

impl StakeLockup {
    /// Withdrawals need the custodian's signature until both have passed.
    pub fn in_force(&self, epoch: u64, unix_now: i64) -> bool {
        self.unix_timestamp > unix_now || self.epoch > epoch
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StakeDelegation {
    /// The validator's vote account.
    pub voter: Pubkey,
    pub stake: u64,
    pub activation_epoch: u64,
    /// `u64::MAX` while the stake is not being deactivated.
    pub deactivation_epoch: u64,
}

/// An initialized stake account, as read from the chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StakeAccount {
    pub pubkey: Pubkey,
    pub lamports: u64,
    /// What the account has to keep to exist. Read from the account, then
    /// replaced by the cluster's current rent when a snapshot is put together.
    pub rent_reserve: u64,
    pub staker: Pubkey,
    pub withdrawer: Pubkey,
    pub lockup: StakeLockup,
    /// `None` for an account that was never delegated.
    pub delegation: Option<StakeDelegation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StakePhase {
    /// Never delegated.
    Undelegated,
    Activating,
    Active,
    Deactivating,
    /// Deactivated and cooled down: everything can be withdrawn.
    Inactive,
}

/// How much of a delegation counts in one epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StakeStatus {
    pub phase: StakePhase,
    /// Lamports earning rewards.
    pub effective: u64,
    pub activating: u64,
    pub deactivating: u64,
}

impl StakeAccount {
    /// Read `account` as a stake account. `None` for anything that is not an
    /// initialized one.
    pub fn parse(pubkey: Pubkey, account: &RawAccount) -> Option<Self> {
        if account.owner != STAKE_PROGRAM_ID {
            return None;
        }
        let (meta, stake) = match wincode::deserialize::<StakeStateV2>(&account.data).ok()? {
            StakeStateV2::Initialized(meta) => (meta, None),
            StakeStateV2::Stake(meta, stake, _) => (meta, Some(stake)),
            StakeStateV2::Uninitialized | StakeStateV2::RewardsPool => return None,
        };
        #[allow(deprecated)]
        let rent_reserve = meta.rent_exempt_reserve;
        Some(Self {
            pubkey,
            lamports: account.lamports,
            rent_reserve,
            staker: meta.authorized.staker,
            withdrawer: meta.authorized.withdrawer,
            lockup: StakeLockup {
                unix_timestamp: meta.lockup.unix_timestamp,
                epoch: meta.lockup.epoch,
                custodian: meta.lockup.custodian,
            },
            delegation: stake.map(|stake| StakeDelegation {
                voter: stake.delegation.voter_pubkey,
                stake: stake.delegation.stake,
                activation_epoch: stake.delegation.activation_epoch,
                deactivation_epoch: stake.delegation.deactivation_epoch,
            }),
        })
    }

    pub fn has_role(&self, address: &Pubkey, role: StakeRole) -> bool {
        match role {
            StakeRole::Staker => self.staker == *address,
            StakeRole::Withdrawer => self.withdrawer == *address,
        }
    }

    /// What the delegation is worth in `epoch`, by the rules the stake
    /// program applies: stake warms up and cools down at the cluster's rate.
    pub fn status(&self, epoch: u64, history: &StakeHistory) -> StakeStatus {
        let Some(delegation) = &self.delegation else {
            return StakeStatus {
                phase: StakePhase::Undelegated,
                effective: 0,
                activating: 0,
                deactivating: 0,
            };
        };
        let mut on_chain = Delegation::new(
            &delegation.voter,
            delegation.stake,
            delegation.activation_epoch,
        );
        on_chain.deactivation_epoch = delegation.deactivation_epoch;
        let entry = on_chain.stake_activating_and_deactivating_v2(
            epoch,
            history,
            NEW_RATE_ACTIVATION_EPOCH,
        );
        let phase = if delegation.deactivation_epoch == u64::MAX {
            if entry.activating > 0 {
                StakePhase::Activating
            } else {
                StakePhase::Active
            }
        } else if entry.effective == 0 && entry.activating == 0 {
            StakePhase::Inactive
        } else {
            StakePhase::Deactivating
        };
        StakeStatus {
            phase,
            effective: entry.effective,
            activating: entry.activating,
            deactivating: entry.deactivating,
        }
    }

    /// Lamports the stake program still counts as staked in `epoch`. Until a
    /// deactivation is asked for, that is the whole delegation.
    pub(super) fn staked(&self, epoch: u64, status: &StakeStatus) -> u64 {
        match &self.delegation {
            None => 0,
            Some(delegation) if epoch >= delegation.deactivation_epoch => status.effective,
            Some(delegation) => delegation.stake,
        }
    }

    /// Lamports a withdrawal can take now. Everything once nothing is
    /// staked, otherwise what is above the stake and the rent reserve.
    pub fn withdrawable(&self, epoch: u64, status: &StakeStatus) -> u64 {
        let staked = self.staked(epoch, status);
        if staked == 0 {
            self.lamports
        } else {
            self.lamports
                .saturating_sub(staked.saturating_add(self.rent_reserve))
        }
    }

    /// Check `requested` against what can be withdrawn. `None` asks for
    /// everything available.
    pub fn withdraw_amount(
        &self,
        epoch: u64,
        status: &StakeStatus,
        requested: Option<u64>,
    ) -> Result<u64, StakeError> {
        let available = self.withdrawable(epoch, status);
        if available == 0 {
            return Err(StakeError::NothingToWithdraw);
        }
        let Some(lamports) = requested else {
            return Ok(available);
        };
        if lamports == 0 {
            return Err(StakeError::Amount);
        }
        if lamports > available {
            return Err(StakeError::TooMuch { available });
        }
        // An account that is not closed has to keep its rent reserve.
        let left = self.lamports - lamports;
        if left != 0 && left < self.rent_reserve {
            return Err(StakeError::BelowReserve {
                reserve: self.rent_reserve,
            });
        }
        Ok(lamports)
    }

    /// Whether the stake program would merge `source` into this account.
    /// Both need the same authorities and lockup. Then either the source
    /// holds no stake, or both are fully active, or both began activating
    /// this epoch, with the same validator.
    pub fn can_absorb(
        &self,
        status: &StakeStatus,
        source: &StakeAccount,
        source_status: &StakeStatus,
        epoch: u64,
    ) -> bool {
        if self.pubkey == source.pubkey
            || self.staker != source.staker
            || self.withdrawer != source.withdrawer
            || self.lockup != source.lockup
        {
            return false;
        }
        let same_voter = match (&self.delegation, &source.delegation) {
            (Some(a), Some(b)) => a.voter == b.voter,
            _ => false,
        };
        match (
            self.merge_kind(status, epoch),
            source.merge_kind(source_status, epoch),
        ) {
            (MergeKind::Inactive, MergeKind::Inactive)
            | (MergeKind::Inactive, MergeKind::ActivationEpoch)
            | (MergeKind::ActivationEpoch, MergeKind::Inactive) => true,
            (MergeKind::ActivationEpoch, MergeKind::ActivationEpoch)
            | (MergeKind::FullyActive, MergeKind::FullyActive) => same_voter,
            _ => false,
        }
    }

    pub(super) fn merge_kind(&self, status: &StakeStatus, epoch: u64) -> MergeKind {
        match (status.phase, &self.delegation) {
            (StakePhase::Undelegated | StakePhase::Inactive, _) => MergeKind::Inactive,
            (StakePhase::Active, _) => MergeKind::FullyActive,
            (StakePhase::Activating, Some(delegation))
                if delegation.activation_epoch == epoch && status.effective == 0 =>
            {
                MergeKind::ActivationEpoch
            }
            _ => MergeKind::Transient,
        }
    }
}

/// The states the stake program tells apart when it merges.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MergeKind {
    Inactive,
    ActivationEpoch,
    FullyActive,
    /// Warming up over several epochs, or cooling down: not mergeable.
    Transient,
}
