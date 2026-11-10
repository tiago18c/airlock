use super::*;
use crate::wallets::stake::StakeLockup;
use crate::{STAKE_PROGRAM_ID, SYSTEM_PROGRAM_ID};

fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

const OWNER: u8 = 1;
const TO: u8 = 2;
const NOW: Now = Now {
    epoch: 100,
    unix_time: 1_000_000,
};

fn stake(staker: u8, withdrawer: u8) -> StakeAccount {
    StakeAccount {
        pubkey: pk(50),
        lamports: 5_000_000_000,
        rent_reserve: 2_282_880,
        staker: pk(staker),
        withdrawer: pk(withdrawer),
        lockup: StakeLockup::default(),
        delegation: None,
    }
}

/// The role each instruction hands over: the stake program's authorize
/// tag, then the role (0 staker, 1 withdrawer).
fn roles(instructions: &[Instruction]) -> Vec<u32> {
    instructions
        .iter()
        .map(|ix| {
            assert_eq!(ix.program_id, STAKE_PROGRAM_ID);
            assert_eq!(u32::from_le_bytes(ix.data[0..4].try_into().unwrap()), 1);
            u32::from_le_bytes(ix.data[36..40].try_into().unwrap())
        })
        .collect()
}

#[test]
fn both_authorities_go_over_staker_first() {
    let (outcome, ixs) = hand_over_stake(
        pk(OWNER),
        pk(TO),
        &stake(OWNER, OWNER),
        StakeChoice::BOTH,
        NOW,
    );
    assert_eq!(
        outcome,
        StakeOutcome::Handed {
            staker: true,
            withdrawer: true
        }
    );
    assert_eq!(roles(&ixs), vec![0, 1]);
    // The new authority is named in each.
    assert!(ixs.iter().all(|ix| ix.data[4..36] == *pk(TO).as_ref()));
}

#[test]
fn a_withdrawer_may_hand_over_the_staker_too() {
    let (outcome, ixs) =
        hand_over_stake(pk(OWNER), pk(TO), &stake(9, OWNER), StakeChoice::BOTH, NOW);
    assert_eq!(
        outcome,
        StakeOutcome::Handed {
            staker: true,
            withdrawer: true
        }
    );
    assert_eq!(roles(&ixs), vec![0, 1]);
}

#[test]
fn a_staker_alone_hands_over_the_staker_alone() {
    let (outcome, ixs) =
        hand_over_stake(pk(OWNER), pk(TO), &stake(OWNER, 9), StakeChoice::BOTH, NOW);
    assert_eq!(
        outcome,
        StakeOutcome::Handed {
            staker: true,
            withdrawer: false
        }
    );
    assert_eq!(roles(&ixs), vec![0]);
    let only_withdrawer = StakeChoice {
        staker: false,
        withdrawer: true,
    };
    let (outcome, ixs) = hand_over_stake(pk(OWNER), pk(TO), &stake(OWNER, 9), only_withdrawer, NOW);
    assert_eq!(outcome, StakeOutcome::Left(StakeBlocked::NotAuthority));
    assert!(ixs.is_empty());
}

#[test]
fn a_lockup_in_force_keeps_the_withdrawer_unless_the_wallet_is_its_custodian() {
    let mut locked = stake(OWNER, OWNER);
    locked.lockup = StakeLockup {
        unix_timestamp: 0,
        epoch: 200,
        custodian: pk(7),
    };
    let (outcome, ixs) = hand_over_stake(pk(OWNER), pk(TO), &locked, StakeChoice::BOTH, NOW);
    assert_eq!(
        outcome,
        StakeOutcome::Handed {
            staker: true,
            withdrawer: false
        }
    );
    assert_eq!(roles(&ixs), vec![0]);
    let withdrawer_only = StakeChoice {
        staker: false,
        withdrawer: true,
    };
    assert_eq!(
        hand_over_stake(pk(OWNER), pk(TO), &locked, withdrawer_only, NOW).0,
        StakeOutcome::Left(StakeBlocked::Lockup { custodian: pk(7) })
    );

    // Its custodian may, and once the lockup is over anyone who withdraws may.
    locked.lockup.custodian = pk(OWNER);
    assert_eq!(
        roles(&hand_over_stake(pk(OWNER), pk(TO), &locked, withdrawer_only, NOW).1),
        vec![1]
    );
    locked.lockup.custodian = pk(7);
    let later = Now {
        epoch: 201,
        unix_time: NOW.unix_time,
    };
    assert_eq!(
        roles(&hand_over_stake(pk(OWNER), pk(TO), &locked, withdrawer_only, later).1),
        vec![1]
    );
}

