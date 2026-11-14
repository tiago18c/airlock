//! Prepare a browser sign request for display and action.

use super::*;
use app_ui_kit::state::forms::{BlockhashClock, SimState};
use wallet_flows::offer::{modal_offer, ModalFacts, Simulated};

/// The transaction a sign request will be answered with.
#[derive(Clone, PartialEq)]
pub(in crate::ui) enum Prepared {
    Working,
    Ready {
        wire: Vec<u8>,
        format: wallet_core::TxFormat,
        tables: wallet_core::LookupTables,
        /// Set when the site's transaction was rebuilt for a keyless wallet.
        /// Boxed, with `converted`, so this variant is not far larger than
        /// the others.
        adapted: Option<Box<Adapted>>,
        /// Set when a legacy or v0 wire was rewritten as v1.
        converted: Option<Box<wallet_core::Converted>>,
        /// When this app chose the blockhash, the height it expires at.
        last_valid_height: Option<u64>,
        flow: u64,
        /// It was moved onto an offline key's durable nonce on the way here.
        onto_nonce: bool,
    },
    Failed(String),
    /// It asks something the site was not given. Trying again changes nothing.
    Refused(String),
}

/// What Review hands to the Sign screen.
#[derive(Clone, PartialEq)]
pub(in crate::ui) struct ReadyWire {
    pub wire: Vec<u8>,
    /// Set when the site's transaction was rebuilt for a keyless wallet.
    pub adapted: Option<Adapted>,
    /// When this app chose the blockhash, the height it expires at.
    pub last_valid_height: Option<u64>,
    pub tables: wallet_core::LookupTables,
    /// Set when a legacy or v0 wire was rewritten as v1.
    pub converted: Option<wallet_core::Converted>,
    pub flow: u64,
    /// It was moved onto an offline key's durable nonce on the way here.
    pub onto_nonce: bool,
}

/// What Sign signs.
#[derive(Clone, PartialEq)]
pub(in crate::ui) struct SignWire {
    pub wire: Vec<u8>,
    pub tables: wallet_core::LookupTables,
    /// What is known of its Vector accounts.
    pub known: Vec<wallet_core::relay::VectorContext>,
    pub flow: u64,
}

/// Values derived from a bridge sign prompt, for the request modal.
pub(in crate::ui) struct PreparedRequest {
    pub account: Option<Pubkey>,
    pub original: Option<Vec<u8>>,
    pub attempt: Signal<u32>,
    pub paid_here: Signal<bool>,
    pub fetching: Signal<bool>,
    pub state: Prepared,
    pub adapted: Option<Adapted>,
    pub converted: Option<wallet_core::Converted>,
    pub offered: Option<PaidHere>,
    pub shown: Option<wallet_core::DecodedTx>,
    pub explained: Option<wallet_core::ExplainedTx>,
    /// It is still being read.
    pub reading: bool,
    /// Reading it took too long.
    pub gave_up: bool,
    pub locked: bool,
    pub can_review: bool,
    pub can_sign: bool,
    /// Sign is offered, but something says to look twice.
    pub cautious_sign: bool,
    /// What the node said it would do.
    pub simulation: Signal<SimState>,
    /// Who signs if this device signs it now.
    pub signs_with: Vec<Pubkey>,
    /// The vendor of the hardware wallet Sign asks first, when one does.
    pub on_device: Option<wallet_core::HardwareVendor>,
    /// It was moved onto an offline key's durable nonce on the way here.
    pub onto_nonce: bool,
    pub verdict: Option<wallet_core::QuickSign>,
    pub step: Option<wallet_core::NextSignature>,
    pub lifetime: Option<wallet_core::TxLifetime>,
    pub modal_expired: bool,
    pub countdown_label: Signal<String>,
    pub clock: BlockhashClock,
    pub clock_expired: Signal<bool>,
    pub for_review: Option<ReadyWire>,
    pub for_sign: Option<SignWire>,
    pub replaced_payer: bool,
    /// Unsigned accounts the site's own transaction creates, such as a new
    /// mint. The site signs for them once it has the transaction back.
    pub site_signs: Vec<Pubkey>,
}

