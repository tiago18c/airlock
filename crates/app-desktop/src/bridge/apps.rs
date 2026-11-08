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

use std::collections::BTreeSet;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use app_core::bridge::{BridgeRequest, SiteCall, SiteConnect, SiteSign, SEND_ALL};
use base64::Engine;
use bridge_host::relay::Answer;
use bridge_protocol::Frame;
use serde::Deserialize;
use serde_json::{json, Value};
use solana_pubkey::Pubkey;

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
            key,
        }
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
        let frame = Frame {
            version: Some(1),
            id: id.clone(),
            origin: self.origin.clone().unwrap_or_default(),
            expires_at: None,
            method: method.to_owned(),
            params,
        };
        if !self.claim(&id) {
            return Reply(Value::Null);
        }
        let bytes = finish(self.bridge.answer(frame, &self.waiting));
        self.unclaim(&id);
        Reply::read(&bytes)
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
    /// being answered returns as declined.
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
