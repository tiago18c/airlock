//! The browser requests waiting for the user. The core holds the one queue:
//! the transport pushes a request with a way to reply, the window reads the
//! published [`BridgeQueue`], and the user's answer goes back through
//! [`Core::bridge_answer`](crate::Core::bridge_answer).
//!
//! The transport (the socket) stays in the desktop shell.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use bridge_protocol::reply::{SendError, SiteAccount};
use bridge_protocol::{MAX_PENDING, MAX_PENDING_PER_ORIGIN};
use solana_pubkey::Pubkey;
use wallet_core::site_message::SignIn;
use wallet_core::{ConfirmationLevel, ConnectChoice, SiteGrant, TxFormat};

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
    Sent(Vec<Result<[u8; 64], SendError>>),
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
        match (self, &request.call) {
            (Self::Rejected, _)
            | (Self::Connect(_), SiteCall::Connect)
            | (Self::SignedMessage { .. }, SiteCall::SignMessage { .. })
            | (Self::SignedIn { .. }, SiteCall::SignIn(_)) => true,
            (Self::Signed(wires), SiteCall::SignTransactions(transactions)) => {
                !wires.is_empty() && wires.len() == transactions.len()
            }
            (Self::Sent(results), SiteCall::Send { transactions, .. }) => {
                !results.is_empty() && results.len() == transactions.len()
            }
            _ => false,
        }
    }

    /// The reply frame for `request`, and the outcome for its notice.
    pub(crate) fn reply(&self, request: &BridgeRequest) -> (Vec<u8>, BridgeRequestOutcome) {
        let id = request.id.as_str();
        match self {
            Self::Sent(results) if request.send_plan() != SendPlan::InOrder => (
                bridge_protocol::reply::sent_each(id, results),
                BridgeRequestOutcome::Answered,
            ),
            Self::Sent(results) => {
                // All of them, or why not: the first failure says what happened,
                // unless one may still land, which a site must hear about.
                let failures = || results.iter().filter_map(|one| one.as_ref().err());
                let why = failures()
                    .find(|why| matches!(why, SendError::Unknown(_)))
                    .or_else(|| failures().next());
                let frame = match why {
                    Some(why) => bridge_protocol::reply::send_error(id, why),
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

/// What a site asks for. The transport reads it from the frame once, and checks
/// it as it reads, so what a modal shows is what was checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteCall {
    Connect,
    /// `signTransaction`: sign these and give them back.
    SignTransactions(Vec<SiteTransaction>),
    /// `signAndSendTransaction`, or by any other plan,
    /// `signAndSendAllTransactions`: the app sends what it signs.
    Send {
        transactions: Vec<SiteTransaction>,
        plan: SendPlan,
    },
    SignMessage {
        account: Pubkey,
        message: Vec<u8>,
    },
    SignIn(Box<SignIn>),
}

impl SiteCall {
    /// The name the site used for it. Notices keep this.
    pub fn method(&self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::SignTransactions(_) => "signTransaction",
            Self::Send {
                plan: SendPlan::InOrder,
                ..
            } => SEND,
            Self::Send { .. } => SEND_ALL,
            Self::SignMessage { .. } => "signMessage",
            Self::SignIn(_) => "signIn",
        }
    }
}

/// One transaction a site handed over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteTransaction {
    /// The account the site says it is for. The site chose it, so it is shown
    /// and never relied on.
    pub account: Pubkey,
    pub wire: SiteWire,
    /// The network it names.
    pub chain: Option<String>,
    /// The commitment the site asked it to reach, for a send.
    pub commitment: Option<ConfirmationLevel>,
}

/// As far as the bytes a site sent for a transaction could be read. One that
/// cannot be still opens the modal, which says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteWire {
    Missing,
    NotBase64,
    /// This many bytes that are no Solana transaction.
    Unreadable(usize),
    Readable(TxFormat, Vec<u8>),
}

impl SiteWire {
    /// The bytes of a transaction that could be read.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Readable(_, bytes) => Some(bytes),
            _ => None,
        }
    }
}

