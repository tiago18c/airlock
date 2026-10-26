use super::*;
use crate::fixtures::pk;
use crate::units::LAMPORTS_PER_SOL;
use solana_stake_interface::stake_flags::StakeFlags;
use solana_stake_interface::stake_history::StakeHistoryEntry;
use solana_stake_interface::state::{Meta, Stake};

const SOL: u64 = LAMPORTS_PER_SOL;

#[allow(deprecated)]
fn meta(staker: Pubkey, withdrawer: Pubkey) -> Meta {
    Meta {
        rent_exempt_reserve: STAKE_RENT_LAMPORTS,
        authorized: Authorized { staker, withdrawer },
        lockup: Lockup::default(),
    }
}

fn raw(state: &StakeStateV2, lamports: u64) -> RawAccount {
    let mut data = wincode::serialize(state).unwrap();
    data.resize(STAKE_ACCOUNT_LEN as usize, 0);
    RawAccount {
        lamports,
        data,
        owner: STAKE_PROGRAM_ID,
        executable: false,
        rent_epoch: u64::MAX,
    }
}

fn delegated(stake: u64, activation: u64, deactivation: u64) -> StakeAccount {
    let mut delegation = Delegation::new(&pk(9), stake, activation);
    delegation.deactivation_epoch = deactivation;
    let state = StakeStateV2::Stake(
        meta(pk(1), pk(2)),
        Stake {
            delegation,
            credits_observed: 0,
        },
        StakeFlags::empty(),
    );
    StakeAccount::parse(pk(5), &raw(&state, stake + STAKE_RENT_LAMPORTS)).unwrap()
}

/// A cluster with so much stake that any delegation warms up in one epoch.
fn history(epochs: std::ops::Range<u64>) -> StakeHistory {
    let mut history = StakeHistory::default();
    for epoch in epochs {
        history.add(
            epoch,
            StakeHistoryEntry {
                effective: 1_000_000 * SOL,
                activating: 10 * SOL,
                deactivating: 10 * SOL,
            },
        );
    }
    history
}

#[test]
fn an_initialized_account_is_read_and_others_are_not() {
    let state = StakeStateV2::Initialized(meta(pk(1), pk(2)));
    let account = raw(&state, 5 * SOL);
    // The offsets the `memcmp` filters use.
    assert_eq!(&account.data[STAKER_OFFSET..][..32], pk(1).as_ref());
    assert_eq!(&account.data[WITHDRAWER_OFFSET..][..32], pk(2).as_ref());

    let read = StakeAccount::parse(pk(5), &account).unwrap();
    assert_eq!((read.staker, read.withdrawer), (pk(1), pk(2)));
    assert_eq!(read.rent_reserve, STAKE_RENT_LAMPORTS);
    assert_eq!(read.delegation, None);
    assert!(read.has_role(&pk(1), StakeRole::Staker));
    assert!(!read.has_role(&pk(1), StakeRole::Withdrawer));

    let status = read.status(10, &StakeHistory::default());
    assert_eq!(status.phase, StakePhase::Undelegated);
    assert_eq!(read.withdrawable(10, &status), 5 * SOL);

    let elsewhere = RawAccount {
        owner: pk(7),
        ..account
    };
    assert!(StakeAccount::parse(pk(5), &elsewhere).is_none());
    let blank = raw(&StakeStateV2::Uninitialized, SOL);
    assert!(StakeAccount::parse(pk(5), &blank).is_none());
}

#[test]
fn a_delegation_warms_up_earns_and_cools_down() {
    let history = history(0..30);
    let stake = delegated(100 * SOL, 10, u64::MAX);

    let warming = stake.status(10, &history);
    assert_eq!(warming.phase, StakePhase::Activating);
    assert_eq!((warming.effective, warming.activating), (0, 100 * SOL));
    // A delegation that was not deactivated keeps all of it locked.
    assert_eq!(stake.withdrawable(10, &warming), 0);

    let earning = stake.status(11, &history);
    assert_eq!(earning.phase, StakePhase::Active);
    assert_eq!(earning.effective, 100 * SOL);

    let leaving = delegated(100 * SOL, 10, 15);
    let last = leaving.status(15, &history);
    assert_eq!(last.phase, StakePhase::Deactivating);
    assert_eq!((last.effective, last.deactivating), (100 * SOL, 100 * SOL));
    assert_eq!(leaving.withdrawable(15, &last), 0);

    let out = leaving.status(16, &history);
    assert_eq!(out.phase, StakePhase::Inactive);
    assert_eq!(leaving.withdrawable(16, &out), leaving.lamports);
}

