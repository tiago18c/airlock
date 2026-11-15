//! Keeping the app's event loop from sleeping on work it has.
//!
//! tao's loop on Android waits on a looper for two things: an event of the
//! activity (start, resume, pause, focus, a configuration change), and a wake
//! that says other threads queued something for it: a render to do, a message
//! from the page. Each turn of the loop takes one of the two. When both are
//! ready at once, the looper reports the activity's event alone and clears the
//! wake with it, so what was queued stays queued, and the loop goes back to
//! waiting for a wake that already came. Dioxus asks for one wake and waits
//! to be polled before it asks for another, so nothing more arrives: the page
//! still takes taps and Rust still runs their handlers, but nothing is drawn
//! until something unrelated wakes the loop. Coming back from another app is
//! when it happens, since the activity's events and the app's own work, held
//! up while it was away, all arrive together.
//!
//! So after every turn in which the loop took nothing from its queue, it is
//! woken once more, and what waits there is taken on the next turn.

/// What the loop's handler is shown, as far as a turn goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Seen {
    /// A turn begins.
    Start,
    /// Something from the queue: the loop is emptying it this turn.
    Queued,
    /// An event of the activity.
    Activity,
    /// The turn's events are over.
    End,
}

/// How many turns in a row the loop is woken for nothing. A wake of ours can
/// be cleared by the next event of the activity as any other can, and the
/// handler is not shown every such event (start, stop, a configuration
/// change): a turn that showed nothing may have been one of those, or our own
/// wake finding the queue empty. So it is woken again, this many times at
/// most, and then left to sleep.
const IDLE_WAKES: u8 = 3;

/// Follows the loop's turns, and says after each whether to wake the loop.
#[derive(Debug, Default)]
pub(crate) struct Turns {
    queued: bool,
    activity: bool,
    /// The turn before this one ended with a wake of ours.
    woken: bool,
    /// Wakes of ours in a row that found nothing.
    idle: u8,
}

impl Turns {
    /// Take one thing the handler was shown. True at the end of a turn after
    /// which the loop has to be woken.
    pub(crate) fn see(&mut self, seen: Seen) -> bool {
        match seen {
            Seen::Start => {
                self.queued = false;
                self.activity = false;
                false
            }
            Seen::Queued => {
                self.queued = true;
                false
            }
            Seen::Activity => {
                self.activity = true;
                false
            }
            Seen::End => {
                let ours = std::mem::take(&mut self.woken);
                if self.queued {
                    // The queue was emptied this turn.
                    self.idle = 0;
                    return false;
                }
                if self.activity || !ours {
                    self.idle = 0;
                }
                if self.idle >= IDLE_WAKES {
                    return false;
                }
                self.idle += 1;
                self.woken = true;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(turns: &mut Turns, seen: &[Seen]) -> bool {
        assert!(!turns.see(Seen::Start));
        for one in seen {
            assert!(!turns.see(*one));
        }
        turns.see(Seen::End)
    }

    #[test]
    fn a_turn_that_emptied_the_queue_needs_no_wake() {
        let mut turns = Turns::default();
        assert!(!turn(&mut turns, &[Seen::Queued, Seen::Queued]));
        assert!(!turn(&mut turns, &[Seen::Queued]));
    }

    #[test]
    fn an_activity_event_is_followed_by_a_wake() {
        let mut turns = Turns::default();
        // Resume, then focus: each may have cleared a wake.
        assert!(turn(&mut turns, &[Seen::Activity]));
        assert!(turn(&mut turns, &[Seen::Activity]));
        // The wake finds what was queued.
        assert!(!turn(&mut turns, &[Seen::Queued]));
    }

    #[test]
    fn a_turn_that_showed_nothing_is_followed_by_a_wake() {
        let mut turns = Turns::default();
        // Start or stop: the handler is shown no event for them.
        assert!(turn(&mut turns, &[]));
        assert!(!turn(&mut turns, &[Seen::Queued]));
    }

    #[test]
    fn waking_for_nothing_stops() {
        let mut turns = Turns::default();
        assert!(turn(&mut turns, &[Seen::Activity]));
        let mut wakes = 1;
        while turn(&mut turns, &[]) {
            wakes += 1;
            assert!(wakes < 100, "the loop never sleeps");
        }
        assert_eq!(wakes, IDLE_WAKES);
        // Asleep until something else wakes it; that turn starts afresh.
        assert!(turn(&mut turns, &[]));
    }

    #[test]
    fn hidden_activity_events_in_a_row_are_each_followed_by_a_wake() {
        let mut turns = Turns::default();
        // Stop, then two more the handler is not shown, each clearing the
        // wake before it. The third wake finds the queue.
        assert!(turn(&mut turns, &[]));
        assert!(turn(&mut turns, &[]));
        assert!(turn(&mut turns, &[]));
        assert!(!turn(&mut turns, &[Seen::Queued]));
        // And counting starts again.
        assert!(turn(&mut turns, &[]));
    }
}
