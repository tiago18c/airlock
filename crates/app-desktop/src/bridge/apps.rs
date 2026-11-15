//! Requests from other apps on the same phone, through Mobile Wallet Adapter.
//!
//! The phone's MWA library (`MwaBridge` in the Android app) holds the
//! session: the association, its encryption, and the auth tokens it issues.
//! Each request it hands over is read here into the frame the extension
//! would have sent, and goes through [`Bridge`]'s own checks, queue and
//! modals. What comes back is read into what MWA answers.
//!
//! Unlike a browser, Android does not vouch for who an app says it is. The
//! origin a request is filed under is the identity URI the app gave, which
//! any app can give. So the modal says the app only *says* it is that site,
//! and names the package Android reported where it did. A new authorization
//! is always asked, even from an origin with a grant: only an app holding
//! the auth token MWA issued may reconnect without asking.
//!
//! An authorized session also takes the intents prototype's methods
//! (`docs/intents.md`, "Airlock's prototype on Android"), each the bridge
//! message of the same name. `airlock_resolve` answers once the person has
//! decided, since MWA has no way for a wallet to speak first.

use std::collections::BTreeSet;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use app_core::bridge::{BridgeRequest, SiteCall, SiteConnect, SiteSign, SEND_ALL};
use app_core::PublicCore;
use base64::Engine;
use bridge_host::relay::Answer;
use bridge_protocol::Frame;
use serde::Deserialize;
use serde_json::{json, Value};
use solana_pubkey::Pubkey;
use wallet_core::intent::{Resolution, State};

use super::{wait_for_user, Bridge, Waiting, DECISION_TIMEOUT};

/// Who an app on the phone says it is, and what Android says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppIdentity {
    /// The name the app gave.
    #[serde(default)]
    pub name: Option<String>,
    /// The web address the app gave.
    #[serde(default)]
    pub uri: Option<String>,
    /// The package Android says started the session, when it says. Chrome
    /// opens a web page's session without one.
    #[serde(default)]
    pub package: Option<String>,
}

impl AppIdentity {
    /// The origin the app's requests and grants are filed under: the site it
    /// gave, or its package when it gave none.
    pub fn origin(&self) -> Option<String> {
        if let Some(origin) = self.uri.as_deref().and_then(web_origin) {
            return Some(origin);
        }
        let package = self.package.as_deref()?;
        let valid = !package.is_empty()
            && package
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_');
        valid.then(|| format!("android-app://{package}"))
    }
}

/// `https://host[:port]` of `uri`, or `None` when it is not a web address
/// with nothing odd in its authority.
fn web_origin(uri: &str) -> Option<String> {
    let uri = uri.trim();
    let (scheme, rest) = uri.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    let valid = !authority.is_empty()
        && authority
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':' | '[' | ']'));
    valid.then(|| format!("{scheme}://{authority}"))
}

/// What the phone's request is shown as, beside the site it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppClaim {
    pub name: Option<String>,
    pub package: Option<String>,
}

/// One request an app made, as `MwaBridge` hands it over. Keys are base58,
/// payloads base64.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "method",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AppCall {
    /// A new authorization. With `sign_in`, the Wallet Standard's
    /// `SolanaSignInInput` to sign as well.
    Authorize {
        #[serde(default)]
        sign_in: Option<Value>,
    },
    /// An app with a valid auth token asks to go on. MWA's library does not
    /// say which accounts the token holds; when given, they are checked too.
    Reauthorize {
        #[serde(default)]
        accounts: Vec<String>,
    },
    /// The app gave up its auth token.
    Deauthorize,
    SignTransactions {
        accounts: Vec<String>,
        chain: String,
        payloads: Vec<String>,
    },
    SignAndSend {
        accounts: Vec<String>,
        chain: String,
        payloads: Vec<String>,
        #[serde(default)]
        commitment: Option<String>,
        /// Each transaction waits for the one before it to reach the
        /// commitment.
        #[serde(default)]
        serial: bool,
    },
    SignMessages {
        addresses: Vec<String>,
        payloads: Vec<String>,
    },
    /// One of the intents prototype's methods, by its MWA name
    /// (`airlock_resolve`), with the params the page sent.
    Intent {
        name: String,
        #[serde(default)]
        params: Value,
    },
}

