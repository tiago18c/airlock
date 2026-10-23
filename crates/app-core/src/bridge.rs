//! The browser requests waiting for the user. The core holds the one queue:
//! the transport pushes a request with a way to reply, the window reads the
//! published [`BridgeQueue`], and the user's answer goes back through
//! [`Core::bridge_answer`](crate::Core::bridge_answer).
//!
//! The transport (the socket) stays in the desktop shell.

use std::collections::HashMap;

use serde_json::Value;

use crate::notices::BridgeRequestOutcome;

/// Sends the bytes of the reply frame back to the site. Called at most once.
pub type BridgeReply = Box<dyn FnOnce(Vec<u8>) + Send>;

/// What the user decided about a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeAnswer {
    /// A `connect`, with the public keys and labels the site may see.
    Connect(Vec<([u8; 32], String)>),
    /// A `signTransaction`, with the signed transaction.
    Signed(Vec<u8>),
    Rejected,
}

impl BridgeAnswer {
    /// Whether this answers a request of `method`. Rejecting fits any.
    fn fits(&self, method: &str) -> bool {
        match self {
            Self::Connect(_) => method == "connect",
            Self::Signed(_) => method == "signTransaction",
            Self::Rejected => true,
        }
    }

    /// The reply frame for request `id`, and the outcome for its notice.
    pub(crate) fn reply(&self, id: &str) -> (Vec<u8>, BridgeRequestOutcome) {
        match self {
            Self::Connect(accounts) => (
                bridge_protocol::reply::connected(id, accounts),
                BridgeRequestOutcome::Answered,
            ),
            Self::Signed(wire) => (
                bridge_protocol::reply::signed(id, wire),
                BridgeRequestOutcome::Answered,
            ),
            Self::Rejected => (
                bridge_protocol::reply::rejected(id),
                BridgeRequestOutcome::Rejected,
            ),
        }
    }
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

/// The queue and, beside it, how to reply to each request in it. Only the
/// queue is published; a reply is used once and never leaves the core.
#[derive(Default)]
pub(crate) struct BridgeState {
    queue: BridgeQueue,
    replies: HashMap<String, BridgeReply>,
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
        let method = &self.queue.pending[index].method;
        if answer.is_some_and(|answer| !answer.fits(method)) {
            return None;
        }
        let request = self.queue.pending.remove(index);
        let reply = self.replies.remove(id);
        Some(Taken { request, reply })
    }
}