#[test]
fn a_withdrawal_takes_what_is_free_and_keeps_the_reserve() {
    let history = history(0..30);
    let mut active = delegated(100 * SOL, 10, u64::MAX);
    let status = active.status(12, &history);
    assert_eq!(
        active.withdraw_amount(12, &status, None),
        Err(StakeError::NothingToWithdraw)
    );
    // Lamports sent to the account on top of its stake are free.
    active.lamports += 3 * SOL;
    assert_eq!(active.withdraw_amount(12, &status, None), Ok(3 * SOL));
    assert_eq!(
        active.withdraw_amount(12, &status, Some(4 * SOL)),
        Err(StakeError::TooMuch { available: 3 * SOL })
    );

    let idle = delegated(100 * SOL, 10, 15);
    let status = idle.status(20, &history);
    assert_eq!(idle.withdraw_amount(20, &status, None), Ok(idle.lamports));
    assert_eq!(idle.withdraw_amount(20, &status, Some(SOL)), Ok(SOL));
    assert_eq!(
        idle.withdraw_amount(20, &status, Some(idle.lamports - 1)),
        Err(StakeError::BelowReserve {
            reserve: STAKE_RENT_LAMPORTS
        })
    );
    assert_eq!(
        idle.withdraw_amount(20, &status, Some(0)),
        Err(StakeError::Amount)
    );
}

#[test]
fn merging_needs_matching_authorities_and_compatible_states() {
    let history = history(0..30);
    let epoch = 20;
    let with = |pubkey: Pubkey, mut stake: StakeAccount| {
        stake.pubkey = pubkey;
        let status = stake.status(epoch, &history);
        (stake, status)
    };
    let (active, active_status) = with(pk(20), delegated(10 * SOL, 5, u64::MAX));
    let (other, other_status) = with(pk(21), delegated(4 * SOL, 6, u64::MAX));
    let (idle, idle_status) = with(pk(22), delegated(4 * SOL, 6, 8));
    let (fresh, fresh_status) = with(pk(23), delegated(4 * SOL, epoch, u64::MAX));
    let (leaving, leaving_status) = with(pk(24), delegated(4 * SOL, 6, epoch));

    assert!(active.can_absorb(&active_status, &other, &other_status, epoch));
    assert!(!active.can_absorb(&active_status, &active, &active_status, epoch));
    // Active stake does not take in idle lamports, but new stake does.
    assert!(!active.can_absorb(&active_status, &idle, &idle_status, epoch));
    assert!(fresh.can_absorb(&fresh_status, &idle, &idle_status, epoch));
    assert!(idle.can_absorb(&idle_status, &fresh, &fresh_status, epoch));
    assert!(!active.can_absorb(&active_status, &leaving, &leaving_status, epoch));

    let mut stranger = other.clone();
    stranger.withdrawer = pk(3);
    assert!(!active.can_absorb(&active_status, &stranger, &other_status, epoch));
    let mut elsewhere = other.clone();
    elsewhere.delegation.as_mut().unwrap().voter = pk(8);
    assert!(!active.can_absorb(&active_status, &elsewhere, &other_status, epoch));
}

#[test]
fn sol_amounts_are_read_to_the_lamport() {
    assert_eq!(sol_to_lamports("1"), Ok(SOL));
    assert_eq!(sol_to_lamports(" 1.5 "), Ok(1_500_000_000));
    assert_eq!(sol_to_lamports(".000000001"), Ok(1));
    assert_eq!(sol_to_lamports("2."), Ok(2 * SOL));
    for bad in [
        "",
        ".",
        "1,5",
        "-1",
        "1.0000000001",
        "1e3",
        "99999999999999999999",
    ] {
        assert_eq!(sol_to_lamports(bad), Err(StakeError::Amount), "{bad}");
    }
}

#[test]
fn a_new_stake_is_created_at_a_seed_address_and_delegated() {
    let owner = pk(1);
    let created = create_stake(&owner, 3, 2 * SOL, STAKE_RENT_LAMPORTS, Some(&pk(9))).unwrap();
    assert_eq!(
        created.address,
        Pubkey::create_with_seed(&owner, "stake:3", &STAKE_PROGRAM_ID).unwrap()
    );
    assert_eq!(created.instructions.len(), 3);
    assert_eq!(
        created.instructions[2],
        stake_ix::delegate_stake(&created.address, &owner, &pk(9))
    );
    // Only the owner signs: the new address has no key.
    let signers: Vec<Pubkey> = created
        .instructions
        .iter()
        .flat_map(|ix| ix.accounts.iter())
        .filter(|meta| meta.is_signer)
        .map(|meta| meta.pubkey)
        .collect();
    assert!(signers.iter().all(|signer| *signer == owner));

    let idle = create_stake(&owner, 0, 2 * SOL, STAKE_RENT_LAMPORTS, None).unwrap();
    assert_eq!(idle.instructions.len(), 2);
    assert_eq!(
        create_stake(&owner, 0, STAKE_RENT_LAMPORTS, STAKE_RENT_LAMPORTS, None),
        Err(StakeError::BelowRent {
            reserve: STAKE_RENT_LAMPORTS
        })
    );
}

