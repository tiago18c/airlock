//! Logging setup and helpers. Libraries emit through `tracing`; this module
//! installs the subscriber and owns the filters Settings can change.

mod redact;
mod tx;

pub use redact::redact_url;
pub use tx::{log_status, next_flow_id, TxLog};

use std::sync::{Mutex, OnceLock};

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::reload;

#[cfg(not(target_arch = "wasm32"))]
use crate::storage::{data_dir, logs_dir};

/// What a shell passes when it installs logging.
#[derive(Clone, Debug)]
pub struct LogConfig {
    /// `online`, `airgap`, or `web`.
    pub flavour: &'static str,
    /// The shell's version, `env!("CARGO_PKG_VERSION")` in its own crate.
    pub version: &'static str,
}

type FilterHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

static FILTER: OnceLock<FilterHandle> = OnceLock::new();
static CONFIG: OnceLock<LogConfig> = OnceLock::new();
static DIAGNOSTIC: Mutex<bool> = Mutex::new(false);

/// Install the process-wide subscriber. Call once, before `dioxus::launch`.
pub fn init(config: LogConfig) {
    let _ = CONFIG.set(config.clone());
    let filter = env_filter();
    let (filter_layer, handle) = reload::Layer::new(filter);
    let _ = FILTER.set(handle);

    #[cfg(not(target_arch = "wasm32"))]
    {
        init_native(filter_layer, &config);
    }
    #[cfg(target_arch = "wasm32")]
    {
        init_web(filter_layer, &config);
    }
}

