//! The network monitor: probes the cluster and refreshes chain state.
//!
//! Probes and background refreshes run on the core's [`Spawner`](app_core::Spawner),
//! not in a Dioxus scope, so they keep going while the window is hidden. They
//! publish [`NetworkStatus`](app_core::NetworkStatus) through the core and log
//! under `airlock::network`.

use app_core::{PublicCore, Reach};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::Instrument;
use wallet_core::{Cluster, RpcError, SolanaRpc};

type SharedRpc = Arc<Mutex<Option<(String, Arc<dyn SolanaRpc>)>>>;
type OnOnline = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

/// Shared handle for probing and talking to the current cluster.
///
/// Cloneable and `Send`, so work can run on the core's spawner.
#[derive(Clone)]
pub struct Monitor {
    core: PublicCore,
    probe_seq: Arc<AtomicU64>,
    rpc: SharedRpc,
    /// Fired after a successful probe, or when asked to refresh, so the sync
    /// scheduler reads the chain again.
    on_online: OnOnline,
}

impl Monitor {
    pub fn new(core: PublicCore) -> Self {
        Self {
            core,
            probe_seq: Arc::new(AtomicU64::new(0)),
            rpc: Arc::new(Mutex::new(None)),
            on_online: Arc::new(Mutex::new(None)),
        }
    }

    /// What to run after the node answers (balances sync kick).
    pub fn set_on_online(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.on_online.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(f));
    }

    /// Probe `getGenesisHash` on the current cluster. A newer probe, or a
    /// cluster change while this one runs, drops the answer.
    pub fn start_detect(&self) {
        let mine = self
            .probe_seq
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        let monitor = self.clone();
        self.core.ports().spawner.spawn(Box::pin(async move {
            monitor.detect_and_sync(mine).await;
        }));
    }

    /// Read what changed after a wallet or an address was added: kicks the
    /// balances sync job. A node that did not answer is asked again first.
    pub fn start_refresh(&self) {
        let status = self.core.slices().network.get();
        if status.reach == Reach::Unreachable {
            self.start_detect();
            return;
        }
        self.kick();
    }

    fn kick(&self) {
        let kick = self
            .on_online
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(kick) = kick {
            kick();
        }
    }

    /// The client for the current cluster, shared by everything that asks
    /// the node. A node that is down answers each call with its own error.
    pub fn rpc(&self) -> Result<Arc<dyn SolanaRpc>, RpcError> {
        let url = self
            .core
            .slices()
            .network
            .get()
            .cluster
            .rpc_url()
            .to_string();
        let mut kept = self
            .rpc
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((_, rpc)) = kept.as_ref().filter(|(kept_url, _)| kept_url == &url) {
            return Ok(rpc.clone());
        }
        let rpc: Arc<dyn SolanaRpc> = Arc::new(wallet_rpc::Rpc::connect(&url));
        *kept = Some((url, rpc.clone()));
        Ok(rpc)
    }

    async fn detect_and_sync(&self, seq: u64) {
        self.core.set_reach(Reach::Probing);
        let current = self.core.slices().network.get().cluster.clone();
        let url = current.rpc_url().to_string();
        let redacted = app_core::diagnostics::redact_url(&url);
        let span = tracing::info_span!(
            target: "airlock::network",
            "probe",
            cluster = %current.name(),
            rpc = %redacted,
        );
        // Instrumented, not entered: an entered guard held across the await
        // would tag whatever else runs on this thread meanwhile as the probe.
        self.probe(seq, current, url, redacted)
            .instrument(span)
            .await;
    }

    async fn probe(&self, seq: u64, current: Cluster, url: String, redacted: String) {
        tracing::info!(
            target: "airlock::network",
            cluster = %current.name(),
            rpc = %redacted,
            "probe start"
        );
        let answer = wallet_rpc::Rpc::probe(&url).await;
        let latest = self.core.slices().network.get();
        if self.probe_seq.load(Ordering::SeqCst) != seq || latest.cluster != current {
            tracing::debug!(
                target: "airlock::network",
                cluster = %current.name(),
                "probe discarded (stale)"
            );
            return;
        }
        match answer {
            Ok(hash) => {
                self.core
                    .workspace_deferred("node_answered", |ws| ws.node_answered(&hash));
                self.core.set_reach(Reach::Answered);
                tracing::info!(
                    target: "airlock::network",
                    cluster = %self.core.slices().network.get().cluster.name(),
                    rpc = %redacted,
                    "probe ok"
                );
                self.kick();
            }
            Err(err) => {
                self.core.set_reach(Reach::Unreachable);
                tracing::warn!(
                    target: "airlock::network",
                    cluster = %current.name(),
                    rpc = %redacted,
                    error = %err,
                    "probe failed"
                );
            }
        }
    }
}

/// Build an RPC client for a cluster URL (tracker resume path).
pub fn rpc_for_cluster(cluster: &Cluster) -> Option<Arc<dyn SolanaRpc>> {
    Some(Arc::new(wallet_rpc::Rpc::connect(cluster.rpc_url())))
}
