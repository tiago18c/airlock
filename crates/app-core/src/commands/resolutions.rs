//! Resolutions: what became of a request a site asked to have resolved
//! (`docs/intents.md`). The bridge makes one as it queues the request; the
//! user's answer, the site and the chain move it on. Each change is saved
//! deferred and published, and the bridge tells the site.
//!
//! Commands that find the request by its id in the queue are the window's:
//! answering is the user's approval. Those that find the resolution by its
//! origin and id are for the bridge and the services as well.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use solana_pubkey::Pubkey;
use tracing::{debug, debug_span, info};
use wallet_core::intent::{
    signing_profile, tx_role, Composed, Delivery, ProfileError, Resolution, ResolutionError,
    ResolutionFailure, SendResult, SigningProfile, State, Timing, TxRole,
};
use wallet_core::{decode_transaction, signing_route, LookupTables};

use super::{locked, Core, Inner, CORE};
use crate::bridge::{BridgeAnswer, BridgeRefused, BridgeRequest, SendingRequest};
use crate::events::Event;
use crate::notices::BridgeRequestOutcome;
use crate::resolutions::{KeptResolution, ResolutionCommandError, ResolutionRefused};
use crate::slices::shown_wallets;

const BRIDGE: &str = "airlock::bridge";

impl Core {
    /// Answer request `id`, a `resolve` delivered `cosign` or `return`, with
    /// what the wallet composed and signed: the user approved it. `composed`
    /// says what each transaction carries when that is not one for each the
    /// site sent; with one for each, [`Core::bridge_answer`] with
    /// `BridgeAnswer::Signed` does the same. Answers whether it was waiting
    /// and its resolution took them.
    pub fn bridge_resolve_signed(&self, id: &str, composed: Vec<Composed>) -> bool {
        self.inner.bridge_resolve_signed(id, composed)
    }

    /// Take request `id`, a `resolve` delivered `send`, out of the queue to
    /// send what the wallet composed, as [`Core::bridge_begin_send`] does
    /// with one transaction for each the site sent. Its resolution is
    /// `sending`; tell it how the send went with
    /// [`Core::bridge_send_finished`].
    pub fn bridge_begin_send_composed(
        &self,
        id: &str,
        composed: Vec<Composed>,
    ) -> Option<SendingRequest> {
        self.inner.bridge_begin_send_composed(id, composed)
    }

    /// The user approved request `id`, a `resolve`, and another device or
    /// other people still sign: an offline key, or a multisig this device
    /// cannot carry alone. It leaves the queue, and its resolution is
    /// `awaiting-signatures`. Answers whether it was waiting.
    pub fn bridge_approved_elsewhere(&self, id: &str) -> bool {
        self.inner.bridge_approved_elsewhere(id)
    }

    /// Record which addresses the notes' rewrite replaced, address sent to
    /// address used, while the request waits for the user.
    pub fn resolution_set_replaced(
        &self,
        origin: &str,
        id: &str,
        replaced: BTreeMap<Pubkey, Pubkey>,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.inner
            .resolution_change("resolution_set_replaced", origin, id, |kept, now| {
                let resolution = &mut kept.resolution;
                if resolution.state != State::AwaitingApproval {
                    return Err(ResolutionError::BadState {
                        state: resolution.state,
                        action: "replacements",
                    });
                }
                resolution.replaced = replaced;
                resolution.updated_at = now;
                Ok(())
            })
    }

    /// The signatures from elsewhere are in, for a resolution approved here
    /// (`awaiting-signatures`): `composed` is every transaction, fully
    /// signed. It is `sending`, and ready for
    /// [`PublicCore::resolution_begin_send`](crate::PublicCore::resolution_begin_send).
    pub fn resolution_signed_elsewhere(
        &self,
        origin: &str,
        id: &str,
        composed: Vec<Composed>,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.inner
            .resolution_change("resolution_signed_elsewhere", origin, id, |kept, now| {
                kept.resolution.signed_elsewhere(composed, now)
            })
    }
}

impl Inner {
    fn now_unix(&self) -> u64 {
        self.ports.clock.now_ms() / 1000
    }

