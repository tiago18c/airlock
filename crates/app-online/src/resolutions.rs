//! What becomes of a resolution after the person approved it, when the
//! window has nothing more to do with it (`docs/intents.md`):
//!
//! - **Sending.** A resolution whose last signature came from elsewhere
//!   (the site's own, through `provideSignatures`, or another device's) is
//!   `sending` with its wires held in the core. This sends it through the
//!   [`Submitter`], as a site's send goes, and records what came of each
//!   transaction.
//! - **Returned.** A transaction handed back to its site, signed by a fee
//!   payer of this device, is watched for on the node for a few minutes. One
//!   seen to land or fail is recorded; one never seen stays `returned`.
//! - **A proposal that waits.** A resolution whose proposal landed and has
//!   not run is followed by the [`RESOLUTIONS_JOB`] sync job until it runs
//!   or closes.
//!
//! All of it runs on the core's spawner, never in a screen's scope, and
//! reads and writes the core only through [`PublicCore`].

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use app_core::diagnostics::{held_flow, next_flow_id, TxLog};
use app_core::resolutions::{KeptResolution, Resolutions};
use app_core::PublicCore;
use solana_signature::Signature;
use tracing::{debug, info};
use wallet_core::intent::{Order, SendResult, State, TxRole, TxState};
use wallet_core::{ConfirmationLevel, SolanaRpc};
use wallet_flows::intent::{follow_proposal, ProposalOutcome};
use wallet_flows::send::{
    send_each, wait_for_commitment, SendFailure, SendPlan, COMMITMENT_POLL, COMMITMENT_TIMEOUT,
    NOT_SENT,
};
use wallet_flows::track::{watch_outcomes, TrackResult, RETURNED_POLL, RETURNED_WINDOW};

use crate::sync::{Cancelled, JobCtx, JobFn, JobOk, JobResult, Scheduler};
use crate::tx::Submitter;

const BRIDGE: &str = "airlock::bridge";

/// The sync job that follows the proposals resolutions wait on.
pub const RESOLUTIONS_JOB: &str = "resolutions";

/// How often a proposal a site waits on is asked about, while one waits. A
/// transaction landing and the Proposals pane reading kick it sooner.
pub const PROPOSAL_INTERVAL: Duration = Duration::from_secs(20);

/// How the service reaches a node: usually [`Monitor::rpc`](crate::Monitor::rpc).
pub type RpcFn = Arc<dyn Fn() -> Result<Arc<dyn SolanaRpc>, wallet_core::RpcError> + Send + Sync>;

/// Start sending what is ready and watching what was returned, for as long
/// as the app runs, and register the proposal job on `scheduler`.
pub fn start_resolutions(
    core: PublicCore,
    submitter: Submitter,
    scheduler: &Scheduler,
    rpc: RpcFn,
) {
    register_proposal_job(scheduler, rpc.clone());
    let service = Service {
        core: core.clone(),
        submitter,
        rpc,
        watched: Arc::new(Mutex::new(BTreeSet::new())),
        scheduler: scheduler.clone(),
    };
    core.ports().spawner.spawn(Box::pin(async move {
        service.run().await;
    }));
}

#[derive(Clone)]
struct Service {
    core: PublicCore,
    submitter: Submitter,
    rpc: RpcFn,
    /// Returned resolutions being watched for, by origin and id, so a change
    /// to the slice does not start a second watch.
    watched: Arc<Mutex<BTreeSet<(String, String)>>>,
    scheduler: Scheduler,
}

impl Service {
    async fn run(self) {
        let mut resolutions = self.core.slices().resolutions.subscribe();
        let mut network = self.core.slices().network.subscribe();
        let mut waiting_before = 0;
        loop {
            let now = resolutions.borrow_and_update().clone();
            network.borrow_and_update();
            self.send_ready(&now);
            self.watch_returned(&now);
            // A proposal that has just landed is asked about at once.
            let waiting = waiting_proposals(&now).count();
            if waiting > waiting_before {
                self.scheduler.kick(RESOLUTIONS_JOB);
            }
            waiting_before = waiting;
            tokio::select! {
                changed = resolutions.changed() => if changed.is_err() { return },
                // A network the resolution's chain matches again, or a node
                // that answers again, can send what had to wait.
                changed = network.changed() => if changed.is_err() { return },
            }
        }
    }

