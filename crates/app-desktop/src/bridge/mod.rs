//! Desktop side of the browser bridge: the socket the extension's native host
//! connects to. A request the user must decide goes into the core's queue,
//! which the window reads and answers (`Core::bridge_answer`); this module
//! only waits for that answer and writes it back.
//!
//! A second launch that cannot bind the socket sends `show` and exits. What
//! keeps two processes off one data folder is the folder's own lock
//! (`app_core::storage::claim_data_dir`), which holds even when the socket
//! cannot be named.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use app_core::bridge::{
    BridgeQueue, BridgeRefused, BridgeRequest, SiteConnect, SiteSign, SEND, SEND_ALL,
};
use app_core::PublicCore;
use base64::Engine;
use bridge_host::relay::{Answer, Connection, FrameSender};
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
    /// The connections open now, by a number of their own.
    peers: Mutex<BTreeMap<u64, PeerLink>>,
    next_peer: AtomicU64,
}

/// What the app knows about one open connection.
struct PeerLink {
    sender: FrameSender,
    /// The origins with a page open in this browser.
    pages: BTreeSet<String>,
}

/// The requests from one connection that went into the core's queue and have
/// not been replied to.
type Waiting = Arc<Mutex<BTreeSet<String>>>;

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
                peers: Mutex::new(BTreeMap::new()),
                next_peer: AtomicU64::new(0),
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
                    let accept = move |sender: FrameSender| -> Arc<dyn Connection> {
                        Arc::new(Peer::new(serving.clone(), Some(sender)))
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
        let spawner = core.ports().spawner.clone();
        // A change of wallet may change what a site should act as.
        let syncing = core.clone();
        let mut wallets = core.slices().wallets.subscribe();
        spawner.spawn(Box::pin(async move {
            while wallets.changed().await.is_ok() {
                syncing.sites_sync();
            }
        }));
        // Tell each site when what it holds changes. One that just got its
        // accounts in a reply has nothing to learn.
        let telling = core.clone();
        let shared = self.shared.clone();
        let mut sites = core.slices().sites.subscribe();
        spawner.spawn(Box::pin(async move {
            let mut told = sites.borrow_and_update().holding.clone();
            while sites.changed().await.is_ok() {
                let now = sites.borrow_and_update().holding.clone();
                for frame in changes(&told, &now, &telling) {
                    shared.push(&frame);
                }
                told = now;
            }
        }));
        let _ = self.shared.core.set(core);
        self.shared.report_pages();
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

    /// What becomes of one frame from the native host. A request that goes to
    /// the user joins `waiting`.
    fn answer(&self, message: &[u8], waiting: &Waiting) -> Answer {
        let value: Value = match serde_json::from_slice(message) {
            Ok(value) => value,
            Err(_) => {
                return Answer::Now(reply::error("", "bad_request", "The request is not JSON."))
            }
        };
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if value.get("v").and_then(Value::as_u64) != Some(1) {
            return Answer::Now(reply::error(
                &id,
                "bad_request",
                "Unsupported bridge version.",
            ));
        }
        if let Some(expires) = value.get("expiresAt").and_then(Value::as_u64) {
            if expires < now_secs() {
                return Answer::Now(reply::error(&id, "expired", "The request expired."));
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
            return Answer::Now(reply::result(&id, json!({ "accounts": [] })));
        }
        if waits_for_user(method) && core.is_none() {
            return Answer::Now(reply::error(&id, "busy", "Airlock is starting."));
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
            ("status", _) => Answer::Now(reply::result(
                &id,
                json!({ "ready": true, "role": "desktop" }),
            )),
            // The connection took these in already: it knows which browser
            // this is, and which requests are its own.
            ("pages" | "cancel", _) => Answer::Now(reply::result(&id, json!({}))),
            ("show", _) => {
                self.shared.show.send_modify(|n| *n = n.wrapping_add(1));
                Answer::Now(reply::result(&id, json!({})))
            }
            ("disconnect", core) => {
                if let Some(core) = core {
                    core.site_disconnected(&origin);
                }
                Answer::Now(reply::result(&id, json!({})))
            }
            (SEND | SEND_ALL, _) if !every_chain_named(&params) => Answer::Now(reply::error(
                &id,
                "bad_request",
                "A transaction names no network.",
            )),
            (TRANSACTION_METHODS!(), _) if transaction_count(&params) == 0 => Answer::Now(
                reply::error(&id, "bad_request", "No transaction was included."),
            ),
            (TRANSACTION_METHODS!(), _) if transaction_count(&params) > MAX_TRANSACTIONS => {
                Answer::Now(reply::error(
                    &id,
                    "not_implemented",
                    &format!("Airlock signs at most {MAX_TRANSACTIONS} transactions per request."),
                ))
            }
            ("connect", Some(core)) => match core.site_connect(&origin, silent) {
                SiteConnect::Allowed(accounts) => Answer::Now(reply::connected(&id, &accounts)),
                _ if silent => Answer::Now(reply::result(&id, json!({ "accounts": [] }))),
                SiteConnect::Blocked => Answer::Now(blocked(&id)),
                SiteConnect::Undecided => wait_for_user(
                    core,
                    BridgeRequest {
                        id,
                        origin,
                        method: method.to_owned(),
                        params,
                    },
                    DECISION_TIMEOUT,
                    waiting,
                ),
            },
            ("signMessage", Some(core)) => {
                let answer = check_message_request(core, &origin, &params);
                match answer {
                    Ok(()) => wait_for_user(
                        core,
                        BridgeRequest {
                            id,
                            origin,
                            method: method.to_owned(),
                            params,
                        },
                        DECISION_TIMEOUT,
                        waiting,
                    ),
                    Err(message) => Answer::Now(reply::error(&id, "rejected", &message)),
                }
            }
            ("signIn", Some(core)) => {
                let answer = check_sign_in_request(core, &origin, &params);
                match answer {
                    Ok(()) => wait_for_user(
                        core,
                        BridgeRequest {
                            id,
                            origin,
                            method: method.to_owned(),
                            params,
                        },
                        DECISION_TIMEOUT,
                        waiting,
                    ),
                    Err(message) => Answer::Now(reply::error(&id, "rejected", &message)),
                }
            }
            (TRANSACTION_METHODS!(), Some(core)) => {
                let accounts = requested_accounts(&params);
                match core.site_sign(&origin, &accounts) {
                    SiteSign::Blocked => Answer::Now(blocked(&id)),
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
                            waiting,
                        )
                    }
                    // A transaction that names no account is not for one the
                    // site was given.
                    _ => Answer::Now(reply::error(&id, "rejected", NOT_CONNECTED)),
                }
            }
            _ => Answer::Now(reply::error(
                &id,
                "not_implemented",
                "Airlock does not know this request.",
            )),
        }
    }
}

