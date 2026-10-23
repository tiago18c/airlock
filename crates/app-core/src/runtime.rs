//! The platform's `Spawner`, `Clock` and `Links`: tokio on native, the
//! browser on the web.

#[cfg(not(target_arch = "wasm32"))]
pub use native::{NativeLinks, TokioClock, TokioSpawner};
#[cfg(target_arch = "wasm32")]
pub use web::{WebClock, WebLinks, WebSpawner};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use crate::ports::{BoxFuture, Clock, Links, Spawner};

    /// Spawns on the tokio runtime it was made in, from whichever thread
    /// asks: the bridge answers on a socket thread of its own, outside it.
    pub struct TokioSpawner {
        handle: tokio::runtime::Handle,
    }

    impl TokioSpawner {
        /// The runtime of the calling thread. Panics outside one.
        pub fn current() -> Self {
            Self {
                handle: tokio::runtime::Handle::current(),
            }
        }
    }

    impl Spawner for TokioSpawner {
        fn spawn(&self, future: BoxFuture) {
            self.handle.spawn(future);
        }
    }

    pub struct TokioClock;

    impl Clock for TokioClock {
        fn now_ms(&self) -> u64 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64)
        }

        fn sleep(&self, duration: Duration) -> BoxFuture {
            Box::pin(tokio::time::sleep(duration))
        }
    }

    /// Opens a URL in the system browser (`url.dll` / `open` / `xdg-open`).
    ///
    /// Dioxus desktop 0.7 does not reliably open `<a target="_blank">` outside
    /// the WebView, so the shell routes explorer links through this port.
    pub struct NativeLinks;

    impl Links for NativeLinks {
        fn open(&self, url: &str) -> Result<(), String> {
            open_url(url)
        }
    }

    fn open_url(url: &str) -> Result<(), String> {
        // Not `cmd /C start`: cmd reads the `&` in a query, such as the
        // explorer's `?cluster=custom&customUrl=…`, as a second command.
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("rundll32")
                .args(["url.dll,FileProtocolHandler", url])
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(url)
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("xdg-open")
                .arg(url)
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::time::Duration;

    use crate::ports::{BoxFuture, Clock, Links, Spawner};

    pub struct WebSpawner;

    impl Spawner for WebSpawner {
        fn spawn(&self, future: BoxFuture) {
            wasm_bindgen_futures::spawn_local(future);
        }
    }

    pub struct WebClock;

    impl Clock for WebClock {
        fn now_ms(&self) -> u64 {
            js_sys::Date::now() as u64
        }

        fn sleep(&self, duration: Duration) -> BoxFuture {
            Box::pin(gloo_timers::future::sleep(duration))
        }
    }

    /// `window.open` without a `window.opener`.
    pub struct WebLinks;

    impl Links for WebLinks {
        fn open(&self, url: &str) -> Result<(), String> {
            let window = web_sys::window().ok_or("no window")?;
            window
                .open_with_url_and_target_and_features(url, "_blank", "noopener,noreferrer")
                .map(|_| ())
                .map_err(|err| format!("{err:?}"))
        }
    }
}
