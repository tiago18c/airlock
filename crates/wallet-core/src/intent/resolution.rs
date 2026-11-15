//! A resolution: what became of a request, kept by the wallet and followed
//! by the site ("The resolution" in `docs/intents.md`).
//!
//! Each change is a method that checks the state it starts from. None of
//! them reads the clock: the caller passes `now`, in Unix seconds.

use super::error::Requirement;
use super::notes::{Delivery, Order};
use super::profile::Timing;
use super::role::TxRole;
use super::text;
use crate::tx::decode::parse_wire;
use crate::tx::renew::without_signatures;
use crate::tx::signatures::insert_signature;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use solana_signature::{Signature, SIGNATURE_BYTES};
use std::collections::BTreeMap;
use thiserror::Error;

/// Where a resolution stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// The person has not decided.
    AwaitingApproval,
    /// Approved here. Another device or other people still sign.
    AwaitingSignatures,
    /// Composed and signed here. The app owes signatures.
    AwaitingApp,
    /// Handed to the app to sign and send.
    Returned,
    Sending,
    /// Every transaction sent in has run.
    Done,
    /// It landed and failed, or it can no longer land.
    Failed,
    /// It may still land.
    Unknown,
    Declined,
    Expired,
    Cancelled,
}

impl State {
    /// Nothing changes it any more.
    pub fn is_final(self) -> bool {
        matches!(
            self,
            Self::Done
                | Self::Failed
                | Self::Unknown
                | Self::Declined
                | Self::Expired
                | Self::Cancelled
        )
    }

    /// The name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Self::AwaitingApproval => "awaiting-approval",
            Self::AwaitingSignatures => "awaiting-signatures",
            Self::AwaitingApp => "awaiting-app",
            Self::Returned => "returned",
            Self::Sending => "sending",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Declined => "declined",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Where one transaction the wallet made stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TxState {
    #[default]
    Pending,
    Landed,
    Failed,
    Unknown,
}

/// One transaction the wallet made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTx {
    pub role: TxRole,
    /// Which transactions of the request it carries.
    pub of: Vec<usize>,
    pub state: TxState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "text::option_signature"
    )]
    pub signature: Option<Signature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// In `awaiting-app` with every signature slot empty, in `returned` as
    /// signed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "text::option_base64"
    )]
    pub wire: Option<Vec<u8>>,
}

/// Where a transaction sent in ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ran {
    /// Which transaction of the request.
    pub transaction: usize,
    #[serde(with = "text::signature")]
    pub signature: Signature,
}

/// Whose signatures the app owes on one transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Awaiting {
    /// Its index in [`Resolution::transactions`].
    pub transaction: usize,
    #[serde(with = "text::addresses")]
    pub signers: Vec<Pubkey>,
}

/// Set when a resolution resolves a message signed as a memo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoMessage {
    pub memo: String,
}

/// Why a resolution failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionFailure {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement: Option<Requirement>,
}

/// One transaction the wallet composed, and signed as far as it signs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composed {
    pub role: TxRole,
    pub of: Vec<usize>,
    pub wire: Vec<u8>,
}

/// What sending one transaction came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendResult {
    Landed {
        signature: Signature,
    },
    /// It landed and failed, or it can no longer land.
    Failed {
        signature: Option<Signature>,
        reason: String,
    },
    /// It may still land.
    Unknown {
        signature: Option<Signature>,
    },
    /// It never went out: one before it failed, and the order stopped the
    /// rest. It has no signature anyone could look for.
    NotSent {
        reason: String,
    },
}

/// A change a resolution does not take.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ResolutionError {
    #[error("The resolution is {}, which does not take {action}.", state.name())]
    BadState { state: State, action: &'static str },
    #[error("{0}")]
    BadSignature(String),
    #[error("{0}")]
    Mismatch(String),
}

impl ResolutionError {
    /// The error code on the wire. A [`ResolutionError::Mismatch`] is a
    /// mistake of the caller, not of the site.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadState { .. } => "bad_state",
            Self::BadSignature(_) => "bad_signature",
            Self::Mismatch(_) => "internal",
        }
    }
}

/// `Resolution` of `docs/intents.md`, with what the wallet keeps from the
/// site: the wires it signed and holds until it sends them, under `held`.
/// Saved as it is. What a site sees is [`Resolution::public`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub id: String,
    #[serde(with = "text::address")]
    pub account: Pubkey,
    pub chain: String,
    pub timing: Timing,
    pub delivery: Delivery,
    pub state: State,
    /// Address sent to address used.
    #[serde(default, with = "text::address_pairs")]
    pub replaced: BTreeMap<Pubkey, Pubkey>,
    /// What the wallet made, once it has.
    #[serde(default)]
    pub transactions: Vec<ResolvedTx>,
    #[serde(default)]
    pub ran: Vec<Ran>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting: Option<Vec<Awaiting>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<MemoMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ResolutionFailure>,
    pub updated_at: u64,
    /// How many transactions the request sent in.
    #[serde(default)]
    pub requested: usize,
    /// How the request asked its transactions to run, which is how the
    /// wallet sends them. Never shown to a site, which knows.
    #[serde(default)]
    pub order: Order,
    /// The wires as the wallet signed them, one for each of
    /// [`Self::transactions`], while it holds them: in `awaiting-app` and in
    /// `sending`. Never shown to a site.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        with = "text::base64_list"
    )]
    pub held: Vec<Vec<u8>>,
}

