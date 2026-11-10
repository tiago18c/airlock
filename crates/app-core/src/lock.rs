//! Locking the vault by itself.

use std::fmt;
use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::task::Poll;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::commands::Inner;
use crate::ports::Clock;
use crate::slices::Settings;

/// Why the vault locked. Logged as `reason=` and sent with `VaultLocked`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LockReason {
    /// The user asked to.
    User,
    /// The window stayed hidden past the timeout in Settings.
    Hidden,
    /// No input while the window was visible, past the same timeout.
    Idle,
}

impl LockReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Hidden => "hidden",
            Self::Idle => "idle",
        }
    }
}

impl fmt::Display for LockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The longest the policy sleeps at once. A monotonic timer stops while the
/// machine is asleep on Linux and macOS, so a laptop closed partway through
/// the timeout would wake with the rest of it still to wait. Waking this
/// often to look at the wall clock locks it at the first step after waking.
pub(crate) const STEP: Duration = Duration::from_secs(15);

/// Locks the vault once the window has been hidden for the timeout in
/// Settings, or after the same time with no activity while visible. The shell
/// reports `ui_visible` and pings `ui_activity`. Runs on the core's `Spawner`,
/// so the airgap build gets it too.
pub struct LockPolicy {
    visible: watch::Receiver<bool>,
    activity: watch::Receiver<()>,
    settings: watch::Receiver<Arc<Settings>>,
    clock: Arc<dyn Clock>,
    core: Weak<Inner>,
}

impl LockPolicy {
    pub(crate) fn new(
        visible: watch::Receiver<bool>,
        activity: watch::Receiver<()>,
        settings: watch::Receiver<Arc<Settings>>,
        clock: Arc<dyn Clock>,
        core: Weak<Inner>,
    ) -> Self {
        Self {
            visible,
            activity,
            settings,
            clock,
            core,
        }
    }

    /// Ends when the core is dropped.
    pub(crate) async fn run(mut self) {
        loop {
            let shown = *self.visible.borrow_and_update();
            if shown {
                // Drop any activity that arrived while we were hidden so the
                // idle clock starts fresh.
                self.activity.borrow_and_update();
                let interrupt = race(self.visible.changed(), self.activity.changed());
                match wait_out(&*self.clock, &mut self.settings, interrupt).await {
                    Waited::Closed => return,
                    Waited::Elapsed => {
                        let Some(core) = self.core.upgrade() else {
                            return;
                        };
                        core.lock(LockReason::Idle);
                        // Wait for input or a hide before timing idle again.
                        match race(self.activity.changed(), self.visible.changed()).await {
                            Either::Left(Err(_)) | Either::Right(Err(_)) => return,
                            Either::Left(Ok(())) | Either::Right(Ok(())) => {}
                        }
                    }
                    Waited::Interrupted(Either::Left(Ok(())) | Either::Right(Ok(()))) => {}
                    Waited::Interrupted(Either::Left(Err(_)) | Either::Right(Err(_))) => return,
                }
            } else {
                match wait_out(&*self.clock, &mut self.settings, self.visible.changed()).await {
                    Waited::Closed => return,
                    Waited::Elapsed => {
                        let Some(core) = self.core.upgrade() else {
                            return;
                        };
                        core.lock(LockReason::Hidden);
                        // Nothing more to time until the window is shown again.
                        if self.visible.changed().await.is_err() {
                            return;
                        }
                    }
                    Waited::Interrupted(Ok(())) => {}
                    Waited::Interrupted(Err(_)) => return,
                }
            }
        }
    }
}

/// How a [`wait_out`] ended.
enum Waited<T> {
    /// The timeout passed.
    Elapsed,
    /// What it was raced with finished first.
    Interrupted(T),
    /// The settings are gone with the core.
    Closed,
}

/// Wait until the auto-lock timeout has passed since this was called, or
/// `interrupt` finishes. Time is measured on both clocks, and whichever says
/// more has passed wins: the steps slept, which a wall clock set back cannot
/// stretch, and the wall clock, which goes on while the machine sleeps. The
/// timeout is read again whenever Settings changes, against the same start,
/// so a shorter one applies at once. A step cut short by that change is not
/// counted on the first clock, only on the wall clock.
async fn wait_out<F: Future>(
    clock: &dyn Clock,
    settings: &mut watch::Receiver<Arc<Settings>>,
    interrupt: F,
) -> Waited<F::Output> {
    let started_ms = clock.now_ms();
    let mut slept = Duration::ZERO;
    let mut interrupt = pin!(interrupt);
    loop {
        let timeout = settings.borrow_and_update().auto_lock;
        let wall = Duration::from_millis(clock.now_ms().saturating_sub(started_ms));
        let left = timeout.saturating_sub(wall.max(slept));
        if left.is_zero() {
            return Waited::Elapsed;
        }
        let step = left.min(STEP);
        match race(
            clock.sleep(step),
            race(interrupt.as_mut(), settings.changed()),
        )
        .await
        {
            Either::Left(()) => slept += step,
            Either::Right(Either::Left(value)) => return Waited::Interrupted(value),
            Either::Right(Either::Right(Ok(()))) => {}
            Either::Right(Either::Right(Err(_))) => return Waited::Closed,
        }
    }
}

enum Either<A, B> {
    Left(A),
    Right(B),
}

/// Whichever of the two finishes first; the other is dropped.
async fn race<A: Future, B: Future>(a: A, b: B) -> Either<A::Output, B::Output> {
    let mut a = pin!(a);
    let mut b = pin!(b);
    poll_fn(|cx| {
        if let Poll::Ready(value) = a.as_mut().poll(cx) {
            return Poll::Ready(Either::Left(value));
        }
        if let Poll::Ready(value) = b.as_mut().poll(cx) {
            return Poll::Ready(Either::Right(value));
        }
        Poll::Pending
    })
    .await
}
