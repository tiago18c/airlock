//! Desktop side of the browser bridge: the socket the extension's native host
//! connects to. A request the user must decide goes into the core's queue,
//! which the window reads and answers (`Core::bridge_answer`); this module
//! only waits for that answer and writes it back.
//!
//! A second launch that cannot bind the socket sends `show` and exits. What
//! keeps two processes off one data folder is the folder's own lock
//! (`app_core::storage::claim_data_dir`), which holds even when the socket
//! cannot be named.

use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use app_core::bridge::{BridgeQueue, BridgeRequest, SiteConnect, SiteSign};
use app_core::PublicCore;
use base64::Engine;
use bridge_protocol::reply;
use serde_json::{json, Value};
use solana_pubkey::Pubkey;
use tokio::sync::watch;

#[cfg(test)]
mod tests;

/// How long a prompt waits for a decision in the window.
const DECISION_TIMEOUT: Duration = Duration::from_secs(600);

pub use bridge_protocol::{request_code, MAX_TRANSACTIONS};

/// The bridge of this process. `main` makes it before the window, so the
/// socket can be claimed first, and hands it to the window as context.
#[derive(Clone)]
pub struct Bridge {
    shared: Arc<Shared>,
}

struct Shared {
    /// Set once the window opens the core. Until then a request that needs
    /// the user is told the app is starting.
    core: OnceLock<PublicCore>,
    /// Bumped when a second launch asks this one to show its window.
    show: watch::Sender<u64>,
    /// Native hosts connected now: one per browser whose extension has
    /// talked to the app since it last started.
    extension: watch::Sender<usize>,
}

/// Result of claiming the bridge socket before the UI starts.
pub enum Bootstrap {
    /// This process owns the socket and is serving it.
    Primary(Bridge),
    /// Another instance is running; it was asked to show its window.
    AlreadyRunning,
}