    /// The profile of `account`, a wallet of this device, and the payer of
    /// its signing route. Read from the wallets the screens show, so it
    /// works while the vault is locked. A Squads vault's is judged from what
    /// the Proposals pane last read of its multisig, and from its record
    /// when the pane has read nothing yet.
    pub(crate) fn resolution_profile(
        &self,
        account: Pubkey,
    ) -> Result<(SigningProfile, Option<Pubkey>), ProfileError> {
        let vault = locked(&self.vault);
        let list = locked(&self.wallet_list);
        let wallets = shown_wallets(&vault, &list).map_or(&[][..], |data| data.wallets.as_slice());
        let multisig = wallets
            .iter()
            .find(|wallet| wallet.address() == Some(account))
            .and_then(|wallet| match &wallet.kind {
                wallet_core::WalletKind::Squads { multisig, .. } => Some(*multisig),
                _ => None,
            });
        // Only what the cluster in use said: the same address on another
        // cluster is another multisig, or none.
        let cluster = self.slices.network.get().cluster.clone();
        let squad = multisig.and_then(|multisig| {
            locked(&self.squads)
                .get(&multisig)
                .filter(|(read_on, _)| *read_on == cluster)
                .map(|(_, state)| state.clone())
        });
        let profile = signing_profile(wallets, account, squad.as_ref())?;
        let payer = wallets
            .iter()
            .find(|wallet| wallet.address() == Some(account))
            .and_then(|wallet| signing_route(wallets, wallet.id).ok())
            .and_then(|route| route.fee_payer)
            .map(|(_, payer)| payer);
        Ok((profile, payer))
    }

    /// Keep what `cluster` said of `multisig`, for the profile of its vault
    /// while the app is on that cluster.
    pub(crate) fn squad_read(
        &self,
        multisig: Pubkey,
        cluster: wallet_core::Cluster,
        state: wallet_core::squads::SquadState,
    ) {
        locked(&self.squads).insert(multisig, (cluster, state));
    }

    pub(crate) fn resolution(&self, origin: &str, id: &str) -> Option<Resolution> {
        self.resolutions
            .lock()
            .data()
            .get(origin, id)
            .map(|kept| kept.resolution.clone())
    }

    /// Keep `resolution` for the origin of `request`, and queue `request`,
    /// both or neither. It expires once its approval window passes.
    pub(crate) fn resolution_create(
        self: &Arc<Self>,
        request: BridgeRequest,
        resolution: Resolution,
    ) -> Result<Resolution, ResolutionRefused> {
        let _span = debug_span!(target: CORE, "command", name = "resolution_create").entered();
        let origin = request.origin.clone();
        let queue_id = request.id.clone();
        let code = request.code();
        let method = request.method();
        let made = self.resolutions.edit(self, |data| {
            if data.get(&origin, &resolution.id).is_some() {
                return Err(ResolutionRefused::DuplicateId);
            }
            {
                let mut bridge = locked(&self.bridge);
                // Nobody waits on a reply: what becomes of it goes to the
                // resolution.
                bridge
                    .push(request, Box::new(|_| {}))
                    .map_err(|refused| match refused {
                        BridgeRefused::Full => ResolutionRefused::Full,
                        BridgeRefused::SameId => ResolutionRefused::SameRequest,
                    })?;
                self.slices.bridge.publish(Arc::new(bridge.queue().clone()));
            }
            let made = resolution.clone();
            data.insert(&origin, resolution)
                .map_err(|_| ResolutionRefused::DuplicateId)?;
            Ok(made)
        });
        let made = match made {
            Ok(made) => made,
            Err(refused) => {
                debug!(target: CORE, outcome = "refused", "command");
                return Err(refused);
            }
        };
        self.resolutions.mark_dirty(self);
        let _ = self.events.send(Event::RequestArrived { id: queue_id });
        info!(
            target: BRIDGE,
            code = %code,
            method,
            origin = %origin,
            id = %made.id,
            timing = ?made.timing,
            delivery = ?made.delivery,
            "resolution made"
        );
        self.expire_later(origin, made.id.clone(), made.timing);
        debug!(target: CORE, outcome = "ok", "command");
        Ok(made)
    }