    /// Send every resolution that is ready, on a network its chain matches.
    /// One on another network waits for the user to change it.
    fn send_ready(&self, resolutions: &Resolutions) {
        let cluster = self.core.slices().network.get().cluster.clone();
        for kept in resolutions.ready_to_send() {
            if cluster.differs_from_chain(&kept.resolution.chain) {
                debug!(
                    target: BRIDGE,
                    origin = %kept.origin,
                    id = %kept.resolution.id,
                    chain = %kept.resolution.chain,
                    "resolution waits for its network"
                );
                continue;
            }
            let Ok(rpc) = (self.rpc)() else {
                continue;
            };
            let Some(wires) = self
                .core
                .resolution_begin_send(&kept.origin, &kept.resolution.id)
            else {
                continue;
            };
            let this = self.clone();
            let (origin, id, order) = (
                kept.origin.clone(),
                kept.resolution.id.clone(),
                kept.resolution.order,
            );
            let cluster = cluster.clone();
            self.core.ports().spawner.spawn(Box::pin(async move {
                let results = this.send(rpc, &origin, &id, order, wires, &cluster).await;
                let _ = this.core.resolution_sent(&origin, &id, results);
            }));
        }
    }

    /// Send `wires`, the transactions of resolution `id` of `origin`, as its
    /// `order` asks: each waited for until it is confirmed, so what the
    /// resolution says landed has.
    async fn send(
        &self,
        rpc: Arc<dyn SolanaRpc>,
        origin: &str,
        id: &str,
        order: Order,
        wires: Vec<Vec<u8>>,
        cluster: &wallet_core::Cluster,
    ) -> Vec<SendResult> {
        let plan = send_plan(order);
        // The site's signatures came in through the bridge: what goes out
        // is logged as signed again, under the flow the window signed it
        // in, or one of its own when that is not known (after it dropped
        // out of the few kept).
        let flows: Vec<u64> = wires
            .iter()
            .map(|wire| {
                let flow = held_flow(wire).unwrap_or_else(next_flow_id);
                TxLog {
                    flow,
                    wire,
                    cluster: Some(cluster),
                }
                .log_signed("site");
                flow
            })
            .collect();
        info!(target: BRIDGE, origin = %origin, id = %id, count = wires.len(), "resolution sending");
        let commitments = vec![Some(ConfirmationLevel::Confirmed); wires.len()];
        let label = format!("Sent for {origin}");
        let clock = self.core.ports().clock.clone();
        let results = send_each(
            plan,
            &commitments,
            |index| {
                let submitter = self.submitter.clone();
                let rpc = rpc.clone();
                let wire = wires[index].clone();
                let flow = flows[index];
                let label = label.clone();
                let cluster = cluster.clone();
                async move {
                    submitter
                        .send(rpc, &wire, flow, &label, &cluster, None)
                        .await
                }
            },
            |signature, level| {
                let rpc = rpc.clone();
                let clock = clock.clone();
                async move {
                    wait_for_commitment(
                        rpc.as_ref(),
                        signature,
                        level,
                        COMMITMENT_TIMEOUT,
                        COMMITMENT_POLL,
                        || clock.now_ms(),
                        |every| clock.sleep(every),
                    )
                    .await
                }
            },
        )
        .await;
        results.into_iter().map(resolution_result).collect()
    }

    /// Watch for each returned resolution not watched yet.
    fn watch_returned(&self, resolutions: &Resolutions) {
        for kept in resolutions.items() {
            if kept.resolution.state != State::Returned {
                continue;
            }
            let key = (kept.origin.clone(), kept.resolution.id.clone());
            {
                let mut watched = self.watched.lock().unwrap_or_else(PoisonError::into_inner);
                if !watched.insert(key) {
                    continue;
                }
            }
            let Ok(rpc) = (self.rpc)() else {
                continue;
            };
            let this = self.clone();
            let kept = kept.clone();
            self.core.ports().spawner.spawn(Box::pin(async move {
                this.watch_one(rpc, kept).await;
            }));
        }
    }