/// The bridge message each intent method of MWA is.
const INTENT_METHODS: [(&str, &str); 6] = [
    ("airlock_get_signing_profile", "getSigningProfile"),
    ("airlock_resolve", "resolve"),
    ("airlock_get_resolution", "getResolution"),
    ("airlock_provide_signatures", "provideSignatures"),
    ("airlock_cancel_resolution", "cancelResolution"),
    ("airlock_sign_message", "resolveMessage"),
];

/// The bridge message for MWA method `name`, when it is an intent method.
pub fn intent_method(name: &str) -> Option<&'static str> {
    INTENT_METHODS
        .iter()
        .find(|(mwa, _)| *mwa == name)
        .map(|(_, bridge)| *bridge)
}

/// Why a session that is not authorized is refused an intent method.
const NOT_AUTHORIZED: &str = "This session is not authorized. Call authorize first.";

/// An intent method's error, as the bridge gave it. MWA answers it as a
/// JSON-RPC error whose `data` holds the code and the requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentError {
    pub code: String,
    pub message: String,
    pub requirement: Option<String>,
}

impl IntentError {
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            requirement: None,
        }
    }
}

/// What MWA answers an app. Keys, payloads and signatures are base64.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAnswer {
    Authorized {
        accounts: Vec<(String, String)>,
        sign_in: Option<SignedIn>,
    },
    Reauthorized,
    Declined,
    /// The session has no authorization for this.
    NotAuthorized,
    TooManyPayloads,
    /// One signed payload for each sent: a transaction, or a message with
    /// its signature after it.
    Signed(Vec<String>),
    Sent(Vec<String>),
    /// What was sent, by signature, and an empty string for what was not.
    NotSubmitted(Vec<String>),
    /// Which payloads could be read.
    InvalidPayloads(Vec<bool>),
    Done,
    /// An intent method's `result`, as the bridge gave it, or its error.
    Intent(Result<Value, IntentError>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub account: String,
    pub signed_message: String,
    pub signature: String,
}

impl AppAnswer {
    /// The answer as `MwaBridge` reads it.
    pub fn to_json(&self) -> String {
        let value = match self {
            Self::Authorized { accounts, sign_in } => {
                let accounts: Vec<Value> = accounts
                    .iter()
                    .map(|(key, label)| json!({ "publicKey": key, "label": label }))
                    .collect();
                let mut value = json!({ "kind": "authorized", "accounts": accounts });
                if let Some(signed) = sign_in {
                    value["signIn"] = json!({
                        "publicKey": signed.account,
                        "signedMessage": signed.signed_message,
                        "signature": signed.signature,
                    });
                }
                value
            }
            Self::Reauthorized => json!({ "kind": "reauthorized" }),
            Self::Declined => json!({ "kind": "declined" }),
            Self::NotAuthorized => json!({ "kind": "notAuthorized" }),
            Self::TooManyPayloads => json!({ "kind": "tooMany" }),
            Self::Signed(payloads) => json!({ "kind": "signed", "payloads": payloads }),
            Self::Sent(signatures) => json!({ "kind": "sent", "signatures": signatures }),
            Self::NotSubmitted(signatures) => {
                json!({ "kind": "notSubmitted", "signatures": signatures })
            }
            Self::InvalidPayloads(valid) => json!({ "kind": "invalid", "valid": valid }),
            Self::Done => json!({ "kind": "done" }),
            Self::Intent(Ok(result)) => json!({ "kind": "result", "result": result }),
            Self::Intent(Err(error)) => {
                let mut value =
                    json!({ "kind": "error", "code": error.code, "message": error.message });
                if let Some(requirement) = &error.requirement {
                    value["requirement"] = json!(requirement);
                }
                value
            }
        };
        value.to_string()
    }
}

/// One MWA session: an app, while it stays connected.
pub struct AppSession {
    bridge: Bridge,
    identity: AppIdentity,
    origin: Option<String>,
    waiting: Waiting,
    /// Every request it put to the bridge and has no reply for yet, whether
    /// or not it reached the queue.
    asked: Mutex<BTreeSet<String>>,
    closed: AtomicBool,
    /// The last `authorize` or `reauthorize` of it was granted, and no
    /// `deauthorize` came after. Only then does it take intent methods.
    authorized: AtomicBool,
    /// Its number in [`super::Shared::apps`].
    key: u64,
}

impl Bridge {
    /// A bridge with no socket, for requests from apps on this phone.
    pub fn for_apps() -> Self {
        Self::new()
    }