#[test]
fn nothing_chosen_or_nothing_held_leaves_it() {
    assert_eq!(
        hand_over_stake(
            pk(OWNER),
            pk(TO),
            &stake(OWNER, OWNER),
            StakeChoice::default(),
            NOW
        )
        .0,
        StakeOutcome::Left(StakeBlocked::Chosen)
    );
    assert_eq!(
        hand_over_stake(pk(OWNER), pk(TO), &stake(8, 9), StakeChoice::BOTH, NOW).0,
        StakeOutcome::Left(StakeBlocked::NotAuthority)
    );
}

#[test]
fn a_nonce_closes_into_the_wallet_unless_its_transactions_live_on_it() {
    let nonce = NonceHeld {
        address: pk(60),
        lamports: 1_447_680,
    };
    let (outcome, ixs) = close_nonce(pk(OWNER), &nonce, true, None);
    assert_eq!(
        outcome,
        NonceOutcome::Closed {
            lamports: 1_447_680
        }
    );
    assert_eq!(ixs.len(), 1);
    assert_eq!(ixs[0].program_id, SYSTEM_PROGRAM_ID);
    // WithdrawNonceAccount: tag 5, then the lamports.
    assert_eq!(u32::from_le_bytes(ixs[0].data[0..4].try_into().unwrap()), 5);
    assert_eq!(
        u64::from_le_bytes(ixs[0].data[4..12].try_into().unwrap()),
        1_447_680
    );
    assert_eq!(ixs[0].accounts[1].pubkey, pk(OWNER));

    assert_eq!(
        close_nonce(pk(OWNER), &nonce, true, Some(pk(60))).0,
        NonceOutcome::Left(NonceBlocked::InUse)
    );
    assert_eq!(
        close_nonce(pk(OWNER), &nonce, false, None).0,
        NonceOutcome::Left(NonceBlocked::Chosen)
    );
}

#[test]
fn a_sweep_empties_a_plain_account_and_keeps_rent_in_any_other() {
    let plain = RawAccount {
        lamports: 10,
        data: Vec::new(),
        owner: SYSTEM_PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    };
    assert_eq!(must_keep(&plain, 890_880), 0);
    let vector = RawAccount {
        data: vec![0; 100],
        owner: pk(77),
        ..plain
    };
    assert_eq!(must_keep(&vector, 1_586_880), 1_586_880);

    assert_eq!(sweep_amount(1_000_000, 500_000, 5_000, 0), 1_495_000);
    assert_eq!(sweep_amount(1_000_000, 0, 0, 1_586_880), 0);
    assert_eq!(sweep_amount(2_000_000, 0, 0, 1_586_880), 413_120);
}

#[test]
fn ops_are_packed_whole_with_room_left_for_the_sweep() {
    let op = |n: usize| -> Vec<Instruction> { (0..n).map(|_| sweep(pk(1), pk(2), 1)).collect() };
    let steps = chunk_ops(vec![op(2), op(2), op(1), Vec::new(), op(2)], 4);
    let sizes: Vec<usize> = steps.iter().map(Vec::len).collect();
    assert_eq!(sizes, vec![4, 3]);
    assert!(chunk_ops(Vec::new(), 4).is_empty());
}
