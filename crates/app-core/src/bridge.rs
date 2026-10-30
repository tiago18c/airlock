//! The browser requests waiting for the user. The core holds the one queue:
//! the transport pushes a request with a way to reply, the window reads the
//! published [`BridgeQueue`], and the user's answer goes back through
//! [`Core::bridge_answer`](crate::Core::bridge_answer).
//!
//! The transport (the socket) stays in the desktop shell.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bridge_protocol::reply::SiteAccount;
use serde_json::Value;
use solana_pubkey::Pubkey;
use wallet_core::{ConfirmationLevel, ConnectChoice, SiteGrant};

use crate::notices::BridgeRequestOutcome;

/// Sends the bytes of the reply frame back to the site. Called at most once.
pub type BridgeReply = Box<dyn FnOnce(Vec<u8>) + Send>;

/// What the user decided about a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeAnswer {
    /// A `connect`, with the accounts the site may see.
    Connect(Vec<SiteAccount>),
    /// A `signTransaction`, with every transaction it asked for, signed, in
    /// the order the site sent them.
    Signed(Vec<Vec<u8>>),
    /// A `signAndSendTransaction` or `signAndSendAllTransactions`, with what
    /// became of each transaction, in the order the site sent them.
    Sent(Vec<Result<[u8; 64], String>>),
    /// A `signMessage`, with the bytes signed and the signature.
    SignedMessage {
        signed: Vec<u8>,
        signature: [u8; 64],
    },
    /// A `signIn`: `account` signed `signed`, and `accounts` are what the site
    /// may see now.
    SignedIn {
        account: [u8; 32],
        accounts: Vec<SiteAccount>,
        signed: Vec<u8>,
        signature: [u8; 64],
    },
    Rejected,
}

impl BridgeAnswer {
    /// Whether this answers `request`. Rejecting fits any. A signed answer
    /// fits only with one transaction for each the site sent, so part of a
    /// batch never goes back.
    fn fits(&self, request: &BridgeRequest) -> bool {
        match self {
            Self::Connect(_) => request.method == "connect",
            Self::Signed(wires) => {
                request.method == "signTransaction"
                    && !wires.is_empty()
                    && wires.len() == request.transaction_count()
            }
            Self::Sent(results) => {
                request.sends()
                    && !results.is_empty()
                    && results.len() == request.transaction_count()
            }
            Self::SignedMessage { .. } => request.method == "signMessage",
            Self::SignedIn { .. } => request.method == "signIn",
            Self::Rejected => true,
        }
    }

    /// The reply frame for request `id`, and the outcome for its notice.
    pub(crate) fn reply(&self, id: &str, method: &str) -> (Vec<u8>, BridgeRequestOutcome) {
        match self {
            Self::Sent(results) if method == SEND_ALL => (
                bridge_protocol::reply::sent_each(id, results),
                BridgeRequestOutcome::Answered,
            ),
            Self::Sent(results) => {
                // All of them, or why not: the first failure says what happened.
                let frame = match results.iter().find_map(|one| one.as_ref().err()) {
                    Some(message) => bridge_protocol::reply::send_failed(id, message),
                    None => {
                        let signatures: Vec<[u8; 64]> =
                            results.iter().filter_map(|one| one.clone().ok()).collect();
                        bridge_protocol::reply::sent(id, &signatures)
                    }
                };
                (frame, BridgeRequestOutcome::Answered)
            }
            Self::Connect(accounts) => (
                bridge_protocol::reply::connected(id, accounts),
                BridgeRequestOutcome::Answered,
            ),
            Self::Signed(wires) => (
                bridge_protocol::reply::signed(id, wires),
                BridgeRequestOutcome::Answered,
            ),
            Self::SignedMessage { signed, signature } => (
                bridge_protocol::reply::message_signed(id, signed, signature),
                BridgeRequestOutcome::Answered,
            ),
            Self::SignedIn {
                account,
                accounts,
                signed,
                signature,
            } => (
                bridge_protocol::reply::signed_in(id, account, accounts, signed, signature),
                BridgeRequestOutcome::Answered,
            ),
            Self::Rejected => (
                bridge_protocol::reply::rejected(id),
                BridgeRequestOutcome::Rejected,
            ),
        }
    }
}

