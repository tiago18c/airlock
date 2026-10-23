//! Follow a sent signature to confirmed, failed, expired, or unknown.
//!
//! Every signature sent to the same node is polled together: one status
//! request per poll for all of them, and one block height when one of them
//! can expire.
//!
//! The shell passes `rpc_for` when resuming (it alone connects via `wallet-rpc`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use app_core::{NoticeKind, PublicCore, TxStatus};
use solana_signature::Signature;
use tracing::info;
use wallet_core::{Cluster, SolanaRpc};
use wallet_flows::track::{self, TrackResult, POLL_INTERVAL, TRACK_TIMEOUT};

/// Told the cluster of a followed transaction that landed, and its slot
/// when it landed without failing.
type OnLanded = Arc<Mutex<Option<Arc<dyn Fn(&Cluster, Option<u64>) + Send + Sync>>>>;

/// Polls `getSignatureStatuses` on the RPC the transaction was sent to.
#[derive(Clone)]
pub struct Tracker {
    core: PublicCore,
    /// What is followed on each node, by the address of its client.
    nodes: Arc<Mutex<HashMap<usize, Node>>>,
    on_landed: OnLanded,
}

/// The signatures followed on one node. Its poll loop runs while there are any.
struct Node {
    rpc: Arc<dyn SolanaRpc>,
    cluster: Cluster,
    followed: Vec<Followed>,
}

#[derive(Clone, Copy)]
struct Followed {
    signature: Signature,
    last_valid_height: Option<u64>,
    deadline_ms: u64,
}

/// One client is one node: the key of what is followed on it.
fn node_key(rpc: &Arc<dyn SolanaRpc>) -> usize {
    Arc::as_ptr(rpc).cast::<()>() as usize
}

impl Tracker {
    pub fn new(core: PublicCore) -> Self {
        Self {
            core,
            nodes: Arc::new(Mutex::new(HashMap::new())),
            on_landed: Arc::new(Mutex::new(None)),
        }
    }