    /// Watch the node for the transactions of a returned resolution whose
    /// fee payer signature it holds, and record what became of them: once
    /// each has settled, or at the end of the window when some have. One
    /// never seen leaves it `returned`.
    async fn watch_one(&self, rpc: Arc<dyn SolanaRpc>, kept: KeptResolution) {
        let signatures: Vec<Option<Signature>> = kept
            .resolution
            .transactions
            .iter()
            .map(|made| made.signature)
            .collect();
        let watched: Vec<Signature> = signatures.iter().flatten().copied().collect();
        if watched.is_empty() {
            return;
        }
        let clock = self.core.ports().clock.clone();
        let settled = watch_outcomes(
            rpc.as_ref(),
            &watched,
            RETURNED_WINDOW,
            RETURNED_POLL,
            |every| clock.sleep(every),
        )
        .await;
        let Some(results) = returned_results(&signatures, &settled) else {
            info!(
                target: BRIDGE,
                origin = %kept.origin,
                id = %kept.resolution.id,
                "returned resolution not seen on chain"
            );
            return;
        };
        let _ = self
            .core
            .resolution_sent(&kept.origin, &kept.resolution.id, results);
    }
}

/// What a returned resolution's watch found, one result for each of its
/// transactions, given each transaction's signature (`None` for one this
/// device did not pay for) and what the watch settled for each signature
/// it watched, in order. `None` when it saw none of them: it stays
/// `returned`. One it did not settle may still land: `unknown`.
pub fn returned_results(
    signatures: &[Option<Signature>],
    settled: &[Option<TrackResult>],
) -> Option<Vec<SendResult>> {
    if settled.iter().all(Option::is_none) {
        return None;
    }
    let mut outcomes = settled.iter();
    Some(
        signatures
            .iter()
            .map(|signature| {
                let outcome = signature.and_then(|_| outcomes.next().cloned().flatten());
                match (signature, outcome) {
                    (Some(signature), Some(TrackResult::Confirmed { .. })) => SendResult::Landed {
                        signature: *signature,
                    },
                    (Some(signature), Some(TrackResult::Failed { error })) => SendResult::Failed {
                        signature: Some(*signature),
                        reason: error,
                    },
                    (signature, _) => SendResult::Unknown {
                        signature: *signature,
                    },
                }
            })
            .collect(),
    )
}

/// How a resolution's `order` goes out, as the equivalent site sends do.
pub fn send_plan(order: Order) -> SendPlan {
    match order {
        Order::Serial => SendPlan::Serial,
        Order::Any => SendPlan::Independent,
        Order::Atomic => SendPlan::InOrder,
    }
}

/// What sending one transaction came to, as a resolution takes it. Each was
/// waited for until confirmed, so one that went out has landed.
fn resolution_result(result: Result<Signature, SendFailure>) -> SendResult {
    match result {
        Ok(signature) => SendResult::Landed { signature },
        Err(SendFailure::Failed(reason)) if reason == NOT_SENT => SendResult::NotSent { reason },
        Err(SendFailure::Failed(reason)) => SendResult::Failed {
            signature: None,
            reason,
        },
        Err(SendFailure::Unknown { signature, .. }) => SendResult::Unknown {
            signature: Some(signature),
        },
    }
}

/// Resolutions waiting on a proposal that landed: each with the index of
/// that transaction among its own and the wire it holds.
fn waiting_proposals(
    resolutions: &Resolutions,
) -> impl Iterator<Item = (&KeptResolution, usize, &[u8])> {
    resolutions
        .items()
        .iter()
        .filter(|kept| kept.resolution.state == State::AwaitingSignatures)
        .flat_map(|kept| {
            kept.resolution
                .transactions
                .iter()
                .enumerate()
                .filter(|(_, made)| made.role == TxRole::Proposal && made.state == TxState::Landed)
                .filter_map(move |(index, _)| {
                    kept.resolution
                        .held
                        .get(index)
                        .map(|wire| (kept, index, wire.as_slice()))
                })
        })
}

fn register_proposal_job(scheduler: &Scheduler, rpc: RpcFn) {
    let work: JobFn = Arc::new(move |ctx| {
        let rpc = rpc.clone();
        Box::pin(async move { follow_proposals(ctx, rpc).await })
    });
    scheduler.register(RESOLUTIONS_JOB, PROPOSAL_INTERVAL, work);
}