/// `signAndSendTransaction`: the app sends what it signs.
pub const SEND: &str = "signAndSendTransaction";
/// `signAndSendAllTransactions`: the same, with a result for each transaction.
pub const SEND_ALL: &str = "signAndSendAllTransactions";

/// What a `connect` from a site gets before anyone is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteConnect {
    /// The user blocked the site.
    Blocked,
    /// The site reconnects with these accounts, the current wallet first.
    Allowed(Vec<SiteAccount>),
    /// Nothing the site was given covers this: only the user can decide.
    Undecided,
}

/// Whether a site may ask for a signature from some accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteSign {
    Blocked,
    /// One of the accounts is not connected to the site.
    NotConnected,
    Allowed,
}

/// What the connect modal opens with for a site.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteChoice {
    pub choice: ConnectChoice,
    /// The site is remembered, so the box starts ticked.
    pub remembered: bool,
}

/// A site with a page open that holds accounts now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectedSite {
    pub origin: String,
    pub accounts: Vec<Pubkey>,
    /// It is also in [`SitesView::authorized`].
    pub remembered: bool,
}

/// What the connections modal lists, and what sites are told.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SitesView {
    pub connected: Vec<ConnectedSite>,
    pub authorized: Vec<SiteGrant>,
    pub blocked: Vec<String>,
    /// What each site holds now, the account it acts as first. The bridge
    /// tells a site when its entry changes.
    pub holding: BTreeMap<String, Vec<Pubkey>>,
}

/// One request a site made through the extension.
#[derive(Clone, Debug, PartialEq)]
pub struct BridgeRequest {
    pub id: String,
    pub origin: String,
    pub method: String,
    pub params: Value,
}

impl BridgeRequest {
    /// The four characters shown beside it, to match the extension's list.
    pub fn code(&self) -> String {
        bridge_protocol::request_code(&self.id)
    }

    /// Whether the app sends what this asks to have signed.
    pub fn sends(&self) -> bool {
        self.method == SEND || self.method == SEND_ALL
    }

    /// Whether this takes a connection, so that ending one rejects it. A
    /// `connect` asks for one, and a `signIn` makes one.
    pub fn needs_connection(&self) -> bool {
        !matches!(self.method.as_str(), "connect" | "signIn")
    }

    /// How the transactions of a send go out. A site that names no mode gets
    /// `parallel`, the standard's first.
    pub fn send_plan(&self) -> SendPlan {
        if self.method != SEND_ALL {
            return SendPlan::InOrder;
        }
        match self.params.get("mode").and_then(Value::as_str) {
            Some("serial") => SendPlan::Serial,
            _ => SendPlan::Independent,
        }
    }