impl Resolution {
    /// A request just taken, in `awaiting-approval`. `requested` is how
    /// many transactions it carries. Set [`Self::replaced`] to what the
    /// notes' rewrite replaced.
    pub fn new(
        id: impl Into<String>,
        account: Pubkey,
        chain: impl Into<String>,
        timing: Timing,
        delivery: Delivery,
        requested: usize,
        now: u64,
    ) -> Self {
        Self {
            id: id.into(),
            account,
            chain: chain.into(),
            timing,
            delivery,
            state: State::AwaitingApproval,
            replaced: BTreeMap::new(),
            transactions: Vec::new(),
            ran: Vec::new(),
            awaiting: None,
            message: None,
            error: None,
            updated_at: now,
            requested,
            order: Order::default(),
            held: Vec::new(),
        }
    }

    /// A message signed as a memo, resolved as one request with delivery
    /// `send`.
    pub fn for_memo(
        id: impl Into<String>,
        account: Pubkey,
        chain: impl Into<String>,
        timing: Timing,
        memo: impl Into<String>,
        now: u64,
    ) -> Self {
        Self {
            message: Some(MemoMessage { memo: memo.into() }),
            ..Self::new(id, account, chain, timing, Delivery::Send, 1, now)
        }
    }

    /// What a site sees: everything but [`Self::held`],
    /// [`Self::requested`] and [`Self::order`].
    pub fn public(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(Self {
            held: Vec::new(),
            ..self.clone()
        })
        .unwrap_or_default();
        if let Some(object) = value.as_object_mut() {
            object.remove("requested");
            object.remove("order");
        }
        value
    }

    fn require(&self, allowed: &[State], action: &'static str) -> Result<(), ResolutionError> {
        if allowed.contains(&self.state) {
            Ok(())
        } else {
            Err(ResolutionError::BadState {
                state: self.state,
                action,
            })
        }
    }

    fn set(&mut self, state: State, now: u64) {
        self.state = state;
        self.updated_at = now;
        if state.is_final() {
            self.held.clear();
            self.awaiting = None;
        }
        if state == State::Returned {
            self.held.clear();
        }
    }

    /// The person declined. Only while it awaits approval.
    pub fn decline(&mut self, now: u64) -> Result<(), ResolutionError> {
        self.require(&[State::AwaitingApproval], "declining")?;
        self.set(State::Declined, now);
        Ok(())
    }

    /// It waited longer than [`Timing::approval_window_secs`]. Only while it
    /// awaits approval: once approved it does not expire.
    pub fn expire(&mut self, now: u64) -> Result<(), ResolutionError> {
        self.require(&[State::AwaitingApproval], "expiring")?;
        self.set(State::Expired, now);
        Ok(())
    }

    /// The site cancelled. While it awaits approval, or the app's
    /// signatures: the wallet's signatures have not left it then, and are
    /// dropped.
    pub fn cancel(&mut self, now: u64) -> Result<(), ResolutionError> {
        self.require(&[State::AwaitingApproval, State::AwaitingApp], "cancelling")?;
        self.set(State::Cancelled, now);
        Ok(())
    }