#[test]
fn a_split_funds_the_new_account_before_it_is_split_into() {
    let source = delegated(10 * SOL, 5, u64::MAX);
    let split = split_stake(&source, 4 * SOL, 1).unwrap();
    assert_eq!(split.address, stake_address(&source.staker, 1).unwrap());
    assert_eq!(
        split.instructions[0],
        solana_system_interface::instruction::transfer(
            &source.staker,
            &split.address,
            STAKE_RENT_LAMPORTS
        )
    );
    assert_eq!(split.instructions.len(), 3);
    assert_eq!(split.instructions[2].program_id, STAKE_PROGRAM_ID);
    assert_eq!(split_stake(&source, 0, 1), Err(StakeError::Amount));
}

#[test]
fn program_accounts_are_read_once_each_and_sorted() {
    let history = history(0..30);
    let small = raw(&StakeStateV2::Initialized(meta(pk(1), pk(1))), SOL);
    let large = raw(&StakeStateV2::Initialized(meta(pk(1), pk(2))), 7 * SOL);
    let mut found = vec![
        StakeAccount::parse(pk(30), &small).unwrap(),
        StakeAccount::parse(pk(31), &large).unwrap(),
    ];
    // The same account, found again under its other authority.
    found.push(StakeAccount::parse(pk(31), &large).unwrap());

    let epoch = EpochInfo {
        epoch: 20,
        slot_index: 108_000,
        slots_in_epoch: 432_000,
    };
    let snapshot = StakeSnapshot::new(epoch, &history, Some(3_000_000), found.clone());
    assert_eq!(snapshot.addresses(), vec![pk(31), pk(30)]);
    // The cluster's current rent replaces what each account recorded.
    assert!(snapshot
        .stakes
        .iter()
        .all(|stake| stake.account.rent_reserve == 3_000_000));
    let mut unasked = StakeSnapshot::new(epoch, &history, None, found);
    assert_eq!(unasked.rent_reserve, STAKE_RENT_LAMPORTS);
    assert!(unasked.can_delegate(STAKE_RENT_LAMPORTS + 1));
    assert!(!unasked.can_delegate(STAKE_RENT_LAMPORTS));
    unasked.min_delegation = SOL;
    assert!(!unasked.can_delegate(SOL));
    assert!(unasked.can_delegate(SOL + STAKE_RENT_LAMPORTS));
    assert_eq!(snapshot.of(&pk(2)).len(), 1);
    assert_eq!(snapshot.of(&pk(1)).len(), 2);
    assert_eq!(snapshot.epoch.percent(), 25);

    // The same accounts read again are the same list. One fewer is another.
    assert!(snapshot.lists_other_accounts_than(None));
    assert!(!snapshot.lists_other_accounts_than(Some(&snapshot.clone())));
    let mut fewer = snapshot.clone();
    fewer.stakes.pop();
    assert!(fewer.lists_other_accounts_than(Some(&snapshot)));

    assert_eq!(STAKE_ACCOUNT_LEN, 200);
    assert_eq!(StakeRole::Staker.offset(), 12);
    assert_eq!(StakeRole::Withdrawer.offset(), 44);
}

#[test]
fn the_stake_history_is_read_from_its_padded_account() {
    let stored = history(3..6);
    let mut data = wincode::serialize(&stored).unwrap();
    // The sysvar account is padded to its full size.
    data.resize(16_392, 0);
    let account = RawAccount {
        lamports: 1,
        data,
        owner: pk(9),
        executable: false,
        rent_epoch: 0,
    };
    assert_eq!(read_stake_history(&account), Some(stored));
    let empty = RawAccount {
        data: Vec::new(),
        ..account
    };
    assert_eq!(read_stake_history(&empty), None);
}

/// Every stake instruction lists exactly the accounts its entry in the
/// embedded IDL names, so the review shows each under its own name.
#[test]
fn every_stake_instruction_names_its_accounts_as_the_idl_does() {
    let (owner, vote, other) = (pk(1), pk(9), pk(8));
    let stake = stake_address(&owner, 3).unwrap();
    let source = delegated(10 * SOL, 5, u64::MAX);
    let built = [
        create_stake(&owner, 3, 2 * SOL, STAKE_RENT_LAMPORTS, Some(&vote))
            .unwrap()
            .instructions,
        split_stake(&source, 4 * SOL, 1).unwrap().instructions,
        delegate_stake(&stake, &owner, &vote),
        deactivate_stake(&stake, &owner),
        withdraw_stake(&stake, &owner, &other, SOL),
        merge_stake(&stake, &other, &owner),
        authorize_stake(&stake, &owner, &other, StakeRole::Staker),
    ]
    .concat();
    let idl = codama_dynamic_client::ProgramClient::from_json(
        crate::idl::builtin::builtin_idl(crate::IdlNetwork::Devnet, &STAKE_PROGRAM_ID)
            .unwrap()
            .json,
    )
    .unwrap();
    for ix in built.iter().filter(|ix| ix.program_id == STAKE_PROGRAM_ID) {
        let parsed = idl.parse_instruction(ix).unwrap().unwrap();
        assert_eq!(parsed.accounts.len(), ix.accounts.len(), "{ix:?}");
        assert!(parsed.remaining_accounts.is_empty(), "{ix:?}");
    }
}