#[cfg(target_arch = "wasm32")]
fn init_web(
    filter_layer: reload::Layer<EnvFilter, tracing_subscriber::Registry>,
    config: &LogConfig,
) {
    use std::io::{self, Write};
    use tracing_subscriber::fmt::MakeWriter;

    struct ConsoleMakeWriter;
    struct ConsoleWriter(Vec<u8>);

    impl<'a> MakeWriter<'a> for ConsoleMakeWriter {
        type Writer = ConsoleWriter;
        fn make_writer(&'a self) -> Self::Writer {
            ConsoleWriter(Vec::new())
        }
    }

    impl Write for ConsoleWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for ConsoleWriter {
        fn drop(&mut self) {
            if self.0.is_empty() {
                return;
            }
            let text = String::from_utf8_lossy(&self.0);
            web_sys::console::log_1(&text.as_ref().into());
        }
    }

    let _ = tracing_subscriber::registry()
        .with(filter_layer)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(ConsoleMakeWriter),
        )
        .try_init();

    tracing::info!(
        target: "airlock::app",
        version = config.version,
        flavour = config.flavour,
        "started"
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn init_native(
    filter_layer: reload::Layer<EnvFilter, tracing_subscriber::Registry>,
    config: &LogConfig,
) {
    use tracing_subscriber::fmt;

    let logs = logs_dir();
    if let Some(logs) = &logs {
        let _ = std::fs::create_dir_all(logs);
    }
    // Written in place rather than through `non_blocking`: its worker only
    // flushes when its guard drops, and the process exits without dropping
    // it, so the last lines (`stopped` among them) would be lost.
    let appender = logs
        .as_ref()
        .ok_or_else(|| "no data folder is named".to_string())
        .and_then(|logs| {
            tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix(crate::storage::app_dirs().folder)
                .filename_suffix("log")
                .max_log_files(7)
                .build(logs)
                .map_err(|err| err.to_string())
        });
    let file_error = appender.as_ref().err().cloned();
    // A log folder that cannot be written must not stop the app.
    let file_layer = appender.ok().map(|appender| {
        fmt::layer()
            .with_writer(appender)
            .with_ansi(false)
            .with_target(true)
    });

    let registry = tracing_subscriber::registry()
        .with(filter_layer)
        .with(file_layer);

    #[cfg(debug_assertions)]
    {
        let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(true);
        let _ = registry.with(stderr_layer).try_init();
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = registry.try_init();
    }

    if let Some(error) = file_error {
        tracing::error!(
            target: "airlock::app",
            logs_dir = %shown(logs.as_deref()),
            error = %error,
            "could not open the log file"
        );
    }

    let data = data_dir();
    tracing::info!(
        target: "airlock::app",
        version = config.version,
        flavour = config.flavour,
        data_dir = %shown(data.as_deref()),
        "started"
    );
}

/// The variable that sets the log filter: `AIRLOCK_LOG`, or
/// `AIRLOCK_OFFLINE_LOG` in Airlock Offline.
pub fn log_env_var() -> String {
    crate::storage::app_dirs().env_var("LOG")
}

fn env_filter() -> EnvFilter {
    match std::env::var(log_env_var()) {
        Ok(spec) => EnvFilter::try_new(spec).unwrap_or_else(|_| EnvFilter::new("info")),
        Err(_) => EnvFilter::new("info"),
    }
}

/// What Diagnostic logging turns on: `debug` for this app's own crates only.
/// Dependencies stay at `info`; an HTTP client at `debug` may log whole URLs,
/// RPC keys in the query included.
const DIAGNOSTIC_FILTER: &str = concat!(
    "info,airlock=debug,app_core=debug,wallet_core=debug,wallet_flows=debug,",
    "wallet_rpc=debug,wallet_vault=debug,bridge_host=debug,bridge_protocol=debug",
);

/// Raise the filter to `debug` for the rest of this process (Settings toggle).
pub fn set_diagnostic_logging(on: bool) {
    if let Ok(mut flag) = DIAGNOSTIC.lock() {
        *flag = on;
    }
    let Some(handle) = FILTER.get() else {
        return;
    };
    let next = if on {
        EnvFilter::new(DIAGNOSTIC_FILTER)
    } else {
        env_filter()
    };
    let _ = handle.reload(next);
}

pub fn diagnostic_logging() -> bool {
    DIAGNOSTIC.lock().map(|g| *g).unwrap_or(false)
}

/// Version, OS, data dir and the full contents of every log file in the log dir.
#[cfg(not(target_arch = "wasm32"))]
pub fn diagnostics_bundle() -> String {
    let data = data_dir();
    let logs = logs_dir();
    let (version, flavour) = shell();
    let mut out = String::new();
    out.push_str(&format!(
        "Airlock diagnostics\nversion: {version}\nflavour: {flavour}\nos: {}\ndata_dir: {}\nlogs_dir: {}\n\n",
        std::env::consts::OS,
        shown(data.as_deref()),
        shown(logs.as_deref()),
    ));
    out.push_str(
        "WARNING: These logs include full addresses and transaction history (base64 wires and signatures). Share only with people you trust.\n\n",
    );
    let Some(Ok(entries)) = logs.as_ref().map(std::fs::read_dir) else {
        out.push_str("(no log directory)\n");
        return out;
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
        .collect();
    files.sort_by_key(|e| e.file_name());
    if files.is_empty() {
        out.push_str("(no log files)\n");
        return out;
    }
    for entry in files {
        let path = entry.path();
        out.push_str("--- ");
        out.push_str(&path.file_name().unwrap_or_default().to_string_lossy());
        out.push_str(" ---\n");
        match std::fs::read_to_string(&path) {
            Ok(text) => out.push_str(&text),
            Err(err) => out.push_str(&format!("(could not read: {err})\n")),
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

#[cfg(target_arch = "wasm32")]
pub fn diagnostics_bundle() -> String {
    let (version, _) = shell();
    format!(
        "Airlock diagnostics\nversion: {version}\nflavour: web\nos: {}\n\n(web build: logs are in the browser console)\n",
        std::env::consts::OS,
    )
}

/// Version and flavour the shell gave `init`.
fn shell() -> (&'static str, &'static str) {
    CONFIG
        .get()
        .map_or(("unknown", "unknown"), |c| (c.version, c.flavour))
}

/// A folder for a log line, or that there is none.
#[cfg(not(target_arch = "wasm32"))]
fn shown(path: Option<&std::path::Path>) -> String {
    path.map_or_else(|| "(none)".into(), |path| path.display().to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn open_log_folder() -> Result<(), String> {
    let dir = logs_dir().ok_or_else(|| "No data folder is named.".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    open_path(&dir)
}

#[cfg(not(target_arch = "wasm32"))]
fn open_path(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

pub fn log_stopped() {
    tracing::info!(target: "airlock::app", "stopped");
}

#[cfg(test)]
mod tests {
    use super::DIAGNOSTIC_FILTER;
    use tracing_subscriber::filter::EnvFilter;

    #[test]
    fn diagnostic_filter_parses() {
        assert!(EnvFilter::try_new(DIAGNOSTIC_FILTER).is_ok());
    }
}