    /// Approved and signed here: `composed` holds each transaction the
    /// wallet made, signed as far as this device signs it. What follows
    /// depends on the delivery:
    ///
    /// - `send`: `sending`.
    /// - `cosign`: `awaiting-app`, each public `wire` with every signature
    ///   slot empty, and `awaiting` listing the signers whose slots the
    ///   wallet left empty. `sending` at once when none is.
    /// - `return`: `returned`, each `wire` as signed, and `awaiting` as for
    ///   `cosign`. The wallet holds nothing: its signatures are out.
    pub fn signed(&mut self, composed: Vec<Composed>, now: u64) -> Result<(), ResolutionError> {
        self.require(&[State::AwaitingApproval], "signing")?;
        let mut transactions = Vec::with_capacity(composed.len());
        let mut awaiting = Vec::new();
        for (index, made) in composed.iter().enumerate() {
            let view =
                parse_wire(&made.wire).map_err(|err| ResolutionError::Mismatch(err.to_string()))?;
            let empty: Vec<Pubkey> = (0..view.num_required_signatures)
                .filter(|slot| !view.slot_valid(*slot))
                .map(|slot| view.static_keys[slot])
                .collect();
            if !empty.is_empty() {
                awaiting.push(Awaiting {
                    transaction: index,
                    signers: empty,
                });
            }
            let wire = match self.delivery {
                Delivery::Send => None,
                Delivery::Cosign => Some(
                    without_signatures(&made.wire)
                        .map_err(|err| ResolutionError::Mismatch(err.to_string()))?,
                ),
                Delivery::Return => Some(made.wire.clone()),
            };
            transactions.push(ResolvedTx {
                role: made.role,
                of: made.of.clone(),
                state: TxState::Pending,
                signature: view.signature(),
                error: None,
                wire,
            });
        }
        if self.delivery == Delivery::Send {
            if let Some(open) = awaiting.first() {
                return Err(ResolutionError::Mismatch(format!(
                    "transaction {} still waits on {} signatures",
                    open.transaction,
                    open.signers.len()
                )));
            }
        }
        self.transactions = transactions;
        self.held = composed.into_iter().map(|made| made.wire).collect();
        match self.delivery {
            Delivery::Send => self.set(State::Sending, now),
            Delivery::Cosign if awaiting.is_empty() => self.start_sending(now),
            Delivery::Cosign => {
                self.awaiting = Some(awaiting);
                self.set(State::AwaitingApp, now);
            }
            Delivery::Return => {
                self.awaiting = Some(awaiting).filter(|awaiting| !awaiting.is_empty());
                self.set(State::Returned, now);
            }
        }
        Ok(())
    }

    /// Approved here, and another device or other people still sign: an
    /// offline key, or a multisig this device cannot carry alone.
    pub fn approved_elsewhere(&mut self, now: u64) -> Result<(), ResolutionError> {
        self.require(&[State::AwaitingApproval], "approving")?;
        self.set(State::AwaitingSignatures, now);
        Ok(())
    }

    /// The signatures from elsewhere are in: `composed` is every transaction,
    /// fully signed, and it goes out.
    pub fn signed_elsewhere(
        &mut self,
        composed: Vec<Composed>,
        now: u64,
    ) -> Result<(), ResolutionError> {
        self.require(&[State::AwaitingSignatures], "signatures from elsewhere")?;
        self.state = State::AwaitingApproval;
        self.signed(composed, now).inspect_err(|_| {
            self.state = State::AwaitingSignatures;
        })
    }

    /// `signature`, by `signer`, on transaction `transaction` of
    /// [`Self::transactions`]. Refused unless the app owes that signature
    /// and it verifies over that transaction's message. Once none is owed,
    /// `sending`.
    pub fn provide_signature(
        &mut self,
        transaction: usize,
        signer: Pubkey,
        signature: &[u8; SIGNATURE_BYTES],
        now: u64,
    ) -> Result<(), ResolutionError> {
        self.require(&[State::AwaitingApp], "signatures")?;
        let owed = self.awaiting.as_ref().is_some_and(|awaiting| {
            awaiting
                .iter()
                .any(|item| item.transaction == transaction && item.signers.contains(&signer))
        });
        if !owed {
            return Err(ResolutionError::BadSignature(format!(
                "No signature of {signer} is owed on transaction {transaction}."
            )));
        }
        let wire = self
            .held
            .get(transaction)
            .ok_or_else(|| ResolutionError::Mismatch("no wire is held for it".into()))?;
        let signed = insert_signature(wire, &signer, signature).map_err(|_| {
            ResolutionError::BadSignature(format!(
                "The signature of {signer} does not verify for transaction {transaction}."
            ))
        })?;
        self.held[transaction] = signed;
        if let Some(awaiting) = self.awaiting.as_mut() {
            for item in awaiting.iter_mut() {
                if item.transaction == transaction {
                    item.signers.retain(|owed| *owed != signer);
                }
            }
            awaiting.retain(|item| !item.signers.is_empty());
        }
        if self.awaiting.as_ref().is_some_and(Vec::is_empty) {
            self.start_sending(now);
        } else {
            self.updated_at = now;
        }
        Ok(())
    }

    /// Every signature is in: `sending`, each public wire dropped and each
    /// transaction's signature read from what is held.
    fn start_sending(&mut self, now: u64) {
        self.awaiting = None;
        for (made, wire) in self.transactions.iter_mut().zip(&self.held) {
            made.wire = None;
            made.signature = parse_wire(wire).ok().and_then(|view| view.signature());
        }
        self.set(State::Sending, now);
    }

    /// The wires to send, in order, while it is `sending`.
    pub fn wires_to_send(&self) -> Option<&[Vec<u8>]> {
        (self.state == State::Sending).then_some(self.held.as_slice())
    }