impl Bridge {
    fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                core: OnceLock::new(),
                show: watch::channel(0).0,
                extension: watch::channel(0).0,
            }),
        }
    }

    /// Bind the bridge socket or ask the running instance to show. Call from
    /// `main` before `dioxus::launch`.
    pub fn bootstrap() -> Bootstrap {
        let bridge = Bridge::new();
        let endpoint = match bridge_host::endpoint::desktop_endpoint() {
            Ok(endpoint) => endpoint,
            Err(err) => {
                tracing::error!(
                    target: "airlock::bridge",
                    error = %err,
                    "could not resolve bridge endpoint"
                );
                return Bootstrap::Primary(bridge);
            }
        };
        if let Err(err) = bridge_host::endpoint::prepare_directory(&endpoint) {
            tracing::warn!(
                target: "airlock::bridge",
                error = %err,
                "could not prepare bridge directory"
            );
        }
        match bridge_host::endpoint::bind_endpoint(&endpoint) {
            Ok(listener) => {
                let serving = bridge.clone();
                std::thread::spawn(move || {
                    let accept = move || -> bridge_host::relay::Connection {
                        let mut peer = Peer::new(serving.clone());
                        Box::new(move |message: &[u8]| peer.answer(message))
                    };
                    if let Err(err) = bridge_host::relay::serve_connections(listener, accept) {
                        tracing::error!(
                            target: "airlock::bridge",
                            error = %err,
                            "bridge server failed"
                        );
                    }
                });
                tracing::info!(target: "airlock::bridge", "listening");
                Bootstrap::Primary(bridge)
            }
            Err(_) => {
                if let Err(err) = ask_running_to_show() {
                    tracing::warn!(
                        target: "airlock::bridge",
                        error = %err,
                        "could not ask running instance to show"
                    );
                }
                Bootstrap::AlreadyRunning
            }
        }
    }

    /// Let requests reach the core. Call once, when the window opens it.
    pub fn attach(&self, core: PublicCore) {
        let _ = self.shared.core.set(core);
    }

    /// Changes each time a second launch asks this one to show its window.
    pub fn subscribe_show(&self) -> watch::Receiver<u64> {
        self.shared.show.subscribe()
    }

    /// How many browsers' extensions are connected. Changes as their native
    /// hosts connect and go.
    pub fn subscribe_extension(&self) -> watch::Receiver<usize> {
        self.shared.extension.subscribe()
    }

    /// The reply to one frame from the native host.
    fn answer(&self, message: &[u8]) -> Vec<u8> {
        let value: Value = match serde_json::from_slice(message) {
            Ok(value) => value,
            Err(_) => return reply::error("", "bad_request", "The request is not JSON."),
        };
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if value.get("v").and_then(Value::as_u64) != Some(1) {
            return reply::error(&id, "bad_request", "Unsupported bridge version.");
        }
        if let Some(expires) = value.get("expiresAt").and_then(Value::as_u64) {
            if expires < now_secs() {
                return reply::error(&id, "expired", "The request expired.");
            }
        }
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let origin = value
            .get("origin")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let params = value.get("params").cloned().unwrap_or(Value::Null);

        let core = self.shared.core.get();
        let silent =
            method == "connect" && params.get("silent").and_then(Value::as_bool) == Some(true);
        // A silent connect never gets an error: the site just stays out.
        if silent && core.is_none() {
            return reply::result(&id, json!({ "accounts": [] }));
        }
        if waits_for_user(method) && core.is_none() {
            return reply::error(&id, "busy", "Airlock is starting.");
        }

        let code = request_code(&id);
        tracing::debug!(
            target: "airlock::bridge",
            code = %code,
            method = %method,
            origin = %origin,
            "request in"
        );

        match (method, core) {
            ("status", _) => reply::result(&id, json!({ "ready": true, "role": "desktop" })),
            ("show", _) => {
                self.shared.show.send_modify(|n| *n = n.wrapping_add(1));
                reply::result(&id, json!({}))
            }
            ("disconnect", core) => {
                if let Some(core) = core {
                    core.site_disconnected(&origin);
                }
                reply::result(&id, json!({}))
            }
            ("signTransaction", _) if transaction_count(&params) == 0 => {
                reply::error(&id, "bad_request", "No transaction was included.")
            }
            ("signTransaction", _) if transaction_count(&params) > MAX_TRANSACTIONS => {
                reply::error(
                    &id,
                    "not_implemented",
                    &format!("Airlock signs at most {MAX_TRANSACTIONS} transactions per request."),
                )
            }
            ("connect", Some(core)) => match core.site_connect(&origin, silent) {
                SiteConnect::Allowed(accounts) => reply::connected(&id, &accounts),
                _ if silent => reply::result(&id, json!({ "accounts": [] })),
                SiteConnect::Blocked => blocked(&id),
                SiteConnect::Undecided => wait_for_user(
                    core,
                    BridgeRequest {
                        id,
                        origin,
                        method: method.to_owned(),
                        params,
                    },
                    DECISION_TIMEOUT,
                ),
            },
            ("signTransaction", Some(core)) => {
                let accounts = requested_accounts(&params);
                match core.site_sign(&origin, &accounts) {
                    SiteSign::Blocked => blocked(&id),
                    SiteSign::Allowed if accounts.len() == transaction_count(&params) => {
                        wait_for_user(
                            core,
                            BridgeRequest {
                                id,
                                origin,
                                method: method.to_owned(),
                                params,
                            },
                            DECISION_TIMEOUT,
                        )
                    }
                    // A transaction that names no account is not for one the
                    // site was given.
                    _ => reply::error(&id, "rejected", NOT_CONNECTED),
                }
            }
            _ => reply::error(
                &id,
                "not_implemented",
                "This prototype approves connect and signTransaction.",
            ),
        }
    }
}

const NOT_CONNECTED: &str = "This account is not connected to the site.";

/// The reply to a site the user blocked.
fn blocked(id: &str) -> Vec<u8> {
    reply::error(id, "rejected", "This site is blocked in Airlock.")
}

/// One connection to the socket. It counts as an extension once it sends
/// anything but `show`, which is a second launch knocking, and stops counting
/// when it closes.
struct Peer {
    bridge: Bridge,
    counted: bool,
}

impl Peer {
    fn new(bridge: Bridge) -> Self {
        Self {
            bridge,
            counted: false,
        }
    }

    fn answer(&mut self, message: &[u8]) -> Vec<u8> {
        if !self.counted && !is_show(message) {
            self.counted = true;
            self.bridge.shared.extension.send_modify(|n| *n += 1);
        }
        self.bridge.answer(message)
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if self.counted {
            self.bridge
                .shared
                .extension
                .send_modify(|n| *n = n.saturating_sub(1));
        }
    }
}

fn is_show(message: &[u8]) -> bool {
    serde_json::from_slice::<Value>(message)
        .ok()
        .and_then(|value| value.get("method")?.as_str().map(|m| m == "show"))
        .unwrap_or(false)
}

/// Whether `method` goes to the user, through the core's queue. The rest,
/// `show` among them, is answered here even before the core opens.
fn waits_for_user(method: &str) -> bool {
    matches!(method, "connect" | "signTransaction")
}

