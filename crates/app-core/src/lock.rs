//! Locking the vault by itself.

use std::fmt;
use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::task::Poll;

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
            // Timeout is read at the start of each wait; a Settings change
            // applies on the next cycle (next hide, or next idle restart).
            let timeout = self.settings.borrow().auto_lock;
            if shown {
                // Drop any activity that arrived while we were hidden so the
                // idle clock starts fresh.
                self.activity.borrow_and_update();
                let sleep = self.clock.sleep(timeout);
                match race(sleep, race(self.visible.changed(), self.activity.changed())).await {
                    Either::Left(()) => {
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
                    Either::Right(Either::Left(Ok(()))) | Either::Right(Either::Right(Ok(()))) => {}
                    Either::Right(Either::Left(Err(_))) | Either::Right(Either::Right(Err(_))) => {
                        return
                    }
                }
            } else {
                let sleep = self.clock.sleep(timeout);
                match race(sleep, self.visible.changed()).await {
                    Either::Left(()) => {
                        let Some(core) = self.core.upgrade() else {
                            return;
                        };
                        core.lock(LockReason::Hidden);
                        // Nothing more to time until the window is shown again.
                        if self.visible.changed().await.is_err() {
                            return;
                        }
                    }
                    Either::Right(Ok(())) => {}
                    Either::Right(Err(_)) => return,
                }
            }
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
