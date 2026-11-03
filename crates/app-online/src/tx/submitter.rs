//! The only code that sends a transaction to a node.

use std::sync::Arc;

use app_core::diagnostics::TxLog;
use app_core::{FlowId, NoticeKind, PublicCore, TxStatus};
use app_ui_kit::{SiteSendFut, SubmitFut, TxSubmit};
use solana_signature::Signature;
use tokio::sync::oneshot;
use wallet_core::{parse_wire, Cluster, SolanaRpc};
use wallet_flows::send::{self, SendFailure};
use wallet_flows::track::{watch_landed, SITE_SEND_POLL, SITE_SEND_WINDOW};

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

    /// Submit `wire` on `rpc`, with a transaction notice for it. One the node
    /// took is tracked. So is one the node gave no answer about, which may
    /// still land, and whose outcome is unknown until it does or cannot.
    pub async fn send(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        wire: &[u8],
        flow: FlowId,
        label: &str,
        cluster: &Cluster,
        last_valid_height: Option<u64>,
    ) -> Result<Signature, SendFailure> {
        let log = TxLog {
            flow,
            wire,
            cluster: Some(cluster),
        };
        let sent = send::submit(rpc.as_ref(), wire).await;
        let err = match sent {
            Ok(signature) => {
                log.log_sent(&signature.to_string());
                self.follow_sent(rpc, flow, signature, label, cluster, last_valid_height);
                return Ok(signature);
            }
            Err(err) => err,
        };
        let text = err.to_string();
        let signature = first_signature(wire);
        if let (true, Some(signature)) = (err.may_have_gone_out(), signature) {
            log.log_unanswered(&signature.to_string(), &text);
            self.follow_sent(rpc, flow, signature, label, cluster, last_valid_height);
            return Err(SendFailure::Unknown {
                signature,
                why: format!("Transaction {signature} may still land: {text}"),
            });
        }
        log.log_rejected(&text);
        // One already followed stays so: a node that refuses it now, as already
        // processed or for a blockhash it has lost, says nothing of the first try.
        if let Some(signature) = signature.filter(|signature| !self.is_followed(signature)) {
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
        Err(SendFailure::Failed(text))
    }

    /// Whether a notice is waiting for what becomes of `signature`.
    fn is_followed(&self, signature: &Signature) -> bool {
        self.core
            .slices()
            .notices
            .get()
            .items()
            .iter()
            .any(|notice| {
                matches!(
                    &notice.kind,
                    NoticeKind::Transaction { signature: known, status: TxStatus::Sent, .. }
                        if known == signature
                )
            })
    }

    /// A notice for `signature`, unless this is the same one sent again, and
    /// the tracker on it.
    fn follow_sent(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        flow: FlowId,
        signature: Signature,
        label: &str,
        cluster: &Cluster,
        last_valid_height: Option<u64>,
    ) {
        if !self.is_followed(&signature) {
            self.core.notices_push(NoticeKind::Transaction {
                flow,
                signature,
                cluster: cluster.clone(),
                label: label.to_string(),
                status: TxStatus::Sent,
            });
        }
        let deadline = self.tracker.fresh_deadline();
        self.tracker
            .follow(rpc, cluster, signature, last_valid_height, deadline);
    }
}

impl Submitter {
    /// Watch the node for the transactions in `returned`, each with its flow,
    /// which went back to `origin`, a site that may send them itself. One
    /// that lands gets a transaction notice and is tracked as if sent here;
    /// one that does not gets a notice saying so. Answers whether each
    /// signed one landed.
    pub async fn watch_site_send(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        returned: &[(Vec<u8>, FlowId)],
        origin: &str,
        cluster: &Cluster,
    ) -> Vec<(Signature, bool)> {
        let watched: Vec<(Signature, FlowId, &[u8])> = returned
            .iter()
            .filter_map(|(wire, flow)| Some((first_signature(wire)?, *flow, wire.as_slice())))
            .collect();
        let signatures: Vec<Signature> = watched.iter().map(|(signature, ..)| *signature).collect();
        let clock = self.core.ports().clock.clone();
        let landed = watch_landed(
            rpc.as_ref(),
            &signatures,
            SITE_SEND_WINDOW,
            SITE_SEND_POLL,
            |every| clock.sleep(every),
        )
        .await;
        for (&(signature, flow, wire), &landed) in watched.iter().zip(&landed) {
            let (label, status) = if landed {
                TxLog {
                    flow,
                    wire,
                    cluster: Some(cluster),
                }
                .log_sent(&signature.to_string());
                (format!("Sent by {origin}"), TxStatus::Sent)
            } else {
                (format!("Returned to {origin}"), TxStatus::NotSent)
            };
            self.core.notices_push(NoticeKind::Transaction {
                flow,
                signature,
                cluster: cluster.clone(),
                label,
                status,
            });
            if landed {
                let deadline = self.tracker.fresh_deadline();
                self.tracker
                    .follow(rpc.clone(), cluster, signature, None, deadline);
            }
        }
        signatures.into_iter().zip(landed).collect()
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
                .unwrap_or_else(|_| Err(SendFailure::Failed("the submit task stopped".into())))
        })
    }

    fn watch_site_send(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        returned: Vec<(Vec<u8>, FlowId)>,
        origin: String,
        cluster: Cluster,
    ) -> SiteSendFut {
        // On the core's spawner, like a send: the notices come whether or
        // not the screen that asked is still open.
        let this = self.clone();
        let (reply, answer) = oneshot::channel();
        self.core.ports().spawner.spawn(Box::pin(async move {
            let landed = this
                .watch_site_send(rpc, &returned, &origin, &cluster)
                .await;
            let _ = reply.send(landed);
        }));
        Box::pin(async move { answer.await.unwrap_or_default() })
    }
}

fn first_signature(wire: &[u8]) -> Option<Signature> {
    parse_wire(wire).ok()?.signature()
}