/// The requests that hand over transactions.
macro_rules! TRANSACTION_METHODS {
    () => {
        "signTransaction" | SEND | SEND_ALL
    };
}
use TRANSACTION_METHODS;

const NOT_CONNECTED: &str = "This account is not connected to the site.";

const BLOCKED: &str = "This site is blocked in Airlock.";

/// The reply to a site the user blocked.
fn blocked(id: &str) -> Vec<u8> {
    reply::error(id, "rejected", BLOCKED)
}

/// Why a `signMessage` cannot even be shown to the user.
fn check_message_request(core: &PublicCore, origin: &str, params: &Value) -> Result<(), String> {
    let account = message_account(params).ok_or(NOT_CONNECTED)?;
    match core.site_sign(origin, &[account]) {
        SiteSign::Blocked => return Err(BLOCKED.into()),
        SiteSign::NotConnected => return Err(NOT_CONNECTED.into()),
        SiteSign::Allowed => {}
    }
    if !holds_key(core, &account) {
        return Err("No key on this device signs as this account.".into());
    }
    let bytes = message_bytes(params).ok_or("The message was not valid base64.")?;
    wallet_core::site_message::check_message(&bytes, origin).map_err(|err| err.to_string())
}

/// Why a `signIn` cannot even be shown to the user.
fn check_sign_in_request(core: &PublicCore, origin: &str, params: &Value) -> Result<(), String> {
    if core.site_sign(origin, &[]) == SiteSign::Blocked {
        return Err(BLOCKED.into());
    }
    if params.get("useOffchainMessage").is_some() {
        return Err("Airlock does not sign in with offchain messages yet.".into());
    }
    let sign_in = sign_in_request(params).ok_or("The sign-in request could not be read.")?;
    sign_in.check(origin).map_err(|err| err.to_string())?;
    if let Some(asked) = &sign_in.address {
        let held = asked
            .parse()
            .ok()
            .is_some_and(|account| holds_key(core, &account));
        if !held {
            return Err("No key on this device signs as the account this site asked for.".into());
        }
    }
    Ok(())
}

