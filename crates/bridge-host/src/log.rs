//! File logging for the native messaging host. Stdout carries frames to the
//! browser, so nothing else may write there.

use std::path::PathBuf;
use std::sync::{Mutex, Once, OnceLock};

use bridge_protocol::data_dir::{create_private_dir, Moved};

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::prelude::*;

/// Install a rolling file subscriber under the app data dir. Safe to call more
/// than once; later calls are ignored.
pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // No folder to keep logs in: log nothing rather than write beside
        // whatever the browser started this in.
        let Some(data) = data_dir() else {
            return;
        };
        let logs = data.join("logs");
        // `0700` on Unix, as the app makes them. `tracing-appender` makes its
        // files with the default mode, so the folder is what keeps other
        // accounts from reading the log.
        let _ = create_private_dir(&data).and_then(|()| create_private_dir(&logs));
        let appender = match tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("bridge-host")
            .filename_suffix("log")
            .max_log_files(7)
            .build(&logs)
        {
            Ok(appender) => appender,
            Err(_) => return,
        };
        // Written in place: `relay_stdio` ends with `process::exit`, which
        // would drop whatever a `non_blocking` worker still held.

        let filter = match std::env::var("AIRLOCK_LOG") {
            Ok(spec) => EnvFilter::try_new(spec).unwrap_or_else(|_| EnvFilter::new("info")),
            Err(_) => EnvFilter::new("info"),
        };

        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(appender)
                    .with_ansi(false)
                    .with_target(true),
            )
            .try_init();

        let moved = MOVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        match moved {
            Some(Moved::Moved { from, to }) => tracing::info!(
                target: "airlock::bridge",
                from = %from.display(),
                to = %to.display(),
                "moved the data folder to where this platform keeps app data"
            ),
            Some(Moved::Failed { from, to, error }) => tracing::warn!(
                target: "airlock::bridge",
                from = %from.display(),
                to = %to.display(),
                error = %error,
                "could not move the data folder, using it where it is"
            ),
            None => {}
        }
    });
}

/// What finding the folder moved, until logging is up to say so.
static MOVED: Mutex<Option<Moved>> = Mutex::new(None);

/// The online app's data folder, as `app_core::storage::data_dir` finds it:
/// `AIRLOCK_DATA_DIR`, else the folder [`bridge_protocol::data_dir`] names.
/// The browser may start this before the app has ever run, so it moves the
/// folder from where builds before this one kept it too.
pub(crate) fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("AIRLOCK_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let (dir, moved) = bridge_protocol::data_dir::resolve("airlock");
        *MOVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = moved;
        dir
    })
    .clone()
}
