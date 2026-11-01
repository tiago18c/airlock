//! Locks that know the order they are taken in.
//!
//! Two threads that take the same two locks in opposite orders can each end up
//! waiting for the other, and nothing shows until the day they do. So each lock has
//! a rank, and a debug build panics as soon as one is taken while a later one is
//! held, on the path that does it, whether or not the other thread exists yet.

use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The order the core's locks are taken in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Rank {
    Vault,
    LeafMarks,
    WalletList,
    Workspace,
    Holdings,
    Notices,
    Bridge,
    Reach,
}

pub(crate) struct Ranked<T> {
    rank: Rank,
    mutex: Mutex<T>,
}

impl<T> Ranked<T> {
    pub(crate) fn new(rank: Rank, value: T) -> Self {
        Self {
            rank,
            mutex: Mutex::new(value),
        }
    }

    pub(crate) fn lock(&self) -> RankedGuard<'_, T> {
        held::taking(self.rank);
        RankedGuard {
            // A panic elsewhere must not stop a save or a lock.
            guard: self.mutex.lock().unwrap_or_else(PoisonError::into_inner),
            rank: self.rank,
        }
    }
}

pub(crate) struct RankedGuard<'a, T> {
    guard: MutexGuard<'a, T>,
    rank: Rank,
}

impl<T> Deref for RankedGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for RankedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> Drop for RankedGuard<'_, T> {
    fn drop(&mut self) {
        held::released(self.rank);
    }
}

#[cfg(debug_assertions)]
mod held {
    use super::Rank;
    use std::cell::RefCell;

    thread_local! {
        static HELD: RefCell<Vec<Rank>> = const { RefCell::new(Vec::new()) };
    }

    // `try_with`: a core dropped as its thread ends may outlive this list, and
    // nothing can be waiting on a thread that is going away.
    pub(super) fn taking(rank: Rank) {
        let _ = HELD.try_with(|held| {
            let mut held = held.borrow_mut();
            if let Some(later) = held.iter().find(|other| **other >= rank).copied() {
                // Not while unwinding from another: that would abort, and say less.
                if !std::thread::panicking() {
                    drop(held);
                    panic!("the {rank:?} lock was taken while the {later:?} lock was held");
                }
            }
            held.push(rank);
        });
    }

    pub(super) fn released(rank: Rank) {
        let _ = HELD.try_with(|held| {
            let mut held = held.borrow_mut();
            if let Some(at) = held.iter().rposition(|other| *other == rank) {
                held.remove(at);
            }
        });
    }
}

#[cfg(not(debug_assertions))]
mod held {
    use super::Rank;

    pub(super) fn taking(_rank: Rank) {}

    pub(super) fn released(_rank: Rank) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locks() -> (Ranked<u8>, Ranked<u8>, Ranked<u8>) {
        (
            Ranked::new(Rank::Vault, 1),
            Ranked::new(Rank::Workspace, 2),
            Ranked::new(Rank::Reach, 3),
        )
    }

    #[test]
    fn locks_taken_in_order_are_fine_and_give_what_they_hold() {
        let (vault, workspace, reach) = locks();
        let mut first = vault.lock();
        let second = workspace.lock();
        let third = reach.lock();
        *first += 1;
        assert_eq!((*first, *second, *third), (2, 2, 3));
    }

    #[test]
    fn a_lock_let_go_no_longer_counts_whatever_the_order_it_went_in() {
        let (vault, workspace, reach) = locks();
        let first = vault.lock();
        let second = workspace.lock();
        // Let go of the earlier one first: what is held is still the later one.
        drop(first);
        let third = reach.lock();
        drop(second);
        drop(third);
        // With nothing held, any order is fine again.
        let _reach = reach.lock();
        drop(_reach);
        let _vault = vault.lock();
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "Vault lock was taken while")
    )]
    fn a_lock_taken_while_a_later_one_is_held_is_caught_in_a_debug_build() {
        let (vault, workspace, _) = locks();
        let _second = workspace.lock();
        let _first = vault.lock();
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "Vault lock was taken while")
    )]
    fn a_lock_of_the_same_rank_is_caught_too() {
        let first = Ranked::new(Rank::Vault, 1);
        let second = Ranked::new(Rank::Vault, 2);
        let _first = first.lock();
        let _second = second.lock();
    }

    #[test]
    fn a_lock_on_another_thread_says_nothing_of_this_one() {
        let (vault, workspace, _) = locks();
        let _second = workspace.lock();
        std::thread::scope(|scope| {
            scope.spawn(|| drop(vault.lock())).join().unwrap();
        });
    }
}