    /// Who the app of the request `id` says it is, for a request from an app
    /// on this phone.
    pub fn app_claim(&self, id: &str) -> Option<AppClaim> {
        self.shared
            .claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    /// Start a session for an app. Its site counts as open until the session
    /// drops.
    pub fn app_session(&self, identity: AppIdentity) -> AppSession {
        let origin = identity.origin();
        let key = self
            .shared
            .next_peer
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(origin) = &origin {
            self.shared
                .apps
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key, origin.clone());
            self.shared.report_pages();
        }
        tracing::info!(
            target: "airlock::bridge",
            origin = ?origin,
            package = ?identity.package,
            "app session"
        );
        AppSession {
            bridge: self.clone(),
            identity,
            origin,
            waiting: Waiting::default(),
            asked: Mutex::default(),
            closed: AtomicBool::new(false),
            authorized: AtomicBool::new(false),
            key,
        }
    }

    /// Forget who the app of request `id` says it is.
    fn drop_claim(&self, id: &str) {
        self.shared
            .claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }
}

impl AppSession {
    /// The site this session's requests are filed under.
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// Answer one request. It may wait for the user, up to the same ten
    /// minutes a browser request does.
    pub fn answer(&self, call: AppCall) -> AppAnswer {
        let authorizing = match &call {
            AppCall::Intent { name, params } => {
                return AppAnswer::Intent(self.intent(name, params.clone()));
            }
            AppCall::Authorize { .. } | AppCall::Reauthorize { .. } => Some(true),
            AppCall::Deauthorize => Some(false),
            _ => None,
        };
        // As MWA's library does, a new authorization ends the one before it
        // until it is granted.
        if authorizing.is_some() {
            self.authorized.store(false, Ordering::SeqCst);
        }
        let answer = self.answer_mwa(call);
        if authorizing == Some(true)
            && matches!(
                answer,
                AppAnswer::Authorized { .. } | AppAnswer::Reauthorized
            )
        {
            self.authorized.store(true, Ordering::SeqCst);
        }
        answer
    }

    /// Answer one of MWA's own requests.
    fn answer_mwa(&self, call: AppCall) -> AppAnswer {
        let Some(origin) = self.origin.clone() else {
            return AppAnswer::Declined;
        };
        let Some(core) = self.bridge.shared.core.get().cloned() else {
            return AppAnswer::Declined;
        };
        match call {
            AppCall::Authorize { sign_in } => self.authorize(&core, origin, sign_in),
            AppCall::Reauthorize { accounts } => {
                // MWA checked the token. The site must still hold a grant it
                // did not end, or the app has to ask again. Each signature
                // is checked against the grant as it is then.
                if core.site_sign(&origin, &keys(&accounts)) == SiteSign::Allowed {
                    AppAnswer::Reauthorized
                } else {
                    AppAnswer::Declined
                }
            }
            AppCall::Deauthorize => {
                core.site_disconnected(&origin);
                AppAnswer::Done
            }
            AppCall::SignTransactions {
                accounts,
                chain,
                payloads,
            } => {
                let Some(params) = transactions_params(&accounts, &chain, &payloads, None) else {
                    return AppAnswer::InvalidPayloads(readable(&payloads));
                };
                let reply = self.ask("signTransaction", params);
                match reply.result() {
                    Ok(result) => AppAnswer::Signed(strings(&result["signedTransactions"])),
                    Err(code) => refused(&code),
                }
            }
            AppCall::SignAndSend {
                accounts,
                chain,
                payloads,
                commitment,
                serial,
            } => {
                let Some(mut params) =
                    transactions_params(&accounts, &chain, &payloads, commitment.as_deref())
                else {
                    return AppAnswer::InvalidPayloads(readable(&payloads));
                };
                params["mode"] = json!(if serial { "serial" } else { "parallel" });
                let reply = self.ask(SEND_ALL, params);
                match reply.result() {
                    Ok(result) => sent(&result["results"]),
                    Err(code) => refused(&code),
                }
            }
            AppCall::SignMessages {
                addresses,
                payloads,
            } => {
                let ([address], [payload]) = (addresses.as_slice(), payloads.as_slice()) else {
                    return AppAnswer::TooManyPayloads;
                };
                let reply = self.ask(
                    "signMessage",
                    json!({ "account": address, "message": payload }),
                );
                match reply.result() {
                    Ok(result) => {
                        let (Some(signed), Some(signature)) =
                            (bytes(&result["signedMessage"]), bytes(&result["signature"]))
                        else {
                            return AppAnswer::Declined;
                        };
                        // MWA's signed message carries its signature after it.
                        AppAnswer::Signed(vec![b64(&[signed, signature].concat())])
                    }
                    Err(code) => refused(&code),
                }
            }
            // Answered before this, by `answer`.
            AppCall::Intent { .. } => AppAnswer::Declined,
        }
    }