/// Whether a key of the vault signs as `account`, locked or not.
fn holds_key(core: &PublicCore, account: &Pubkey) -> bool {
    core.slices()
        .wallets
        .get()
        .local_signer_pubkeys
        .contains(account)
}

/// The frames that tell sites what changed from `told` to `now`.
fn changes(
    told: &BTreeMap<String, Vec<Pubkey>>,
    now: &BTreeMap<String, Vec<Pubkey>>,
    core: &PublicCore,
) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    for (origin, held) in told {
        match now.get(origin) {
            Some(accounts) if accounts != held => frames.push(reply::accounts_changed(
                origin,
                &core.site_labels(accounts.clone()),
            )),
            None => frames.push(reply::accounts_changed(origin, &[])),
            _ => {}
        }
    }
    frames
}

impl Shared {
    /// Send `frame` to every browser. The extension passes it only to pages
    /// of the origin it names.
    fn push(&self, frame: &[u8]) {
        // Written with the list let go: a browser that stopped reading can hold
        // up a write, and must not hold up every other connection with it.
        let senders: Vec<FrameSender> = self
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|peer| peer.sender.clone())
            .collect();
        for sender in senders {
            let _ = sender.send(frame);
        }
    }

    /// Tell the core which origins have a page open, in any browser.
    fn report_pages(&self) {
        let Some(core) = self.core.get() else {
            return;
        };
        let pages: BTreeSet<String> = self
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .flat_map(|peer| peer.pages.iter().cloned())
            .collect();
        core.site_pages(pages);
    }
}

/// One connection to the socket. It counts as an extension once it sends
/// anything but `show`, which is a second launch knocking, and stops counting
/// when it closes.
struct Peer {
    bridge: Bridge,
    id: u64,
    counted: AtomicBool,
    waiting: Waiting,
}

impl Peer {
    fn new(bridge: Bridge, sender: Option<FrameSender>) -> Self {
        let id = bridge.shared.next_peer.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = sender {
            let link = PeerLink {
                sender,
                pages: BTreeSet::new(),
            };
            bridge
                .shared
                .peers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id, link);
        }
        Self {
            bridge,
            id,
            counted: AtomicBool::new(false),
            waiting: Waiting::default(),
        }
    }

    /// The extension's word that nobody is waiting on one of its requests any
    /// more. Another connection's request is not its to end.
    fn cancel(&self, message: &[u8]) {
        let value = serde_json::from_slice::<Value>(message).unwrap_or(Value::Null);
        let Some(id) = value
            .get("params")
            .and_then(|params| params.get("id"))
            .and_then(Value::as_str)
        else {
            return;
        };
        let ours = self
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(id);
        if let (true, Some(core)) = (ours, self.bridge.shared.core.get()) {
            core.bridge_expire(id);
        }
    }

    /// The origins with a page open in this browser, as the extension lists them.
    fn set_pages(&self, message: &[u8]) {
        let value = serde_json::from_slice::<Value>(message).unwrap_or(Value::Null);
        let pages: BTreeSet<String> = value
            .get("params")
            .and_then(|params| params.get("origins"))
            .and_then(Value::as_array)
            .map(|origins| {
                origins
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let shared = &self.bridge.shared;
        if let Some(peer) = shared
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&self.id)
        {
            peer.pages = pages;
        }
        shared.report_pages();
    }
}

impl Connection for Peer {
    fn answer(&self, message: &[u8]) -> Answer {
        if !is_show(message) && !self.counted.swap(true, Ordering::SeqCst) {
            self.bridge.shared.extension.send_modify(|n| *n += 1);
        }
        match method_of(message).as_deref() {
            Some("pages") => self.set_pages(message),
            Some("cancel") => self.cancel(message),
            _ => {}
        }
        self.bridge.answer(message, &self.waiting)
    }