/// Ask about each proposal a resolution waits on, and record the ones that
/// ran or closed. Nothing to ask is a run that does nothing. A node that
/// does not answer for one leaves it for the next run.
pub async fn follow_proposals(ctx: JobCtx, rpc: RpcFn) -> JobResult {
    let resolutions = ctx.core.slices().resolutions.get();
    let waiting: Vec<(String, String, usize, Vec<u8>)> = waiting_proposals(&resolutions)
        .map(|(kept, index, wire)| {
            (
                kept.origin.clone(),
                kept.resolution.id.clone(),
                index,
                wire.to_vec(),
            )
        })
        .collect();
    if waiting.is_empty() {
        return Ok(JobOk::default());
    }
    let rpc = rpc().map_err(|err| err.to_string())?;
    let mut unanswered = 0;
    for (origin, id, index, wire) in waiting {
        let outcome = ctx
            .run_cancellable(follow_proposal(rpc.as_ref(), &wire))
            .await
            .map_err(|Cancelled| "sync job was cancelled.".to_string())?;
        if ctx.is_cancelled() {
            return Err("sync job was cancelled.".into());
        }
        match outcome {
            Ok(ProposalOutcome::Executed {
                signature: Some(signature),
            }) => {
                let _ = ctx.core.resolution_executed(&origin, &id, index, signature);
            }
            Ok(ProposalOutcome::Rejected) => {
                let _ = ctx.core.resolution_proposal_closed(
                    &origin,
                    &id,
                    "The multisig's members rejected the proposal, so what it holds will not run."
                        .into(),
                );
            }
            Ok(ProposalOutcome::Cancelled) => {
                let _ = ctx.core.resolution_proposal_closed(
                    &origin,
                    &id,
                    "The proposal was cancelled, so what it holds will not run.".into(),
                );
            }
            // Executed, with the transaction it ran in not found yet: the
            // next run looks again.
            Ok(_) => {}
            Err(err) => {
                debug!(target: BRIDGE, origin = %origin, id = %id, error = %err, "proposal not read");
                unanswered += 1;
            }
        }
    }
    Ok(JobOk {
        partial: (unanswered > 0)
            .then(|| format!("{unanswered} proposal(s) a site waits on could not be read.")),
        ..JobOk::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_core::{KeypairSigner, WalletSigner};
    use wallet_flows::testing::pk;

    fn sig(n: u8) -> Signature {
        Signature::from([n; 64])
    }

    #[test]
    fn a_returned_transaction_never_seen_leaves_it_returned() {
        assert_eq!(returned_results(&[Some(sig(1))], &[None]), None);
    }

    #[test]
    fn a_returned_watch_records_what_ran_what_failed_and_what_may_still_land() {
        let results = returned_results(
            &[Some(sig(1)), None, Some(sig(2)), Some(sig(3))],
            &[
                Some(TrackResult::Confirmed { slot: 1 }),
                Some(TrackResult::Failed {
                    error: "InstructionError".into(),
                }),
                None,
            ],
        )
        .unwrap();
        assert_eq!(
            results,
            vec![
                SendResult::Landed { signature: sig(1) },
                // Paid for elsewhere: not watched, and it may still land.
                SendResult::Unknown { signature: None },
                SendResult::Failed {
                    signature: Some(sig(2)),
                    reason: "InstructionError".into()
                },
                SendResult::Unknown {
                    signature: Some(sig(3))
                },
            ]
        );
    }

    /// A `cosign` resolution from `ORIGIN` for one transaction `wallet`
    /// pays for and `app` also signs, signed by the wallet and queued.
    fn cosigned(kit: &app_core::testing::Kit, wallet: &KeypairSigner, app: &KeypairSigner) {
        use app_core::bridge::{
            BridgeAnswer, BridgeRequest, ResolveCall, SiteCall, SiteTransaction, SiteWire,
        };
        use wallet_core::intent::{
            Delivery, Execution, Notes, Plan, Resolution, SigningProfile, Timing,
        };
        let mut ix = wallet_flows::testing::transfer_instruction(wallet.pubkey(), pk(9));
        let mut meta = ix.accounts[0].clone();
        meta.pubkey = app.pubkey();
        meta.is_signer = true;
        meta.is_writable = false;
        ix.accounts.push(meta);
        let wire = wallet_core::compile_v1(
            wallet.pubkey(),
            vec![ix],
            None,
            Some(solana_hash::Hash::new_from_array([1; 32])),
        )
        .unwrap()
        .wire;
        let signed =
            wallet_core::apply_local_signatures(&wire, &[wallet as &dyn WalletSigner]).unwrap();
        let call = ResolveCall {
            resolution: "r1".into(),
            account: wallet.pubkey(),
            chain: "solana:devnet".into(),
            transactions: vec![SiteTransaction {
                account: wallet.pubkey(),
                wire: SiteWire::Readable(wallet_core::TxFormat::V1, wire),
                chain: Some("solana:devnet".into()),
                commitment: None,
            }],
            notes: Notes::default(),
            order: Order::Serial,
            delivery: Delivery::Cosign,
            profile: SigningProfile {
                account: wallet.pubkey(),
                timing: Timing::Sync,
                execution: Execution::TopLevel,
                carrier: None,
                fee_payer: Vec::new(),
                new_accounts: Vec::new(),
                delivery: vec![Delivery::Cosign],
                messages: Vec::new(),
                max_transactions: 16,
            },
            plan: Plan {
                app_signs: vec![app.pubkey()],
                ..Plan::default()
            },
            notes_checked: true,
            message: None,
        };
        kit.core
            .resolution_create(
                BridgeRequest {
                    id: "q1".into(),
                    origin: ORIGIN.into(),
                    call: SiteCall::Resolve(Box::new(call)),
                },
                Resolution::new(
                    "r1",
                    wallet.pubkey(),
                    "solana:devnet",
                    Timing::Sync,
                    Delivery::Cosign,
                    1,
                    0,
                ),
            )
            .unwrap();
        assert!(kit
            .core
            .bridge_answer("q1", &BridgeAnswer::Signed(vec![signed])));
    }

    const ORIGIN: &str = "https://cosign.test";

    #[tokio::test]
    async fn once_the_site_has_signed_the_service_sends_it_and_records_that_it_ran() {
        let kit = app_core::testing::kit();
        let wallet = KeypairSigner::new_random();
        let app = KeypairSigner::new_random();
        cosigned(&kit, &wallet, &app);
        let held = kit.core.resolution(ORIGIN, "r1").unwrap();
        assert_eq!(held.state, State::AwaitingApp);

        // The node takes it and says it confirmed.
        let rpc = Arc::new(
            wallet_flows::testing::FakeRpc::default().with_signature_status(
                Signature::default(),
                wallet_core::SignatureStatus {
                    slot: 5,
                    err: None,
                    confirmation: Some(ConfirmationLevel::Confirmed),
                },
            ),
        );
        let public = kit.core.public();
        let scheduler = Scheduler::new(public.clone());
        let for_rpc = rpc.clone();
        let rpc_fn: RpcFn = Arc::new(move || Ok(for_rpc.clone() as Arc<dyn SolanaRpc>));
        let submitter = Submitter::new(public.clone(), crate::tx::Tracker::new(public.clone()));
        start_resolutions(public.clone(), submitter, &scheduler, rpc_fn);
        app_core::testing::settle().await;
        // Nothing goes out before the site has signed.
        assert_eq!(rpc.asked(), 0);

        let wire = held.transactions[0].wire.clone().unwrap();
        let message = wallet_core::parse_wire(&wire).unwrap().message;
        let signature = app.try_sign_message(&message).unwrap();
        let sending = kit
            .core
            .resolution_provide_signatures(
                ORIGIN,
                "r1",
                vec![(0, app.pubkey(), <[u8; 64]>::from(signature))],
            )
            .unwrap();
        assert_eq!(sending.state, State::Sending);
        for _ in 0..8 {
            app_core::testing::settle().await;
            kit.clock.advance(COMMITMENT_POLL);
        }
        let done = kit.core.resolution(ORIGIN, "r1").unwrap();
        assert_eq!(done.state, State::Done, "{:?}", done.error);
        assert_eq!(done.transactions[0].state, TxState::Landed);
        assert_eq!(done.ran.len(), 1);
        // It went out once, with a toast as a site's send gets.
        let sent_notices = kit
            .core
            .slices()
            .notices
            .get()
            .items()
            .iter()
            .filter(|notice| matches!(&notice.kind, app_core::NoticeKind::Transaction { label, .. } if label == "Sent for https://cosign.test"))
            .count();
        assert_eq!(sent_notices, 1);
    }

    #[test]
    fn an_order_goes_out_as_the_equivalent_site_send() {
        assert_eq!(send_plan(Order::Serial), SendPlan::Serial);
        assert_eq!(send_plan(Order::Any), SendPlan::Independent);
        assert_eq!(send_plan(Order::Atomic), SendPlan::InOrder);
    }
}