    /// What to run when a followed transaction lands, failed or not, so the
    /// balances it moved are read again.
    pub fn set_on_landed(&self, f: impl Fn(&Cluster, Option<u64>) + Send + Sync + 'static) {
        *self
            .on_landed
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(f));
    }

    /// Re-track every notice still in [`TxStatus::Sent`], using the remaining
    /// time until the five-minute unknown deadline. Notices on the same
    /// cluster share one client, so they are polled together.
    pub fn resume(
        &self,
        rpc_for: impl Fn(&Cluster) -> Option<Arc<dyn SolanaRpc>> + Send + Sync + 'static,
    ) {
        let notices = self.core.slices().notices.get();
        let now_ms = self.core.ports().clock.now_ms();
        let mut clients: Vec<(Cluster, Arc<dyn SolanaRpc>)> = Vec::new();
        for notice in notices.items() {
            let NoticeKind::Transaction {
                signature,
                cluster,
                status: TxStatus::Sent,
                ..
            } = &notice.kind
            else {
                continue;
            };
            let created_ms = (notice.created as u64).saturating_mul(1000);
            let deadline_ms = created_ms.saturating_add(TRACK_TIMEOUT.as_millis() as u64);
            if now_ms >= deadline_ms {
                self.core
                    .notices_set_tx_status(*signature, TxStatus::Unknown);
                continue;
            }
            let rpc = match clients.iter().find(|(known, _)| known == cluster) {
                Some((_, rpc)) => rpc.clone(),
                None => {
                    let Some(rpc) = rpc_for(cluster) else {
                        continue;
                    };
                    clients.push((cluster.clone(), rpc.clone()));
                    rpc
                }
            };
            info!(
                target: "airlock::tx",
                signature = %signature,
                cluster = %cluster.name(),
                "tracking resumed"
            );
            self.follow(rpc, cluster, *signature, None, deadline_ms);
        }
    }

    /// Poll `signature` on `rpc`, a node of `cluster`, until a terminal
    /// status or `deadline_ms`.
    pub fn follow(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        cluster: &Cluster,
        signature: Signature,
        last_valid_height: Option<u64>,
        deadline_ms: u64,
    ) {
        let key = node_key(&rpc);
        let followed = Followed {
            signature,
            last_valid_height,
            deadline_ms,
        };
        {
            let mut nodes = self.nodes.lock().unwrap_or_else(PoisonError::into_inner);
            let already = nodes
                .values()
                .any(|node| node.followed.iter().any(|f| f.signature == signature));
            if already {
                return;
            }
            if let Some(node) = nodes.get_mut(&key) {
                // Its loop is running and takes this one on its next poll.
                node.followed.push(followed);
                return;
            }
            nodes.insert(
                key,
                Node {
                    rpc,
                    cluster: cluster.clone(),
                    followed: vec![followed],
                },
            );
        }
        let tracker = self.clone();
        self.core.ports().spawner.spawn(Box::pin(async move {
            tracker.poll_node(key).await;
        }));
    }

    /// Poll everything followed on the node at `key`, every
    /// [`POLL_INTERVAL`], until nothing is left.
    async fn poll_node(&self, key: usize) {
        let clock = self.core.ports().clock.clone();
        loop {
            let Some((rpc, watched)) = self.due(key, clock.now_ms()) else {
                return;
            };
            if let Ok(results) = track::poll_many(rpc.as_ref(), &watched).await {
                let done: Vec<(Signature, TrackResult)> = watched
                    .iter()
                    .zip(results)
                    .filter_map(|((signature, _), result)| Some((*signature, result?)))
                    .collect();
                for (signature, result) in done {
                    self.finish(key, signature, status_of(result));
                }
            }
            if self.is_idle(key) {
                return;
            }
            clock.sleep(POLL_INTERVAL).await;
        }
    }

    /// What to poll on the node at `key` now. Each signature past its
    /// deadline is unknown from here. `None` once nothing is left, which
    /// ends the node's loop.
    #[allow(clippy::type_complexity)]
    fn due(
        &self,
        key: usize,
        now_ms: u64,
    ) -> Option<(Arc<dyn SolanaRpc>, Vec<(Signature, Option<u64>)>)> {
        let mut expired = Vec::new();
        let due = {
            let mut nodes = self.nodes.lock().unwrap_or_else(PoisonError::into_inner);
            let node = nodes.get_mut(&key)?;
            node.followed.retain(|f| {
                let live = now_ms < f.deadline_ms;
                if !live {
                    expired.push(f.signature);
                }
                live
            });
            if node.followed.is_empty() {
                nodes.remove(&key);
                None
            } else {
                Some((
                    node.rpc.clone(),
                    node.followed
                        .iter()
                        .map(|f| (f.signature, f.last_valid_height))
                        .collect(),
                ))
            }
        };
        for signature in expired {
            self.core
                .notices_set_tx_status(signature, TxStatus::Unknown);
        }
        due
    }

    /// Whether the node at `key` has nothing left to poll. Ends its loop
    /// when so, so a later `follow` starts a new one.
    fn is_idle(&self, key: usize) -> bool {
        let mut nodes = self.nodes.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(node) = nodes.get(&key) else {
            return true;
        };
        if node.followed.is_empty() {
            nodes.remove(&key);
            return true;
        }
        false
    }

    fn finish(&self, key: usize, signature: Signature, status: TxStatus) {
        let cluster = {
            let mut nodes = self.nodes.lock().unwrap_or_else(PoisonError::into_inner);
            nodes.get_mut(&key).map(|node| {
                node.followed.retain(|f| f.signature != signature);
                node.cluster.clone()
            })
        };
        let landed = match status {
            TxStatus::Confirmed { slot } => Some(Some(slot)),
            TxStatus::Failed { .. } => Some(None),
            _ => None,
        };
        self.core.notices_set_tx_status(signature, status);
        let on_landed = self
            .on_landed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let (Some(cluster), Some(slot), Some(on_landed)) = (cluster, landed, on_landed) {
            on_landed(&cluster, slot);
        }
    }

    /// Deadline for a freshly sent transaction: now plus [`TRACK_TIMEOUT`].
    pub fn fresh_deadline(&self) -> u64 {
        self.core
            .ports()
            .clock
            .now_ms()
            .saturating_add(TRACK_TIMEOUT.as_millis() as u64)
    }
}

fn status_of(result: TrackResult) -> TxStatus {
    match result {
        TrackResult::Confirmed { slot } => TxStatus::Confirmed { slot },
        TrackResult::Failed { error } => TxStatus::Failed { error },
        TrackResult::Expired => TxStatus::Expired,
        TrackResult::Unknown => TxStatus::Unknown,
    }
}