    /// An intent method `name`: its bridge message, put through the bridge
    /// as the extension's frame would be. Its `result` or error is the
    /// bridge's, unchanged, except that a `resolve`, and a message signed as
    /// a memo, answer once the person decided.
    fn intent(&self, name: &str, params: Value) -> Result<Value, IntentError> {
        let Some(method) = intent_method(name) else {
            return Err(IntentError::new(
                "not_implemented",
                "Airlock does not know this request.",
            ));
        };
        if self.origin.is_none() || !self.authorized.load(Ordering::SeqCst) {
            return Err(IntentError::new("rejected", NOT_AUTHORIZED));
        }
        let Some(core) = self.bridge.shared.core.get().cloned() else {
            return Err(IntentError::new("busy", "Airlock is starting."));
        };
        match method {
            "resolve" | "resolveMessage" => self.resolve(&core, method, params),
            _ => self.ask(method, params).outcome(),
        }
    }

    /// A `resolve` or a `resolveMessage`. The bridge answers a resolution in
    /// `awaiting-approval` at once, and the extension waits for its push;
    /// here the answer waits until the resolution leaves that state, as the
    /// Wallet Standard `resolve` settles, and leaves `sending` too (see
    /// [`AppSession::decision`]). Declined, expired and cancelled are
    /// errors, as the extension makes them.
    ///
    /// The session closing meanwhile does not end it, as a page closing does
    /// not on desktop: the person may still decide, and the site asks for
    /// the resolution later. A message signed with a key waits in the bridge
    /// as `signMessage` does, and ends with the session as that does.
    fn resolve(
        &self,
        core: &PublicCore,
        method: &str,
        params: Value,
    ) -> Result<Value, IntentError> {
        let id = request_id();
        if !self.claim(&id) {
            return Err(gone());
        }
        let bytes = finish(
            self.bridge
                .answer(self.frame(&id, method, params), &self.waiting),
        );
        // Out of what the session's closing expires, but still this app's in
        // the modal while the request waits there.
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        let mut result = match Reply::read(&bytes).outcome() {
            Ok(result) => result,
            Err(error) => {
                self.unclaim(&id);
                return Err(error);
            }
        };
        let Some(resolution) = result
            .get("resolution")
            .and_then(|resolution| resolution.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            // A message signed with a key: the person decided already.
            self.unclaim(&id);
            return Ok(result);
        };
        let Some(decided) = self.decision(core, &id, &resolution) else {
            return Err(gone());
        };
        match decided.state {
            State::Declined => Err(IntentError::new(
                "rejected",
                "The request was declined in Airlock.",
            )),
            State::Expired => Err(IntentError::new(
                "expired",
                "Nobody decided in time, so the request expired.",
            )),
            State::Cancelled => Err(IntentError::new("cancelled", "The request was cancelled.")),
            _ => {
                result["resolution"] = decided.public();
                Ok(result)
            }
        }
    }

