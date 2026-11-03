//! Small runtime helpers shared by desktop and web.

use std::time::Duration;

/// Wait for `duration` without going through the webview.
///
/// Desktop uses tokio; the browser uses `gloo-timers`, so a hidden window
/// does not starve these waits the way a timer in the page can.
pub async fn sleep(duration: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::sleep(duration).await;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let millis = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
        gloo_timers::future::TimeoutFuture::new(millis).await;
    }
}

/// Run `work`, which takes too long for a window to wait on, where it does not
/// wait: on another thread on desktop. A page has only the one, so there it gets
/// the chance to draw first, and then waits.
pub async fn off_the_window<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    #[cfg(not(target_arch = "wasm32"))]
    {
        match tokio::task::spawn_blocking(work).await {
            Ok(out) => out,
            Err(failed) if failed.is_panic() => std::panic::resume_unwind(failed.into_panic()),
            // The runtime is going away, and the window with it.
            Err(_) => std::future::pending().await,
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        sleep(Duration::from_millis(50)).await;
        work()
    }
}

/// [`off_the_window`], waiting `deadline` at most: `None` when that passed. The work
/// goes on to its end, since a thread cannot be stopped. A page has no other thread
/// to run it on, so there it waits for as long as it takes.
pub async fn off_the_window_within<T: Send + 'static>(
    deadline: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::timeout(deadline, off_the_window(work))
            .await
            .ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = deadline;
        Some(off_the_window(work).await)
    }
}
