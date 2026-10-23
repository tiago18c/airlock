//! The only code that sends a transaction to a node.

use std::sync::Arc;

use app_core::diagnostics::TxLog;
use app_core::{FlowId, NoticeKind, PublicCore, TxStatus};
use app_ui_kit::{SubmitFut, TxSubmit};
use solana_signature::Signature;
use tokio::sync::oneshot;
use wallet_core::{parse_wire, Cluster, RpcError, SolanaRpc, SIGNATURE_BYTES};
use wallet_flows::send;

use super::tracker::Tracker;

/// Sends a wire, logs it, pushes a notice, and hands the signature to the tracker.
#[derive(Clone)]
pub struct Submitter {
    core: PublicCore,
    tracker: Tracker,
}

impl Submitter {
    pub fn new(core: PublicCore, tracker: Tracker) -> Self {
        Self { core, tracker }
    }

    /// Submit `wire` on `rpc`. On success (or reject with a signature in the
    /// wire), pushes a transaction notice and tracks a successful send.
    pub async fn send(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        wire: &[u8],
        flow: FlowId,
        label: &str,
        cluster: &Cluster,
        last_valid_height: Option<u64>,
    ) -> Result<Signature, RpcError> {
        match send::submit(rpc.as_ref(), wire).await {
            Ok(signature) => {
                TxLog {
                    flow,
                    wire,
                    cluster: Some(cluster),
                }
                .log_sent(&signature.to_string());
                self.core.notices_push(NoticeKind::Transaction {
                    flow,
                    signature,
                    cluster: cluster.clone(),
                    label: label.to_string(),
                    status: TxStatus::Sent,
                });
                let deadline = self.tracker.fresh_deadline();
                self.tracker
                    .follow(rpc, cluster, signature, last_valid_height, deadline);
                Ok(signature)
            }
            Err(err) => {
                let text = err.to_string();
                TxLog {
                    flow,
                    wire,
                    cluster: Some(cluster),
                }
                .log_rejected(&text);
                if let Some(signature) = first_signature(wire) {
                    self.core.notices_push(NoticeKind::Transaction {
                        flow,
                        signature,
                        cluster: cluster.clone(),
                        label: label.to_string(),
                        status: TxStatus::Rejected {
                            error: text.clone(),
                        },
                    });
                }
                Err(err)
            }
        }
    }
}

impl TxSubmit for Submitter {
    fn submit(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        wire: Vec<u8>,
        flow: FlowId,
        label: String,
        cluster: Cluster,
        last_valid_height: Option<u64>,
    ) -> SubmitFut {
        // On the core's spawner, not in the caller's scope: a screen that
        // closes while the node answers would otherwise drop the send, and a
        // transaction that landed would get no notice and no tracking.
        let this = self.clone();
        let (reply, answer) = oneshot::channel();
        self.core.ports().spawner.spawn(Box::pin(async move {
            let sent = this
                .send(rpc, &wire, flow, &label, &cluster, last_valid_height)
                .await;
            let _ = reply.send(sent);
        }));
        Box::pin(async move {
            answer
                .await
                .unwrap_or_else(|_| Err(RpcError::Message("the submit task stopped".into())))
        })
    }
}

fn first_signature(wire: &[u8]) -> Option<Signature> {
    let view = parse_wire(wire).ok()?;
    let offset = *view.signature_offsets.first()?;
    if !view.slot_valid(0) {
        return None;
    }
    let bytes = view.wire.get(offset..offset + SIGNATURE_BYTES)?;
    Signature::try_from(bytes).ok()
}
