//! Notices: a record that something happened. Toasts and the notification
//! center are two views of the same list.

use serde::{Deserialize, Serialize};
use solana_signature::Signature;
use wallet_core::Cluster;
use wallet_vault::store::{check_saveable, load_json, LoadProblem, VaultStore};
use wallet_vault::VaultError;

use crate::lock::LockReason;

/// How long a notice is kept on disk (and in memory after a load).
pub const RETENTION_SECS: i64 = 30 * 24 * 60 * 60;
/// Hard cap; the oldest notice is dropped when a push would exceed it.
pub const MAX_NOTICES: usize = 500;

pub type NoticeId = u64;
/// The transaction log's flow id ([`crate::diagnostics::next_flow_id`]).
pub type FlowId = u64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub id: NoticeId,
    pub created: i64,
    pub updated: i64,
    pub kind: NoticeKind,
    pub toast: ToastState,
    pub read: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeKind {
    Transaction {
        flow: FlowId,
        signature: Signature,
        cluster: Cluster,
        label: String,
        status: TxStatus,
    },
    /// A browser request arrived, was answered, or was rejected.
    BridgeRequest {
        id: String,
        origin: String,
        method: String,
        outcome: BridgeRequestOutcome,
    },
    /// The vault locked (user, hidden timeout, or idle).
    VaultLocked { reason: LockReason },
    /// A background sync job failed (at most one per job until it succeeds).
    SyncFailed { job: String, error: String },
    /// A deferred store write failed; the change is still in memory.
    SaveFailed { store: String, error: String },
}

/// How a [`NoticeKind::BridgeRequest`] ended, or that it is still waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BridgeRequestOutcome {
    Waiting,
    Answered,
    Rejected,
    /// Nobody answered before the site stopped waiting.
    Expired,
}

impl BridgeRequestOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Answered => "answered",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxStatus {
    Sent,
    Confirmed { slot: u64 },
    Failed { error: String },
    Rejected { error: String },
    Expired,
    Unknown,
}

impl TxStatus {
    /// `Sent` is before every terminal status. Terminal statuses do not order
    /// against each other: once final, a later update is ignored.
    fn rank(&self) -> u8 {
        match self {
            Self::Sent => 0,
            Self::Confirmed { .. }
            | Self::Failed { .. }
            | Self::Rejected { .. }
            | Self::Expired
            | Self::Unknown => 1,
        }
    }

    /// Whether applying `next` would move the status forward.
    pub fn advances_to(&self, next: &TxStatus) -> bool {
        next.rank() > self.rank()
    }

    /// Short name for the `airlock::tx` `status` line.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Confirmed { .. } => "confirmed",
            Self::Failed { .. } => "failed",
            Self::Rejected { .. } => "rejected",
            Self::Expired => "expired",
            Self::Unknown => "unknown",
        }
    }

    /// Title-case label for notice rows, chips, and OS toasts.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Sent => "Sent",
            Self::Confirmed { .. } => "Confirmed",
            Self::Failed { .. } => "Failed",
            Self::Rejected { .. } => "Rejected",
            Self::Expired => "Expired",
            Self::Unknown => "Unknown",
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Failed { error } | Self::Rejected { error } => Some(error.as_str()),
            _ => None,
        }
    }

    pub fn slot(&self) -> Option<u64> {
        match self {
            Self::Confirmed { slot } => Some(*slot),
            _ => None,
        }
    }

    /// Whether this status is terminal (no further updates apply).
    pub fn is_final(&self) -> bool {
        self.rank() > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToastState {
    Up,
    Dismissed,
}

/// Newest first. Saved deferred to `notices.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notices {
    items: Vec<Notice>,
    next_id: NoticeId,
}