    /// Wait until resolution `resolution` of this site leaves
    /// `awaiting-approval`, and `sending` after it, and forget who asked
    /// request `id` then. The wait goes on in the core after the session
    /// closes, so the modal still says who the app says it is; this answer
    /// then stops waiting. `None` once the session closed, or the
    /// resolution is gone.
    ///
    /// Why through `sending`: what the person approved here is sent by the
    /// screen (`send_for_site`), and a phone stops the screen's tasks while
    /// Airlock is behind. The answer moves Airlock behind, so it waits
    /// until the send is out, as a `sign_and_send_transactions` does.
    fn decision(&self, core: &PublicCore, id: &str, resolution: &str) -> Option<Resolution> {
        let origin = self.origin.clone()?;
        let (tx, rx) = mpsc::channel();
        let watching = core.clone();
        let bridge = self.bridge.clone();
        let (id, resolution) = (id.to_owned(), resolution.to_owned());
        core.ports().spawner.spawn(Box::pin(async move {
            let mut changes = watching.slices().resolutions.subscribe();
            let decided = loop {
                let _ = changes.borrow_and_update();
                match watching.resolution(&origin, &resolution) {
                    Some(now) if matches!(now.state, State::AwaitingApproval | State::Sending) => {}
                    other => break other,
                }
                if changes.changed().await.is_err() {
                    break None;
                }
            };
            bridge.drop_claim(&id);
            let _ = tx.send(decided);
        }));
        loop {
            match rx.recv_timeout(CLOSED_CHECK) {
                Ok(decided) => return decided,
                Err(RecvTimeoutError::Timeout) if self.closed.load(Ordering::SeqCst) => {
                    return None
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// A new authorization, with a sign-in or without. It is always the
    /// user's to decide, whatever the site was given before.
    fn authorize(
        &self,
        core: &app_core::PublicCore,
        origin: String,
        sign_in: Option<Value>,
    ) -> AppAnswer {
        if core.site_connect(&origin, true) == SiteConnect::Blocked {
            return AppAnswer::Declined;
        }
        let reply = match sign_in {
            Some(input) => self.ask("signIn", input),
            None => {
                let id = request_id();
                if !self.claim(&id) {
                    return AppAnswer::Declined;
                }
                let request = BridgeRequest {
                    id: id.clone(),
                    origin,
                    call: SiteCall::Connect,
                };
                let reply = finish(wait_for_user(
                    core,
                    request,
                    DECISION_TIMEOUT,
                    &self.waiting,
                ));
                self.unclaim(&id);
                Reply::read(&reply)
            }
        };
        match reply.result() {
            Ok(result) => {
                let accounts = site_accounts(&result["accounts"]);
                if accounts.is_empty() {
                    return AppAnswer::Declined;
                }
                let sign_in = result
                    .get("account")
                    .and_then(Value::as_str)
                    .map(|account| SignedIn {
                        account: account.to_owned(),
                        signed_message: text(&result["signedMessage"]),
                        signature: text(&result["signature"]),
                    });
                AppAnswer::Authorized { accounts, sign_in }
            }
            Err(code) => refused(&code),
        }
    }

    /// Put a request through the bridge as the extension's frame, and wait
    /// for its reply.
    fn ask(&self, method: &str, params: Value) -> Reply {
        let id = request_id();
        if !self.claim(&id) {
            return Reply(Value::Null);
        }
        let bytes = finish(
            self.bridge
                .answer(self.frame(&id, method, params), &self.waiting),
        );
        self.unclaim(&id);
        Reply::read(&bytes)
    }

    /// The extension's frame for request `id`, from this session's site.
    fn frame(&self, id: &str, method: &str, params: Value) -> Frame {
        Frame {
            version: Some(1),
            id: id.to_owned(),
            origin: self.origin.clone().unwrap_or_default(),
            expires_at: None,
            method: method.to_owned(),
            params,
        }
    }

    /// Note request `id` as this session's, and who it comes from. False
    /// once the session closed: nothing more is asked for it.
    fn claim(&self, id: &str) -> bool {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned());
        if self.closed.load(Ordering::SeqCst) {
            self.unclaim(id);
            return false;
        }
        let claim = AppClaim {
            name: self.identity.name.clone(),
            package: self.identity.package.clone(),
        };
        self.bridge
            .shared
            .claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), claim);
        true
    }

    fn unclaim(&self, id: &str) {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        self.bridge
            .shared
            .claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }
}

impl AppSession {
    /// The app went away: nobody is left to hear what the user decides, so
    /// nobody is asked, and its site is no longer open. A request still
    /// being answered returns as declined. A `resolve` waiting for the
    /// person stays, as it does when a page closes on desktop: its
    /// resolution is what the site asks for later.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let shared = &self.bridge.shared;
        if let Some(core) = shared.core.get() {
            // What it asked, including what is reaching the queue only now.
            let asked = self
                .asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            for id in asked {
                core.bridge_expire(&id);
            }
        }
        shared
            .apps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
        shared.report_pages();
    }
}

impl Drop for AppSession {
    fn drop(&mut self) {
        self.close();
    }
}

/// The reply's bytes, once the user decided if they had to.
fn finish(answer: Answer) -> Vec<u8> {
    match answer {
        Answer::Now(bytes) => bytes,
        Answer::Later(wait) => wait(),
    }
}

/// A reply of the bridge, read back.
struct Reply(Value);

impl Reply {
    fn read(bytes: &[u8]) -> Self {
        Self(serde_json::from_slice(bytes).unwrap_or(Value::Null))
    }