    /// Expire resolution `id` of `origin` once its approval window has
    /// passed, if nobody decided by then.
    fn expire_later(self: &Arc<Self>, origin: String, id: String, timing: Timing) {
        let clock = self.ports.clock.clone();
        let core = Arc::downgrade(self);
        self.ports.spawner.spawn(Box::pin(async move {
            // Made in the task: a timer needs a runtime, and the bridge's
            // threads have none.
            clock
                .sleep(Duration::from_secs(timing.approval_window_secs()))
                .await;
            if let Some(core) = core.upgrade() {
                let waiting = core
                    .resolution(&origin, &id)
                    .is_some_and(|resolution| resolution.state == State::AwaitingApproval);
                if waiting {
                    let _ = core.resolution_expire(&origin, &id);
                }
            }
        }));
    }

    pub(crate) fn resolution_expire(
        self: &Arc<Self>,
        origin: &str,
        id: &str,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.resolution_change("resolution_expire", origin, id, |kept, now| {
            kept.resolution.expire(now)
        })
    }

    pub(crate) fn resolution_cancel(
        self: &Arc<Self>,
        origin: &str,
        id: &str,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.resolution_change("resolution_cancel", origin, id, |kept, now| {
            kept.resolution.cancel(now)
        })
    }

    pub(crate) fn resolution_fail(
        self: &Arc<Self>,
        origin: &str,
        id: &str,
        failure: ResolutionFailure,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.resolution_change("resolution_fail", origin, id, |kept, now| {
            kept.resolution.fail(failure, now)
        })
    }

    /// Every signature in `signatures` goes in, or none does.
    pub(crate) fn resolution_provide_signatures(
        self: &Arc<Self>,
        origin: &str,
        id: &str,
        signatures: Vec<(usize, Pubkey, [u8; 64])>,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.resolution_change("resolution_provide_signatures", origin, id, |kept, now| {
            for (transaction, signer, signature) in &signatures {
                kept.resolution
                    .provide_signature(*transaction, *signer, signature, now)?;
            }
            Ok(())
        })
    }

    pub(crate) fn resolution_begin_send(
        self: &Arc<Self>,
        origin: &str,
        id: &str,
    ) -> Option<Vec<Vec<u8>>> {
        let resolution = self
            .resolution_change("resolution_begin_send", origin, id, |kept, _| {
                if !kept.ready_to_send() {
                    return Err(ResolutionError::BadState {
                        state: kept.resolution.state,
                        action: "sending",
                    });
                }
                kept.claimed = true;
                Ok(())
            })
            .ok()?;
        resolution.wires_to_send().map(<[Vec<u8>]>::to_vec)
    }

    pub(crate) fn resolution_sent(
        self: &Arc<Self>,
        origin: &str,
        id: &str,
        results: Vec<SendResult>,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.resolution_change("resolution_sent", origin, id, |kept, now| {
            kept.resolution.sent(results, now)
        })
    }

    pub(crate) fn resolution_executed(
        self: &Arc<Self>,
        origin: &str,
        id: &str,
        transaction: usize,
        signature: solana_signature::Signature,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.resolution_change("resolution_executed", origin, id, |kept, now| {
            kept.resolution.executed(transaction, signature, now)
        })
    }

    pub(crate) fn resolution_proposal_closed(
        self: &Arc<Self>,
        origin: &str,
        id: &str,
        reason: String,
    ) -> Result<Resolution, ResolutionCommandError> {
        self.resolution_change("resolution_proposal_closed", origin, id, |kept, now| {
            kept.resolution.proposal_closed(reason, now)
        })
    }