    /// The commitment a site asked each transaction of a send to reach.
    pub fn send_commitments(&self) -> Vec<Option<ConfirmationLevel>> {
        let items = self.params.get("transactions").and_then(Value::as_array);
        items
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        match item
                            .get("options")
                            .and_then(|options| options.get("commitment"))
                            .and_then(Value::as_str)
                        {
                            Some("processed") => Some(ConfirmationLevel::Processed),
                            Some("confirmed") => Some(ConfirmationLevel::Confirmed),
                            Some("finalized") => Some(ConfirmationLevel::Finalized),
                            _ => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The networks the transactions of this name, each once.
    pub fn chains(&self) -> Vec<String> {
        let mut chains: Vec<String> = self
            .params
            .get("transactions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("chain")?.as_str().map(str::to_owned))
            .collect();
        chains.dedup();
        chains
    }

    /// How many transactions a `signTransaction` asks to have signed.
    pub fn transaction_count(&self) -> usize {
        self.params
            .get("transactions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    }
}

/// Pending requests, oldest first. The head is the one on screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BridgeQueue {
    pending: Vec<BridgeRequest>,
}

impl BridgeQueue {
    pub fn pending(&self) -> &[BridgeRequest] {
        &self.pending
    }

    pub fn head(&self) -> Option<&BridgeRequest> {
        self.pending.first()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&BridgeRequest> {
        self.pending.iter().find(|request| request.id == id)
    }
}

/// How the transactions of a send go out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendPlan {
    InOrder,
    Independent,
    Serial,
}

/// A send the user approved: out of the queue, so nobody can reject it while
/// it goes out, with the way to tell the site how it went.
pub struct SendingRequest {
    pub request: BridgeRequest,
    pub(crate) reply: Option<BridgeReply>,
}

/// The queue and, beside it, how to reply to each request in it. Only the
/// queue is published; a reply is used once and never leaves the core.
#[derive(Default)]
pub(crate) struct BridgeState {
    queue: BridgeQueue,
    replies: HashMap<String, BridgeReply>,
    /// Sites connected without being remembered. They last until the site
    /// disconnects or the app closes.
    sessions: Vec<SiteGrant>,
    /// What each site holds now.
    holding: BTreeMap<String, Vec<Pubkey>>,
    /// Origins with a page open, in any browser.
    pages: BTreeSet<String>,
}

/// A request taken out of the queue, with its reply when it had one.
pub(crate) struct Taken {
    pub request: BridgeRequest,
    pub reply: Option<BridgeReply>,
}

impl BridgeState {
    pub fn queue(&self) -> &BridgeQueue {
        &self.queue
    }

    pub fn push(&mut self, request: BridgeRequest, reply: BridgeReply) {
        self.replies.insert(request.id.clone(), reply);
        self.queue.pending.push(request);
    }

    /// Take `id` out of the queue: any request, or with `answer`, only one
    /// whose method it answers.
    pub fn take(&mut self, id: &str, answer: Option<&BridgeAnswer>) -> Option<Taken> {
        let index = self.queue.pending.iter().position(|r| r.id == id)?;
        let request = &self.queue.pending[index];
        if answer.is_some_and(|answer| !answer.fits(request)) {
            return None;
        }
        let request = self.queue.pending.remove(index);
        let reply = self.replies.remove(id);
        Some(Taken { request, reply })
    }

    /// The ids of the requests waiting from `origin`: all of them, or only
    /// those that take a connection.
    pub fn waiting_from(&self, origin: &str, only_connected: bool) -> Vec<String> {
        self.queue
            .pending
            .iter()
            .filter(|request| {
                request.origin == origin && (!only_connected || request.needs_connection())
            })
            .map(|request| request.id.clone())
            .collect()
    }

    pub fn session(&self, origin: &str) -> Option<&SiteGrant> {
        self.sessions.iter().find(|site| site.origin == origin)
    }

    pub fn set_session(&mut self, origin: &str, accounts: Vec<Pubkey>) {
        self.end_session(origin);
        self.sessions.push(SiteGrant::new(origin, accounts));
    }

    pub fn end_session(&mut self, origin: &str) {
        self.sessions.retain(|site| site.origin != origin);
    }

    pub fn holding(&self) -> &BTreeMap<String, Vec<Pubkey>> {
        &self.holding
    }

    pub fn set_holding(&mut self, origin: &str, accounts: Vec<Pubkey>) {
        self.holding.insert(origin.to_owned(), accounts);
    }

    pub fn drop_holding(&mut self, origin: &str) {
        self.holding.remove(origin);
    }

    /// The origins with a page open. A site with none left holds nothing.
    pub fn set_pages(&mut self, pages: BTreeSet<String>) {
        let closed: Vec<String> = self
            .pages
            .difference(&pages)
            .filter(|origin| self.holding.contains_key(*origin))
            .cloned()
            .collect();
        for origin in closed {
            self.holding.remove(&origin);
        }
        self.pages = pages;
    }

    /// The sites to show as connected: those holding accounts with a page open.
    pub fn connected(&self) -> impl Iterator<Item = (&String, &Vec<Pubkey>)> {
        self.holding
            .iter()
            .filter(|(origin, _)| self.pages.contains(*origin))
    }
}