/// Queue `request` in the core and wait for the window's answer, at most
/// `timeout`.
fn wait_for_user(core: &PublicCore, request: BridgeRequest, timeout: Duration) -> Vec<u8> {
    let id = request.id.clone();
    tracing::debug!(
        target: "airlock::bridge",
        code = %request.code(),
        method = %request.method,
        origin = %request.origin,
        "request queued"
    );
    let (tx, rx) = mpsc::channel();
    core.bridge_push(
        request,
        Box::new(move |bytes| {
            let _ = tx.send(bytes);
        }),
    );
    if let Ok(bytes) = rx.recv_timeout(timeout) {
        return bytes;
    }
    // Nobody decided. An answer that lands while the request is being
    // dropped still wins.
    if !core.bridge_expire(&id) {
        if let Ok(bytes) = rx.try_recv() {
            return bytes;
        }
    }
    reply::error(&id, "expired", "No decision arrived.")
}

fn ask_running_to_show() -> std::io::Result<()> {
    use bridge_host::frame::{read_frame, write_frame};
    let endpoint = bridge_host::endpoint::desktop_endpoint()?;
    let mut stream = bridge_host::endpoint::connect(&endpoint.name)?;
    write_frame(&mut stream, br#"{"v":1,"id":"show","method":"show"}"#)?;
    let _ = read_frame(&mut stream)?;
    Ok(())
}

/// The request on screen: the head of the core's queue, with what the window
/// shows about it.
#[derive(Clone, Debug, PartialEq)]
pub struct BridgePrompt {
    pub id: String,
    pub origin: String,
    pub method: String,
    pub params: Value,
    pub detail: String,
    /// How many transactions a `signTransaction` asks to have signed.
    pub transactions: usize,
    /// Requests queued behind this one.
    pub waiting: usize,
}

impl BridgePrompt {
    pub fn head_of(queue: &BridgeQueue) -> Option<Self> {
        let request = queue.head()?;
        Some(Self {
            id: request.id.clone(),
            origin: request.origin.clone(),
            method: request.method.clone(),
            params: request.params.clone(),
            detail: detail(&request.method, &request.params),
            transactions: request.transaction_count(),
            waiting: queue.len() - 1,
        })
    }
}

/// Transaction `index` of a `signTransaction` request.
fn transaction_item(params: &Value, index: usize) -> Option<&Value> {
    params.get("transactions")?.as_array()?.get(index)
}

/// The account transaction `index` of a `signTransaction` request says it is
/// for. The site chose it, so it is shown and never relied on.
pub fn requested_account(params: &Value, index: usize) -> Option<Pubkey> {
    transaction_item(params, index)?
        .get("account")?
        .as_str()?
        .parse()
        .ok()
}

/// The accounts a `signTransaction` asks to sign, those that read as addresses.
fn requested_accounts(params: &Value) -> Vec<Pubkey> {
    (0..transaction_count(params))
        .filter_map(|index| requested_account(params, index))
        .collect()
}

/// The bytes of transaction `index`, when they read as a Solana transaction.
pub fn transaction_wire(params: &Value, index: usize) -> Option<Vec<u8>> {
    let text = transaction_item(params, index)?
        .get("transaction")?
        .as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?;
    wallet_core::parse_wire(&bytes).ok()?;
    Some(bytes)
}

fn detail(method: &str, params: &Value) -> String {
    match method {
        "connect" => "Choose which accounts this site may see.".into(),
        "signTransaction" => sign_detail(params, 0),
        other => other.into(),
    }
}

/// What transaction `index` of a request is, when it cannot be prepared.
pub fn sign_detail(params: &Value, index: usize) -> String {
    let Some(item) = transaction_item(params, index) else {
        return "No transaction was included.".into();
    };
    let Some(text) = item.get("transaction").and_then(Value::as_str) else {
        return "The transaction was not base64.".into();
    };
    match base64::engine::general_purpose::STANDARD.decode(text) {
        Ok(bytes) => match wallet_core::parse_wire(&bytes).map(|view| view.format) {
            Ok(wallet_core::TxFormat::V1) => format!("{} byte v1 transaction.", bytes.len()),
            Ok(wallet_core::TxFormat::V0) => format!("{} byte v0 transaction.", bytes.len()),
            Ok(wallet_core::TxFormat::Legacy) => {
                format!("{} byte legacy transaction.", bytes.len())
            }
            Err(_) => format!(
                "{} bytes that could not be read as a Solana transaction.",
                bytes.len()
            ),
        },
        Err(_) => "The transaction was not valid base64.".into(),
    }
}

fn transaction_count(params: &Value) -> usize {
    params
        .get("transactions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