    fn closed(&self) {
        let shared = &self.bridge.shared;
        // Nobody is left to hear what the user decides, so the user is not
        // asked. A send already approved left the queue, and goes on.
        if let Some(core) = shared.core.get() {
            let waiting =
                std::mem::take(&mut *self.waiting.lock().unwrap_or_else(PoisonError::into_inner));
            for id in waiting {
                core.bridge_expire(&id);
            }
        }
        if self.counted.swap(false, Ordering::SeqCst) {
            shared.extension.send_modify(|n| *n = n.saturating_sub(1));
        }
        shared
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
        shared.report_pages();
    }
}

fn is_show(message: &[u8]) -> bool {
    method_of(message).as_deref() == Some("show")
}

fn method_of(message: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(message)
        .ok()?
        .get("method")?
        .as_str()
        .map(str::to_owned)
}

/// Whether `method` goes to the user, through the core's queue. The rest,
/// `show` among them, is answered here even before the core opens.
fn waits_for_user(method: &str) -> bool {
    matches!(
        method,
        "connect" | "signTransaction" | SEND | SEND_ALL | "signMessage" | "signIn"
    )
}

/// Queue `request` in the core, with the wait for the window's answer, at most
/// `timeout`, as the way to its reply. The queue's limits are what limit the
/// threads these waits get.
fn wait_for_user(
    core: &PublicCore,
    request: BridgeRequest,
    timeout: Duration,
    waiting: &Waiting,
) -> Answer {
    let id = request.id.clone();
    tracing::debug!(
        target: "airlock::bridge",
        code = %request.code(),
        method = %request.method,
        origin = %request.origin,
        "request queued"
    );
    let (tx, rx) = mpsc::channel();
    let pushed = core.bridge_push(
        request,
        Box::new(move |bytes| {
            let _ = tx.send(bytes);
        }),
    );
    match pushed {
        Ok(()) => {}
        Err(BridgeRefused::Full) => {
            return Answer::Now(reply::error(
                &id,
                "too_many_requests",
                "Too many requests are waiting in Airlock.",
            ))
        }
        Err(BridgeRefused::SameId) => {
            return Answer::Now(reply::error(
                &id,
                "bad_request",
                "A request with this id is already waiting.",
            ))
        }
    }
    waiting
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id.clone());
    let core = core.clone();
    let waiting = waiting.clone();
    Answer::Later(Box::new(move || {
        let bytes = wait_for_decision(&core, &id, &rx, timeout);
        waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        bytes
    }))
}

/// The reply to request `id` once its answer arrives, it is dropped, or
/// `timeout` passes with nobody having decided.
fn wait_for_decision(
    core: &PublicCore,
    id: &str,
    rx: &mpsc::Receiver<Vec<u8>>,
    timeout: Duration,
) -> Vec<u8> {
    if let Ok(bytes) = rx.recv_timeout(timeout) {
        return bytes;
    }
    // Nobody decided, unless it left the queue just now, or a send was
    // approved and is going out. Then the answer is still to come, however
    // long the send takes: a site told "expired" could send it all again.
    if !core.bridge_expire(id) {
        if let Ok(bytes) = rx.recv() {
            return bytes;
        }
    }
    reply::error(id, "expired", "No decision arrived.")
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
    /// The app sends what this asks to have signed.
    pub sends: bool,
    /// The networks its transactions name.
    pub chains: Vec<String>,
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
            sends: request.sends(),
            chains: request.chains(),
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

/// The account a `signMessage` names. The site chose it.
pub fn message_account(params: &Value) -> Option<Pubkey> {
    params.get("account")?.as_str()?.parse().ok()
}

/// The bytes a `signMessage` asks to have signed.
pub fn message_bytes(params: &Value) -> Option<Vec<u8>> {
    let text = params.get("message")?.as_str()?;
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

/// What a `signIn` asks for. It may ask for nothing at all.
pub fn sign_in_request(params: &Value) -> Option<wallet_core::site_message::SignIn> {
    if params.is_null() {
        return Some(Default::default());
    }
    serde_json::from_value(params.clone()).ok()
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
        TRANSACTION_METHODS!() => sign_detail(params, 0),
        "signMessage" => "Asks you to sign a message.".into(),
        "signIn" => "Asks you to sign in.".into(),
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

/// Whether every transaction of a send says which network it is for, as the
/// standard requires.
fn every_chain_named(params: &Value) -> bool {
    params
        .get("transactions")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .all(|item| item.get("chain").is_some_and(Value::is_string))
        })
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