/// One request a site made through the extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeRequest {
    pub id: String,
    pub origin: String,
    pub call: SiteCall,
}

impl BridgeRequest {
    /// The four characters shown beside it, to match the extension's list.
    pub fn code(&self) -> String {
        bridge_protocol::request_code(&self.id)
    }

    /// The name the site used for it.
    pub fn method(&self) -> &'static str {
        self.call.method()
    }

    /// The transactions it hands over, if any.
    pub fn transactions(&self) -> &[SiteTransaction] {
        match &self.call {
            SiteCall::SignTransactions(transactions) | SiteCall::Send { transactions, .. } => {
                transactions
            }
            _ => &[],
        }
    }

    /// Whether the app sends what this asks to have signed.
    pub fn sends(&self) -> bool {
        matches!(self.call, SiteCall::Send { .. })
    }

    /// Whether this takes a connection, so that ending one rejects it. A
    /// `connect` asks for one, and a `signIn` makes one.
    pub fn needs_connection(&self) -> bool {
        !matches!(self.call, SiteCall::Connect | SiteCall::SignIn(_))
    }

    /// How the transactions of a send go out.
    pub fn send_plan(&self) -> SendPlan {
        match self.call {
            SiteCall::Send { plan, .. } => plan,
            _ => SendPlan::InOrder,
        }
    }

    /// The commitment a site asked each transaction of a send to reach.
    pub fn send_commitments(&self) -> Vec<Option<ConfirmationLevel>> {
        self.transactions()
            .iter()
            .map(|transaction| transaction.commitment)
            .collect()
    }

    /// The networks the transactions of this name, each once.
    pub fn chains(&self) -> Vec<String> {
        let mut chains: Vec<String> = self
            .transactions()
            .iter()
            .filter_map(|transaction| transaction.chain.clone())
            .collect();
        chains.dedup();
        chains
    }

    /// How many transactions it hands over.
    pub fn transaction_count(&self) -> usize {
        self.transactions().len()
    }
}

/// Why a request did not join the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeRefused {
    /// Its site, or every site together, has as many waiting as may wait.
    Full,
    /// One with this id is waiting, and answers find a request by its id.
    SameId,
}

/// Pending requests, oldest first. The head is the one on screen. Published
/// each time it changes, so it holds them by reference: one can be 1 MiB.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BridgeQueue {
    pending: Vec<Arc<BridgeRequest>>,
}

impl BridgeQueue {
    pub fn pending(&self) -> &[Arc<BridgeRequest>] {
        &self.pending
    }

    pub fn head(&self) -> Option<&BridgeRequest> {
        self.pending.first().map(Arc::as_ref)
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&BridgeRequest> {
        self.pending
            .iter()
            .find(|request| request.id == id)
            .map(Arc::as_ref)
    }
}

/// How the transactions of a send go out. A site that names no mode gets
/// `parallel`, the standard's first, which is `Independent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendPlan {
    InOrder,
    Independent,
    Serial,
}

/// A send the user approved: out of the queue, so nobody can reject it while
/// it goes out, with the way to tell the site how it went.
pub struct SendingRequest {
    pub request: Arc<BridgeRequest>,
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
    pub request: Arc<BridgeRequest>,
    pub reply: Option<BridgeReply>,
}

impl BridgeState {
    pub fn queue(&self) -> &BridgeQueue {
        &self.queue
    }

    pub fn push(
        &mut self,
        request: BridgeRequest,
        reply: BridgeReply,
    ) -> Result<(), BridgeRefused> {
        if self.queue.get(&request.id).is_some() {
            return Err(BridgeRefused::SameId);
        }
        let from_site = self
            .queue
            .pending
            .iter()
            .filter(|waiting| waiting.origin == request.origin)
            .count();
        if self.queue.len() >= MAX_PENDING || from_site >= MAX_PENDING_PER_ORIGIN {
            return Err(BridgeRefused::Full);
        }
        self.replies.insert(request.id.clone(), reply);
        self.queue.pending.push(Arc::new(request));
        Ok(())
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
