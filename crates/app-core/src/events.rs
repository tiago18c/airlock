//! Things that happen once, sent on a `broadcast` channel. A receiver that
//! falls behind skips ahead; anything it must not miss is a slice instead.

use solana_signature::Signature;

use crate::lock::LockReason;
use crate::notices::TxStatus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A deferred save failed. The change is still in memory.
    SaveFailed { store: &'static str, error: String },
    /// A background sync job failed (at most one notice per job until success).
    SyncFailed { job: String, error: String },
    /// The vault locked and its secrets were dropped.
    VaultLocked { reason: LockReason },
    /// A browser request joined the queue.
    RequestArrived { id: String },
    /// A transaction notice moved to a new status.
    TxStatusChanged {
        signature: Signature,
        status: TxStatus,
    },
}
