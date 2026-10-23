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
