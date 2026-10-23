//! File logging for the native messaging host. Stdout carries frames to the
//! browser, so nothing else may write there.

use std::sync::Once;

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::prelude::*;

/// Install a rolling file subscriber under the app data dir. Safe to call more
/// than once; later calls are ignored.
pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let logs = data_dir().join("logs");
        let _ = std::fs::create_dir_all(&logs);
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
    });
}

fn data_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("AIRLOCK_DATA_DIR") {
        return std::path::PathBuf::from(dir);
    }
    let mut dir = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    dir.push("airlock");
    dir
}