/// Prepare transaction `index` of the sign request `current`.
pub(in crate::ui) fn use_prepared_request(
    session: Session,
    current: BridgePrompt,
    index: usize,
) -> PreparedRequest {
    let transaction = current.request.transactions().get(index);
    let original = transaction.and_then(|transaction| transaction.wire.bytes().map(<[u8]>::to_vec));
    let account = transaction.map(|transaction| transaction.account);

    let mut prepared = use_signal(|| Prepared::Working);
    let attempt = use_signal(|| 0u32);
    // Counts preparations, so a slow older one cannot overwrite a newer one.
    let mut turn = use_signal(|| 0u64);
    // A locked vault knows no wallets. Prepare again once it is unlocked.
    let unlocked = use_memo(move || session.is_unlocked());
    let mut offer = use_signal(|| None::<PaidHere>);
    let mut paid_here = use_signal(|| false);

    // One log trail for the request, however often it is prepared again.
    let flow = use_hook(app_core::diagnostics::next_flow_id);
    let mut received_logged = use_hook(|| CopyValue::new(false));

    let source = original.clone();
    let prepare_origin = current.origin.clone();
    let prepare_id = current.id.clone();
    use_effect(move || {
        let _ = (attempt(), unlocked());
        let Some(wire) = source.clone() else {
            return;
        };
        let mine = *turn.peek() + 1;
        turn.set(mine);
        prepared.set(Prepared::Working);
        offer.set(None);
        paid_here.set(false);
        let origin = prepare_origin.clone();
        let request_id = prepare_id.clone();
        // `prepare_request` logs `received` before anything can stop it.
        let log_received = !*received_logged.peek();
        received_logged.set(true);
        spawn(async move {
            let code = crate::bridge::request_code(&request_id);
            let result = prepare_request(
                session,
                wire.clone(),
                log_received.then(|| app_ui_kit::actions::sign::ReceivedMeta {
                    source: "browser",
                    origin: Some(&origin),
                    code: Some(&code),
                }),
                Some(flow),
                true,
            )
            .await;
            if *turn.peek() != mine {
                return;
            }
            let ready = result.is_ok();
            prepared.set(match result {
                Ok(prepared) if !session.site_given(&origin, &prepared.asks) => {
                    Prepared::Refused(crate::bridge::NOT_GIVEN_TO_SITE.to_owned())
                }
                Ok(prepared) => Prepared::Ready {
                    wire: prepared.wire,
                    format: prepared.format,
                    tables: prepared.tables,
                    adapted: prepared.adapted.map(Box::new),
                    converted: prepared.converted.map(Box::new),
                    last_valid_height: prepared.last_valid_height,
                    flow: prepared.flow,
                    onto_nonce: prepared.onto_nonce,
                },
                Err(err) => Prepared::Failed(err),
            });
            // Payer swap only after a v1 (or converted) path; as-is legacy/v0
            // keeps the site's fee payer.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(Prepared::Ready {
                format: wallet_core::TxFormat::V1,
                wire: ready_wire,
                flow,
                ..
            }) = ready.then(|| prepared.peek().clone())
            {
                let found = offer_paid_here(session, ready_wire, flow).await;
                if *turn.peek() == mine {
                    offer.set(found);
                }
            }
        });
    });

    // IDLs this device lacks for what is about to be shown. A stored IDL
    // changes the catalog, and the summary below is read again.
    let mut fetched_for = use_signal(|| None::<[u8; 32]>);
    let mut fetching = use_signal(|| false);
    use_effect(move || {
        let Prepared::Ready { wire, tables, .. } = prepared() else {
            return;
        };
        let Ok(tx) = wallet_core::decode_transaction(&wire, &tables) else {
            return;
        };
        let digest = sha256_bytes(&tx.message);
        if *fetched_for.peek() == Some(digest) {
            return;
        }
        fetched_for.set(Some(digest));
        let missing = missing_catalog_programs(&*session.idls.peek(), &tx);
        if missing.is_empty() {
            return;
        }
        fetching.set(true);
        spawn(async move {
            let _ = fetch_missing_idls(&session, &missing).await;
            fetching.set(false);
        });
    });

    let state = prepared();
    let offered = offer();
    let taken = offered.clone().filter(|_| paid_here());
    let ready = match (&state, &taken) {
        (
            Prepared::Ready {
                last_valid_height,
                tables,
                converted,
                flow,
                onto_nonce,
                ..
            },
            Some(paid),
        ) => Some(ReadyWire {
            wire: paid.wire.clone(),
            adapted: paid.adapted.clone(),
            last_valid_height: *last_valid_height,
            tables: tables.clone(),
            converted: converted.as_deref().cloned(),
            flow: *flow,
            onto_nonce: *onto_nonce,
        }),
        (
            Prepared::Ready {
                wire,
                adapted,
                last_valid_height,
                tables,
                converted,
                flow,
                onto_nonce,
                ..
            },
            None,
        ) => Some(ReadyWire {
            wire: wire.clone(),
            adapted: adapted.as_deref().cloned(),
            last_valid_height: *last_valid_height,
            tables: tables.clone(),
            converted: converted.as_deref().cloned(),
            flow: *flow,
            onto_nonce: *onto_nonce,
        }),
        _ => None,
    };
    let adapted = ready.as_ref().and_then(|ready| ready.adapted.clone());
    let converted = ready.as_ref().and_then(|ready| ready.converted.clone());
    let replaced_payer = taken.is_some();
    // Decoded once per wire, not on every render of the modal.
    let decode_key = ready
        .as_ref()
        .map(|ready| (ready.wire.clone(), ready.tables.clone()));
    let tx = use_memo(use_reactive!(|(decode_key)| {
        decode_key
            .as_ref()
            .and_then(|(wire, tables)| wallet_core::decode_transaction(wire, tables).ok())
    }))();
    let shown = tx.clone();
    let vector_known = adapted
        .as_ref()
        .map(|adapted| adapted.context.clone())
        .unwrap_or_default();
    let reading = use_explained_for_site(tx.clone(), vector_known.clone());
    let reading_waits = reading.as_ref().is_some_and(|reading| reading.unsettled());
    let reading_gave_up = reading.as_ref().is_some_and(|reading| reading.gave_up);
    // Nothing is offered on a reading that has not come, or that an IDL came
    // after. One it gave up on says none of it in words, and is offered as that.
    let explained = reading
        .filter(|reading| !reading.unsettled())
        .map(|reading| reading.explained);
    // Preparing may rebuild the request around a wallet's keys, so nothing
    // goes further until the vault is open.
    let locked = session.is_initialized() && !session.is_unlocked();
    let vector_here = session.local_vector_accounts();
    let local_keys = session.local_signer_pubkeys();
    let devices = session.local_device_keys();
    let local = wallet_core::LocalKeys {
        keys: &local_keys,
        vector: &vector_here,
        devices: &devices,
    };
    let ctx = wallet_core::relay::AuthContext {
        vector: &vector_known,
    };
    let site_signs = shown
        .as_ref()
        .filter(|tx| tx.format != wallet_core::TxFormat::V1)
        .map(|tx| {
            let mut here = session.known_wallet_pubkeys();
            here.extend(local_keys.iter().copied());
            wallet_flows::request::left_for_site(tx, &here, &session.forms.sign.created_in(tx))
        })
        .unwrap_or_default();
    let verdict = match (&shown, &explained) {
        (Some(tx), Some(explained)) => Some(quick_sign_in(tx, &local, &ctx, explained)),
        _ => None,
    };
    let status = shown
        .as_ref()
        .map(|tx| wallet_core::signing_status_in(tx, &local, &ctx));
    let step = status.as_ref().map(wallet_core::next_step);
    let mut signs_with: Vec<Pubkey> = status
        .iter()
        .flat_map(|status| &status.lines)
        .filter(|line| {
            matches!(
                line.state,
                wallet_core::SignerState::Local | wallet_core::SignerState::Device
            )
        })
        .map(|line| line.authority_chip())
        .collect();
    signs_with.dedup();
    let on_device = status
        .iter()
        .flat_map(|status| &status.lines)
        .find(|line| line.state == wallet_core::SignerState::Device)
        .and_then(|line| line.on_device);
    let lifetime = shown.as_ref().map(wallet_core::tx_lifetime);
    // The modal's own countdown. A blockhash this app chose (a rebuild) has
    // an exact expiry. A site's is estimated from when the request arrived,
    // and Review hands that estimate on to the Sign screen.
    let clock = use_blockhash_clock_state();
    let prepared_height = ready.as_ref().and_then(|ready| ready.last_valid_height);
    use_effect(use_reactive!(|(lifetime, prepared_height)| {
        let _ = lifetime;
        clock.reset(prepared_height.map(BlockhashExpiry::exact));
    }));
    let clock_expired = use_signal(|| false);
    let countdown_label = use_signal(|| String::from("…"));
    let keys_replaced = converted
        .as_ref()
        .is_some_and(|converted| !converted.replaced.is_empty());
    let authorizations_here = status
        .as_ref()
        .map_or(0, |status| status.authorizations_here());
    // What the node says it would do, before anyone is asked to sign it. One with
    // an authorization to sign first would only fail for the lack of it.
    let to_simulate = shown
        .clone()
        .filter(|_| authorizations_here == 0 && !(adapted.is_some() && keys_replaced));
    let mut simulation = use_signal(|| SimState::Idle);
    // Counts simulations, so a slow older one cannot overwrite a newer one.
    let mut simulations = use_signal(|| 0u64);
    use_effect(use_reactive!(|(to_simulate)| {
        let mine = *simulations.peek() + 1;
        simulations.set(mine);
        let Some(tx) = to_simulate.clone() else {
            simulation.set(SimState::Idle);
            return;
        };
        simulation.set(SimState::Running);
        let rpc = session.rpc();
        spawn(async move {
            let result = match rpc {
                Ok(rpc) => wallet_flows::simulate::simulate(rpc.as_ref(), &tx).await,
                Err(err) => Err(err),
            };
            if *simulations.peek() == mine {
                simulation.set(match result {
                    Ok(simulation) => SimState::Done(simulation),
                    Err(err) => SimState::Failed(err.to_string()),
                });
            }
        });
    }));
    let simulated = match &*simulation.read() {
        SimState::Done(done) if done.error.is_some() => Simulated::Fails,
        SimState::Done(_) => Simulated::Succeeds,
        SimState::Failed(_) => Simulated::Unavailable,
        _ => Simulated::Pending,
    };
    let modal = modal_offer(ModalFacts {
        prepared: ready.is_some(),
        locked,
        rebuilt: adapted.is_some(),
        keys_replaced,
        authorizations_here,
        verdict,
        left_for_site: site_signs.len(),
        looking_for_idls: fetching(),
        simulated,
        clock_expired: clock_expired(),
        step,
        too_large: shown
            .as_ref()
            .is_some_and(wallet_core::DecodedTx::too_large),
    });

    let for_review = ready.clone();
    let for_sign = ready.as_ref().map(|ready| SignWire {
        wire: ready.wire.clone(),
        tables: ready.tables.clone(),
        known: ready
            .adapted
            .as_ref()
            .map(|adapted| adapted.context.clone())
            .unwrap_or_default(),
        flow: ready.flow,
    });

    PreparedRequest {
        account,
        original,
        attempt,
        paid_here,
        fetching,
        state,
        adapted,
        converted,
        offered,
        shown,
        explained,
        reading: reading_waits,
        gave_up: reading_gave_up,
        locked,
        can_review: modal.review,
        can_sign: modal.sign,
        cautious_sign: modal.cautious_sign,
        simulation,
        signs_with,
        on_device,
        onto_nonce: ready.as_ref().is_some_and(|ready| ready.onto_nonce),
        verdict,
        step,
        lifetime,
        modal_expired: modal.expired,
        countdown_label,
        clock,
        clock_expired,
        for_review,
        for_sign,
        replaced_payer,
        site_signs,
    }
}