    /// Change resolution `id` of `origin` with `f`, at the clock's time. A
    /// change `f` refuses leaves it as it was. Once it no longer waits for
    /// the person its request leaves the queue.
    pub(super) fn resolution_change(
        self: &Arc<Self>,
        name: &'static str,
        origin: &str,
        id: &str,
        f: impl FnOnce(&mut KeptResolution, u64) -> Result<(), ResolutionError>,
    ) -> Result<Resolution, ResolutionCommandError> {
        let _span = debug_span!(target: CORE, "command", name).entered();
        let now = self.now_unix();
        let changed: Result<_, ResolutionCommandError> = self.resolutions.edit(self, |data| {
            let kept = data
                .get_mut(origin, id)
                .ok_or(ResolutionCommandError::NotFound)?;
            let mut copy = kept.clone();
            f(&mut copy, now).map_err(ResolutionCommandError::Refused)?;
            *kept = copy;
            Ok(kept.resolution.clone())
        });
        let resolution = match changed {
            Ok(resolution) => resolution,
            Err(why) => {
                debug!(target: CORE, outcome = why.code(), "command");
                return Err(why);
            }
        };
        self.resolutions.mark_dirty(self);
        log_change(origin, &resolution);
        if resolution.state != State::AwaitingApproval {
            self.drop_resolve_request(origin, id, resolution.state);
        }
        debug!(target: CORE, outcome = "ok", "command");
        Ok(resolution)
    }

    /// Take the request that waits for resolution `id` of `origin` out of
    /// the queue, when one does: nobody can decide it any more.
    fn drop_resolve_request(self: &Arc<Self>, origin: &str, id: &str, state: State) {
        let taken = {
            let mut bridge = locked(&self.bridge);
            let Some(queue_id) = bridge.resolve_waiting(origin, id) else {
                return;
            };
            let taken = bridge.take(&queue_id, None);
            self.slices.bridge.publish(Arc::new(bridge.queue().clone()));
            taken
        };
        if let Some(taken) = taken {
            self.bridge_finished(&taken.request, outcome_of(state));
        }
    }

    /// Whether request `id` in the queue is a `resolve`.
    pub(super) fn is_resolve(&self, id: &str) -> bool {
        locked(&self.bridge)
            .queue()
            .get(id)
            .is_some_and(|request| request.resolve().is_some())
    }

    /// Take request `id`, a `resolve`, out of the queue when `fit` fits it,
    /// and change its resolution with `f`: both or neither.
    pub(super) fn resolve_answer(
        self: &Arc<Self>,
        name: &'static str,
        id: &str,
        fit: Option<&BridgeAnswer>,
        f: impl FnOnce(&mut KeptResolution, u64) -> Result<(), ResolutionError>,
    ) -> Option<(Arc<BridgeRequest>, Resolution)> {
        let _span = debug_span!(target: CORE, "command", name).entered();
        let (origin, resolution_id) = {
            let bridge = locked(&self.bridge);
            let request = bridge.queue().get(id)?;
            let call = request.resolve()?;
            (request.origin.clone(), call.resolution.clone())
        };
        let now = self.now_unix();
        let done: Result<_, &str> = self.resolutions.edit(self, |data| {
            let kept = data
                .get_mut(&origin, &resolution_id)
                .ok_or("no resolution")?;
            let mut copy = kept.clone();
            f(&mut copy, now).map_err(|err| {
                debug!(target: CORE, error = %err, "the resolution refused the answer");
                "refused"
            })?;
            let taken = {
                let mut bridge = locked(&self.bridge);
                let taken = bridge.take(id, fit).ok_or("not waiting")?;
                self.slices.bridge.publish(Arc::new(bridge.queue().clone()));
                taken
            };
            *kept = copy;
            Ok((taken.request, kept.resolution.clone()))
        });
        let (request, resolution) = match done {
            Ok(done) => done,
            Err(why) => {
                debug!(target: CORE, outcome = why, "command");
                return None;
            }
        };
        self.resolutions.mark_dirty(self);
        log_change(&origin, &resolution);
        self.bridge_finished(&request, outcome_of(resolution.state));
        Some((request, resolution))
    }