    /// What sending each transaction came to, one result for each of
    /// [`Self::transactions`]. From `sending`, or from `returned` when the
    /// wallet watched for what it returned.
    ///
    /// `unknown` when any may still land, then `failed` when any failed.
    /// When all landed, `awaiting-signatures` when one is a proposal, which
    /// has not run, otherwise `done`, with `ran` filled.
    pub fn sent(&mut self, results: Vec<SendResult>, now: u64) -> Result<(), ResolutionError> {
        self.require(&[State::Sending, State::Returned], "send results")?;
        if results.len() != self.transactions.len() {
            return Err(ResolutionError::Mismatch(format!(
                "{} results for {} transactions",
                results.len(),
                self.transactions.len()
            )));
        }
        for (made, result) in self.transactions.iter_mut().zip(results) {
            match result {
                SendResult::Landed { signature } => {
                    made.state = TxState::Landed;
                    made.signature = Some(signature);
                }
                SendResult::Failed { signature, reason } => {
                    made.state = TxState::Failed;
                    made.signature = signature.or(made.signature);
                    made.error = Some(reason);
                }
                SendResult::Unknown { signature } => {
                    made.state = TxState::Unknown;
                    made.signature = signature.or(made.signature);
                }
                SendResult::NotSent { reason } => {
                    made.state = TxState::Failed;
                    made.signature = None;
                    made.error = Some(reason);
                }
            }
        }
        let any = |state: TxState| self.transactions.iter().any(|made| made.state == state);
        let next = if any(TxState::Unknown) {
            State::Unknown
        } else if any(TxState::Failed) {
            State::Failed
        } else {
            for made in &self.transactions {
                if made.role == TxRole::Vote || made.role == TxRole::Proposal {
                    continue;
                }
                if let Some(signature) = made.signature {
                    for transaction in &made.of {
                        record_ran(&mut self.ran, *transaction, signature);
                    }
                }
            }
            if self.all_ran() {
                State::Done
            } else {
                State::AwaitingSignatures
            }
        };
        if next == State::Failed {
            let reason = self
                .transactions
                .iter()
                .find_map(|made| made.error.clone())
                .unwrap_or_default();
            self.error = Some(ResolutionFailure {
                code: "failed".into(),
                message: reason,
                requirement: None,
            });
        }
        self.set(next, now);
        Ok(())
    }

    /// The proposal that transaction `transaction` of [`Self::transactions`]
    /// created was executed, in the transaction `signature`. `done` once
    /// every transaction sent in has run.
    pub fn executed(
        &mut self,
        transaction: usize,
        signature: Signature,
        now: u64,
    ) -> Result<(), ResolutionError> {
        self.require(&[State::AwaitingSignatures], "an execution")?;
        let made = self.transactions.get(transaction).ok_or_else(|| {
            ResolutionError::Mismatch(format!("there is no transaction {transaction}"))
        })?;
        if made.role != TxRole::Proposal || made.state != TxState::Landed {
            return Err(ResolutionError::Mismatch(format!(
                "transaction {transaction} is not a proposal that landed"
            )));
        }
        for of in made.of.clone() {
            record_ran(&mut self.ran, of, signature);
        }
        if self.all_ran() {
            self.set(State::Done, now);
        } else {
            self.updated_at = now;
        }
        Ok(())
    }

    /// A proposal it waits on was rejected or cancelled, so what it holds
    /// can no longer run: `failed`.
    pub fn proposal_closed(
        &mut self,
        reason: impl Into<String>,
        now: u64,
    ) -> Result<(), ResolutionError> {
        self.require(&[State::AwaitingSignatures], "a closed proposal")?;
        self.error = Some(ResolutionFailure {
            code: "failed".into(),
            message: reason.into(),
            requirement: None,
        });
        self.set(State::Failed, now);
        Ok(())
    }

    /// It cannot go on, for `failure`, before anything was sent. Only from a
    /// state that has not sent anything: a transaction out of the wallet
    /// ends in [`Self::sent`].
    pub fn fail(&mut self, failure: ResolutionFailure, now: u64) -> Result<(), ResolutionError> {
        self.require(
            &[
                State::AwaitingApproval,
                State::AwaitingSignatures,
                State::AwaitingApp,
            ],
            "a failure",
        )?;
        self.error = Some(failure);
        self.set(State::Failed, now);
        Ok(())
    }

    fn all_ran(&self) -> bool {
        (0..self.requested)
            .all(|transaction| self.ran.iter().any(|ran| ran.transaction == transaction))
    }
}

fn record_ran(ran: &mut Vec<Ran>, transaction: usize, signature: Signature) {
    if !ran.iter().any(|item| item.transaction == transaction) {
        ran.push(Ran {
            transaction,
            signature,
        });
        ran.sort_by_key(|item| item.transaction);
    }
}
