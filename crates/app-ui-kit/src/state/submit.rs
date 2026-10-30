//! Handle the online shell plugs in so Sign can send a transaction.
//!
//! The concrete submitter lives in `app-online` (it owns tracking). Airgap
//! uses [`SubmitHandle::none`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use app_core::FlowId;
use solana_signature::Signature;
use wallet_core::{Cluster, RpcError, SolanaRpc};

pub type SubmitFut = Pin<Box<dyn Future<Output = Result<Signature, RpcError>>>>;
/// Whether each signed transaction handed back to a site landed while it
/// was watched.
pub type SiteSendFut = Pin<Box<dyn Future<Output = Vec<(Signature, bool)>>>>;

/// What the online shell implements with its submitter.
pub trait TxSubmit: Send + Sync {
    fn submit(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        wire: Vec<u8>,
        flow: FlowId,
        label: String,
        cluster: Cluster,
        last_valid_height: Option<u64>,
    ) -> SubmitFut;

    /// Watch for a site sending the transactions it was given back, each
    /// with its flow, and tell the user what came of each.
    fn watch_site_send(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        returned: Vec<(Vec<u8>, FlowId)>,
        origin: String,
        cluster: Cluster,
    ) -> SiteSendFut;
}

/// Cloneable handle stored on [`Session`](super::session::Session).
#[derive(Clone)]
pub struct SubmitHandle {
    inner: Arc<dyn TxSubmit>,
}

impl SubmitHandle {
    pub fn new(inner: impl TxSubmit + 'static) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Never sends: every submit is offline.
    pub fn none() -> Self {
        Self::new(NoSubmit)
    }

    pub fn submit(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        wire: &[u8],
        flow: FlowId,
        label: &str,
        cluster: &Cluster,
        last_valid_height: Option<u64>,
    ) -> SubmitFut {
        self.inner.submit(
            rpc,
            wire.to_vec(),
            flow,
            label.to_string(),
            cluster.clone(),
            last_valid_height,
        )
    }

    pub fn watch_site_send(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        returned: Vec<(Vec<u8>, FlowId)>,
        origin: &str,
        cluster: &Cluster,
    ) -> SiteSendFut {
        self.inner
            .watch_site_send(rpc, returned, origin.to_string(), cluster.clone())
    }
}

struct NoSubmit;

impl TxSubmit for NoSubmit {
    fn submit(
        &self,
        _rpc: Arc<dyn SolanaRpc>,
        _wire: Vec<u8>,
        _flow: FlowId,
        _label: String,
        _cluster: Cluster,
        _last_valid_height: Option<u64>,
    ) -> SubmitFut {
        Box::pin(async { Err(RpcError::Offline) })
    }

    fn watch_site_send(
        &self,
        _rpc: Arc<dyn SolanaRpc>,
        _returned: Vec<(Vec<u8>, FlowId)>,
        _origin: String,
        _cluster: Cluster,
    ) -> SiteSendFut {
        Box::pin(async { Vec::new() })
    }
}