    /// Its result, or its error code.
    fn result(self) -> Result<Value, String> {
        let Self(mut value) = self;
        if let Some(result) = value.get_mut("result") {
            return Ok(result.take());
        }
        Err(value["error"]["code"]
            .as_str()
            .unwrap_or("internal")
            .to_owned())
    }

    /// Its result, or its error whole.
    fn outcome(self) -> Result<Value, IntentError> {
        let Self(mut value) = self;
        if let Some(result) = value.get_mut("result") {
            return Ok(result.take());
        }
        let error = &value["error"];
        let text = |key: &str| error.get(key).and_then(Value::as_str).map(str::to_owned);
        Err(IntentError {
            code: text("code").unwrap_or_else(|| "internal".to_owned()),
            message: text("message")
                .unwrap_or_else(|| "Airlock could not answer this request.".to_owned()),
            requirement: text("requirement"),
        })
    }
}

/// How often an answer that waits on a resolution looks whether its
/// session closed.
const CLOSED_CHECK: Duration = Duration::from_millis(250);

/// The answer to a request whose session closed: nobody hears it.
fn gone() -> IntentError {
    IntentError::new("rejected", "The app's session ended.")
}

/// What MWA answers for a bridge error.
fn refused(code: &str) -> AppAnswer {
    match code {
        "not_implemented" | "too_many_requests" => AppAnswer::TooManyPayloads,
        _ => AppAnswer::Declined,
    }
}

/// The frame's `transactions`: each payload, with the account it is asked of.
/// `None` when a payload is not base64.
fn transactions_params(
    accounts: &[String],
    chain: &str,
    payloads: &[String],
    commitment: Option<&str>,
) -> Option<Value> {
    let granted = keys(accounts);
    let mut transactions = Vec::new();
    for payload in payloads {
        let wire = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()?;
        let account = signer_of(&wire, &granted)
            .or_else(|| granted.first().copied())
            .map(|key| key.to_string())
            .unwrap_or_default();
        let mut item = json!({ "account": account, "transaction": payload, "chain": chain });
        if let Some(commitment) = commitment {
            item["options"] = json!({ "commitment": commitment });
        }
        transactions.push(item);
    }
    Some(json!({ "transactions": transactions }))
}

/// The first of `granted` that `wire` still needs a signature from. MWA names
/// no account per transaction, as the Wallet Standard does.
fn signer_of(wire: &[u8], granted: &[Pubkey]) -> Option<Pubkey> {
    let view = wallet_core::parse_wire(wire).ok()?;
    (0..view.num_required_signatures)
        .filter(|slot| !view.slot_valid(*slot))
        .map(|slot| view.static_keys[slot])
        .find(|key| granted.contains(key))
}

fn readable(payloads: &[String]) -> Vec<bool> {
    payloads
        .iter()
        .map(|payload| {
            base64::engine::general_purpose::STANDARD
                .decode(payload)
                .is_ok()
        })
        .collect()
}

/// A `signAndSendAllTransactions` result, as MWA answers it: every
/// signature, or what went out when something did not.
fn sent(results: &Value) -> AppAnswer {
    let results = results.as_array().map_or(&[][..], Vec::as_slice);
    let signatures: Vec<Option<String>> = results
        .iter()
        .map(|one| {
            one.get("signature")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    if signatures.is_empty() {
        return AppAnswer::Declined;
    }
    if signatures.iter().all(Option::is_some) {
        return AppAnswer::Sent(signatures.into_iter().flatten().collect());
    }
    AppAnswer::NotSubmitted(
        signatures
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect(),
    )
}

/// The base64 keys and labels of a reply's accounts.
fn site_accounts(accounts: &Value) -> Vec<(String, String)> {
    accounts
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|account| {
            let key = account.get("publicKey")?.as_str()?.to_owned();
            Some((key, text(&account["label"])))
        })
        .collect()
}

fn keys(accounts: &[String]) -> Vec<Pubkey> {
    accounts
        .iter()
        .filter_map(|account| account.parse().ok())
        .collect()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn bytes(value: &Value) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(value.as_str()?)
        .ok()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A request id of 16 hex digits, unguessable enough that its code tells
/// requests apart.
fn request_id() -> String {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:016x}", hasher.finish())
}

/// The origins of the sessions open now, for [`super::Shared::report_pages`].
pub(super) fn open_origins(apps: &std::collections::BTreeMap<u64, String>) -> BTreeSet<String> {
    apps.values().cloned().collect()
}

#[cfg(test)]
#[path = "apps_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "apps_intents_tests.rs"]
mod intents_tests;