    /// The user's answer to request `id`, a `resolve`: declined, or signed
    /// one transaction for each the site sent. A send goes through
    /// [`Inner::bridge_begin_send`].
    pub(super) fn resolve_answered(self: &Arc<Self>, id: &str, answer: &BridgeAnswer) -> bool {
        match answer {
            BridgeAnswer::Rejected => self
                .resolve_answer("bridge_answer", id, Some(answer), |kept, now| {
                    kept.resolution.decline(now)
                })
                .is_some(),
            BridgeAnswer::Signed(wires) => self
                .resolve_answer("bridge_answer", id, Some(answer), |kept, now| {
                    let composed = one_for_each(wires, &kept.resolution.account);
                    kept.resolution.signed(composed, now)
                })
                .is_some(),
            _ => {
                debug!(target: CORE, outcome = "does not fit", "command");
                false
            }
        }
    }

    /// [`Inner::bridge_begin_send`] for a `resolve`: its resolution takes
    /// what `composed` makes of the account, signed, and is `sending`.
    pub(super) fn begin_resolve_send(
        self: &Arc<Self>,
        id: &str,
        fit: Option<&BridgeAnswer>,
        composed: impl FnOnce(&Pubkey) -> Vec<Composed>,
    ) -> Option<SendingRequest> {
        let (request, resolution) =
            self.resolve_answer("bridge_begin_send", id, fit, |kept, now| {
                if kept.resolution.delivery != Delivery::Send {
                    return Err(ResolutionError::Mismatch(
                        "only a resolution delivered send is sent here".into(),
                    ));
                }
                let made = composed(&kept.resolution.account);
                kept.resolution.signed(made, now)?;
                // Whoever began the send sends it.
                kept.claimed = true;
                Ok(())
            })?;
        Some(SendingRequest {
            resolution: Some((request.origin.clone(), resolution.id.clone())),
            request,
            reply: None,
        })
    }

    pub(crate) fn bridge_begin_send_composed(
        self: &Arc<Self>,
        id: &str,
        composed: Vec<Composed>,
    ) -> Option<SendingRequest> {
        self.begin_resolve_send(id, None, |_| composed)
    }

    pub(crate) fn bridge_resolve_signed(
        self: &Arc<Self>,
        id: &str,
        composed: Vec<Composed>,
    ) -> bool {
        self.resolve_answer("bridge_resolve_signed", id, None, |kept, now| {
            if kept.resolution.delivery == Delivery::Send {
                return Err(ResolutionError::Mismatch(
                    "a resolution delivered send is sent, not answered".into(),
                ));
            }
            kept.resolution.signed(composed, now)
        })
        .is_some()
    }

    pub(crate) fn bridge_approved_elsewhere(self: &Arc<Self>, id: &str) -> bool {
        self.resolve_answer("bridge_approved_elsewhere", id, None, |kept, now| {
            kept.resolution.approved_elsewhere(now)
        })
        .is_some()
    }
}

/// `wires`, one for each transaction the site sent and in its order, as the
/// transactions the wallet composed, each with its role for `account`. A
/// transaction that cannot be read here, such as a v0 one whose tables were
/// not read, counts as one the account signs.
pub(super) fn one_for_each(wires: &[Vec<u8>], account: &Pubkey) -> Vec<Composed> {
    wires
        .iter()
        .enumerate()
        .map(|(index, wire)| Composed {
            role: decode_transaction(wire, &LookupTables::new())
                .ok()
                .and_then(|tx| tx_role(&tx, account))
                .unwrap_or(TxRole::Direct),
            of: vec![index],
            wire: wire.clone(),
        })
        .collect()
}

/// How a request that waited for a resolution ended, for its notice.
fn outcome_of(state: State) -> BridgeRequestOutcome {
    match state {
        State::Declined | State::Failed => BridgeRequestOutcome::Rejected,
        State::Expired | State::Cancelled => BridgeRequestOutcome::Expired,
        _ => BridgeRequestOutcome::Answered,
    }
}

fn log_change(origin: &str, resolution: &Resolution) {
    info!(
        target: BRIDGE,
        origin = %origin,
        id = %resolution.id,
        state = resolution.state.name(),
        error = resolution.error.as_ref().map(|error| error.message.as_str()),
        "resolution changed"
    );
}