impl Notices {
    pub fn items(&self) -> &[Notice] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, id: NoticeId) -> Option<&Notice> {
        self.items.iter().find(|n| n.id == id)
    }

    /// Drop notices older than `cutoff` (unix seconds). Returns how many went.
    pub fn prune_older_than(&mut self, cutoff: i64) -> usize {
        let before = self.items.len();
        self.items.retain(|n| n.created >= cutoff);
        before - self.items.len()
    }

    /// Append a notice (newest first), trim to [`MAX_NOTICES`], return its id.
    pub fn push(&mut self, kind: NoticeKind, now: i64) -> NoticeId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.items.insert(
            0,
            Notice {
                id,
                created: now,
                updated: now,
                kind,
                toast: ToastState::Up,
                read: false,
            },
        );
        while self.items.len() > MAX_NOTICES {
            self.items.pop();
        }
        id
    }

    /// Move a transaction notice's status forward. Returns a clone of the
    /// notice when the change was applied.
    pub fn set_tx_status(
        &mut self,
        signature: &Signature,
        status: TxStatus,
        now: i64,
    ) -> Option<Notice> {
        let notice = self.items.iter_mut().find(|n| match &n.kind {
            NoticeKind::Transaction { signature: sig, .. } => sig == signature,
            _ => false,
        })?;
        let NoticeKind::Transaction {
            status: current, ..
        } = &mut notice.kind
        else {
            return None;
        };
        if !current.advances_to(&status) {
            return None;
        }
        *current = status;
        notice.updated = now;
        Some(notice.clone())
    }

    /// Record how a waiting browser request ended, on the notice pushed when it
    /// arrived. Returns whether such a notice was waiting.
    pub fn set_bridge_outcome(
        &mut self,
        request_id: &str,
        outcome: BridgeRequestOutcome,
        now: i64,
    ) -> bool {
        let Some(notice) = self.items.iter_mut().find(|n| {
            matches!(
                &n.kind,
                NoticeKind::BridgeRequest {
                    id,
                    outcome: BridgeRequestOutcome::Waiting,
                    ..
                } if id == request_id
            )
        }) else {
            return false;
        };
        if let NoticeKind::BridgeRequest {
            outcome: current, ..
        } = &mut notice.kind
        {
            *current = outcome;
        }
        notice.updated = now;
        true
    }

    /// Mark the toast dismissed. Returns whether a notice with that id existed.
    pub fn dismiss_toast(&mut self, id: NoticeId) -> bool {
        let Some(notice) = self.items.iter_mut().find(|n| n.id == id) else {
            return false;
        };
        notice.toast = ToastState::Dismissed;
        true
    }

    /// How many notices are still unread.
    pub fn unread_count(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    /// Mark the given ids read. Returns whether anything changed.
    pub fn mark_read(&mut self, ids: &[NoticeId]) -> bool {
        let mut changed = false;
        for notice in &mut self.items {
            if !notice.read && ids.contains(&notice.id) {
                notice.read = true;
                changed = true;
            }
        }
        changed
    }

    /// Mark every notice read. Returns whether anything changed.
    pub fn mark_all_read(&mut self) -> bool {
        let mut changed = false;
        for notice in &mut self.items {
            if !notice.read {
                notice.read = true;
                changed = true;
            }
        }
        changed
    }

    /// Drop one notice. A transaction still [`TxStatus::Sent`] stays, as in
    /// [`Self::clear`]. Returns whether it went.
    pub fn remove(&mut self, id: NoticeId) -> bool {
        let before = self.items.len();
        self.items.retain(|n| n.id != id || n.pinned());
        self.items.len() != before
    }

    /// Drop every notice except transactions still [`TxStatus::Sent`].
    pub fn clear(&mut self) {
        self.items.retain(Notice::pinned);
    }
}

impl Notice {
    /// A transaction still [`TxStatus::Sent`]: it stays until it ends, so its
    /// status updates have somewhere to land.
    pub fn pinned(&self) -> bool {
        matches!(
            &self.kind,
            NoticeKind::Transaction {
                status: TxStatus::Sent,
                ..
            }
        )
    }
}

/// One-line summary for a notice panel row (and its tests).
pub fn notice_row_text(notice: &Notice) -> String {
    match &notice.kind {
        NoticeKind::Transaction { label, status, .. } => {
            format!("{label} · {}", status.label())
        }
        NoticeKind::BridgeRequest {
            origin,
            method,
            outcome,
            ..
        } => {
            let action = if method == "connect" {
                "connect"
            } else {
                "sign"
            };
            let who = if origin.is_empty() { "A site" } else { origin };
            match outcome {
                BridgeRequestOutcome::Waiting => format!("{who} asks to {action}"),
                BridgeRequestOutcome::Answered => format!("{who} · {action} answered"),
                BridgeRequestOutcome::Rejected => format!("{who} · {action} rejected"),
                BridgeRequestOutcome::Expired => format!("{who} · {action} expired"),
            }
        }
        NoticeKind::VaultLocked { reason } => match reason {
            LockReason::User => "Vault locked".into(),
            LockReason::Hidden => "Vault locked · window hidden".into(),
            LockReason::Idle => "Vault locked · idle".into(),
        },
        NoticeKind::SyncFailed { job, error } => {
            format!("Sync failed · {job}: {error}")
        }
        NoticeKind::SaveFailed { store, error } => {
            format!("Save failed · {store}: {error}")
        }
    }
}

/// On-disk wrapper, same shape as holdings and the workspace.
pub struct NoticesStore<S: VaultStore> {
    store: S,
    data: Notices,
    problem: Option<LoadProblem>,
}

impl<S: VaultStore> NoticesStore<S> {
    pub fn open(store: S) -> Self {
        let (data, problem) = load_json(&store, "notices");
        Self {
            store,
            data,
            problem,
        }
    }

    pub fn load_problem(&self) -> Option<&LoadProblem> {
        self.problem.as_ref()
    }

    pub fn data(&self) -> &Notices {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Notices {
        &mut self.data
    }

    pub fn save(&self) -> Result<(), VaultError> {
        check_saveable(&self.problem)?;
        let json =
            serde_json::to_vec_pretty(&self.data).map_err(|e| VaultError::Serde(e.to_string()))?;
        self.store.save(&json)
    }

    /// Drop notices older than the retention window. Returns whether anything
    /// changed (so the caller can mark the store dirty).
    pub fn prune_expired(&mut self, now: i64) -> bool {
        self.data.prune_older_than(now - RETENTION_SECS) > 0
    }
}
