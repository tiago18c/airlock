//! Getting a transaction ready to be signed on this device.

use super::sentence;
use crate::diagnostics::{next_flow_id, TxLog};
use crate::state::session::Session;
use dioxus::prelude::ReadableExt;
#[cfg(not(target_arch = "wasm32"))]
use solana_pubkey::Pubkey;
use wallet_core::adapt::{Adapted, AdaptedStep};
use wallet_core::{
    decode_transaction, decode_v1_transaction, parse_wire, sha256_bytes, Converted, LookupTables,
    TxFormat,
};
use wallet_flows::lookup::resolve_lookup_tables;
use wallet_flows::request::{
    accounts_created, replaceable_signers, route_created, OneTimeKeys, Route,
};

/// Who to name on the `received` line when preparing starts.
pub struct ReceivedMeta<'a> {
    pub source: &'a str,
    pub origin: Option<&'a str>,
    pub code: Option<&'a str>,
}

/// What [`prepare_request`](prepare_request) hands the request modal.
#[derive(Clone, PartialEq)]
pub struct PreparedRequest {
    pub wire: Vec<u8>,
    pub format: TxFormat,
    pub tables: LookupTables,
    pub adapted: Option<Adapted>,
    pub converted: Option<Converted>,
    /// When this app chose a fresh blockhash, the height it expires at.
    pub last_valid_height: Option<u64>,
    /// Log trail id shared with later stages.
    pub flow: u64,
}

/// Prepare a browser (or paste) request: v1 as today; legacy/v0 signed as-is
/// when every missing signature is a hot key here and no keyless wallet is
/// asked; otherwise convert to v1 and rebuild. With `site_signs`, the
/// answer goes back to a site that can still sign it, and an unsigned account
/// the transaction creates is left for the site to sign: see
/// [`wallet_flows::request::route`].
///
/// `flow` continues an earlier attempt's log trail. `received` is logged
/// before the vault is checked, so a request that arrives while it is locked
/// is on record once, and the retry after unlocking need not log it again.
pub async fn prepare_request(
    session: Session,
    wire: Vec<u8>,
    received: Option<ReceivedMeta<'_>>,
    flow: Option<u64>,
    site_signs: bool,
) -> Result<PreparedRequest, String> {
    let flow = flow.unwrap_or_else(next_flow_id);
    let cluster = session.cluster();
    if let Some(meta) = &received {
        TxLog {
            flow,
            wire: &wire,
            cluster: Some(&cluster),
        }
        .log_received(meta.source, meta.origin, meta.code);
    }

    if session.is_initialized() && !session.is_unlocked() {
        return Err("The vault is locked. Unlock it before preparing a transaction.".into());
    }

    let view = parse_wire(&wire).map_err(sentence)?;
    if view.format == TxFormat::V1 {
        let (wire, adapted, last_valid_height) =
            rebuild_for_wallets_flow(session, wire, flow).await?;
        let (wire, last_valid_height) =
            onto_offline_nonce(session, wire, adapted.as_ref(), last_valid_height, flow).await;
        return Ok(PreparedRequest {
            wire,
            format: TxFormat::V1,
            tables: LookupTables::new(),
            adapted,
            converted: None,
            last_valid_height,
            flow,
        });
    }

    let tables = lookup_tables_for(session, &wire).await?;
    let tx = decode_transaction(&wire, &tables).map_err(sentence)?;
    let known = session.known_wallet_pubkeys();
    let rpc = session.rpc().ok();
    // Found once, and kept for the screens that show the transaction.
    let created = accounts_created(rpc.as_deref(), &tx, &known).await;
    session.forms.sign.keep_created(&tx, created.clone());
    // A vault may be asked along with keys the rewrite would replace.
    let keyless = session
        .plan_adaptation_allowing(&tx, &replaceable_signers(&tx, &known, &created))
        .map_err(sentence)?
        .is_some();
    let route = route_created(
        rpc.as_deref(),
        &tx,
        &session.local_signer_pubkeys(),
        &known,
        keyless,
        site_signs,
        &created,
    )
    .await
    .map_err(sentence)?;
    let Route::Converted(converted, one_time) = route else {
        return Ok(PreparedRequest {
            wire,
            format: tx.format,
            tables,
            adapted: None,
            converted: None,
            last_valid_height: None,
            flow,
        });
    };
    let from_message = hex::encode(sha256_bytes(&view.message));
    TxLog {
        flow,
        wire: &converted.wire,
        cluster: Some(&cluster),
    }
    .log_transformed("convert_v1", &from_message);
    // Kept before the rebuild, which plans with them, and signing with
    // them waits for the end: an authorization written into the
    // transaction voids every signature made before it.
    session.keep_made_here(one_time.pairs());
    let (wire, adapted, last_valid_height) =
        rebuild_for_wallets_flow(session, converted.wire.clone(), flow).await?;
    let (wire, last_valid_height) =
        onto_offline_nonce(session, wire, adapted.as_ref(), last_valid_height, flow).await;
    still_signs(&wire, &one_time)?;
    Ok(PreparedRequest {
        wire,
        format: TxFormat::V1,
        tables: LookupTables::new(),
        adapted,
        converted: Some(converted),
        last_valid_height,
        flow,
    })
}

/// Refuse `wire`, in its final shape, when a key made in place of a site's
/// one-time key no longer signs it: a rebuild moved it somewhere this
/// device cannot sign for it.
fn still_signs(wire: &[u8], keys: &OneTimeKeys) -> Result<(), String> {
    if keys.is_empty() {
        return Ok(());
    }
    let view = parse_wire(wire).map_err(sentence)?;
    let signers = &view.static_keys[..view.num_required_signatures];
    match keys.pubkeys().iter().find(|key| !signers.contains(key)) {
        Some(missing) => Err(format!(
            "Reshaping this transaction is not supported for this website. The rebuilt transaction would need {missing}, the key made here for it, to sign inside it, and this device cannot do that."
        )),
        None => Ok(()),
    }
}

/// Move a request that waits on one offline key alone onto that key's
/// durable nonce, so it stays valid while the key signs on another device.
/// A rebuilt request, one that cannot be moved, or one whose nonce cannot be
/// read comes back as it was. A moved one has no height at which it
/// expires. Deprecated once the programmatic signer or Vector is on mainnet.
async fn onto_offline_nonce(
    session: Session,
    wire: Vec<u8>,
    adapted: Option<&Adapted>,
    last_valid_height: Option<u64>,
    flow: u64,
) -> (Vec<u8>, Option<u64>) {
    if adapted.is_some() {
        return (wire, last_valid_height);
    }
    let Ok(tx) = decode_v1_transaction(&wire) else {
        return (wire, last_valid_height);
    };
    let Some((key, nonce)) = session.offline_nonce_for(&tx) else {
        return (wire, last_valid_height);
    };
    let Ok(rpc) = session.rpc() else {
        return (wire, last_valid_height);
    };
    match wallet_flows::offline_nonce::move_onto(rpc.as_ref(), &tx, key, nonce).await {
        Ok(moved) => {
            let cluster = session.cluster();
            TxLog {
                flow,
                wire: &moved,
                cluster: Some(&cluster),
            }
            .log_transformed("onto_durable_nonce", &message_hash(&wire));
            (moved, None)
        }
        Err(_) => (wire, last_valid_height),
    }
}

/// The lookup tables `wire` names, read from the node. Legacy, v1 and a v0
/// wire that names no table need no connection.
pub async fn lookup_tables_for(session: Session, wire: &[u8]) -> Result<LookupTables, String> {
    let rpc = session.rpc().ok();
    resolve_lookup_tables(rpc.as_deref(), wire)
        .await
        .map_err(sentence)
}

/// Rebuild `wire` if it asks a wallet without a key to sign: a programmatic
/// signer or a Squads vault of this device. Anything else comes back as it
/// was given. When this app chose a fresh blockhash, the height at which it
/// expires is the third value.
pub async fn rebuild_for_wallets(
    session: Session,
    wire: Vec<u8>,
) -> Result<(Vec<u8>, Option<Adapted>, Option<u64>), String> {
    let flow = session.forms.sign.flow.peek().unwrap_or_else(next_flow_id);
    rebuild_for_wallets_flow(session, wire, flow).await
}

async fn rebuild_for_wallets_flow(
    session: Session,
    wire: Vec<u8>,
    flow: u64,
) -> Result<(Vec<u8>, Option<Adapted>, Option<u64>), String> {
    let from_message = message_hash(&wire);
    let (wire, adapted, last_valid_height) = rebuild_quietly(session, wire).await?;
    if let Some(adapted) = &adapted {
        log_rebuild(session, flow, adapted, &from_message);
    }
    Ok((wire, adapted, last_valid_height))
}

/// [`rebuild_for_wallets_flow`] without the log line, for a wire that may
/// never be used (the paid-here offer).
async fn rebuild_quietly(
    session: Session,
    wire: Vec<u8>,
) -> Result<(Vec<u8>, Option<Adapted>, Option<u64>), String> {
    let tx = decode_v1_transaction(&wire).map_err(sentence)?;
    let Some(plan) = session.plan_adaptation(&tx).map_err(sentence)? else {
        return Ok((wire, None, None));
    };
    let rpc = session.rpc().map_err(sentence)?;
    let (adapted, last_valid_height) = wallet_flows::adapt::rebuild(rpc.as_ref(), &tx, &plan)
        .await
        .map_err(sentence)?;
    Ok((adapted.wire.clone(), Some(adapted), Some(last_valid_height)))
}

fn log_rebuild(session: Session, flow: u64, adapted: &Adapted, from_message: &str) {
    let cluster = session.cluster();
    TxLog {
        flow,
        wire: &adapted.wire,
        cluster: Some(&cluster),
    }
    .log_transformed(transform_name(adapted), from_message);
}

/// Hex SHA-256 of the message in `wire`, as the log's `message` field has it.
fn message_hash(wire: &[u8]) -> String {
    parse_wire(wire)
        .map(|view| hex::encode(sha256_bytes(&view.message)))
        .unwrap_or_default()
}

fn transform_name(adapted: &Adapted) -> &'static str {
    if adapted
        .steps
        .iter()
        .any(|s| matches!(s, AdaptedStep::Squads { .. }))
    {
        "rebuild_squads"
    } else if adapted
        .steps
        .iter()
        .any(|s| matches!(s, AdaptedStep::Vector { .. }))
    {
        "rebuild_vector"
    } else if adapted
        .steps
        .iter()
        .any(|s| matches!(s, AdaptedStep::Programmatic { .. }))
    {
        "rebuild_programmatic"
    } else {
        "rebuild"
    }
}

/// The same request with a hot wallet of this device paying the fee.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, PartialEq)]
pub struct PaidHere {
    /// The fee payer the site named.
    pub from: Pubkey,
    pub to: Pubkey,
    pub wire: Vec<u8>,
    pub adapted: Option<Adapted>,
    pub flow: u64,
    /// The site's wire with only the payer swapped, before any rebuild.
    swapped: Vec<u8>,
    /// Message hash of the site's wire the swap started from.
    from_message: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl PaidHere {
    /// Log the swap, and the rebuild after it, once the user takes the offer.
    /// An offer left alone leaves no `transformed` line behind.
    pub fn log_taken(&self, session: Session) {
        let cluster = session.cluster();
        TxLog {
            flow: self.flow,
            wire: &self.swapped,
            cluster: Some(&cluster),
        }
        .log_transformed("payer_swap", &self.from_message);
        if let Some(adapted) = &self.adapted {
            log_rebuild(session, self.flow, adapted, &message_hash(&self.swapped));
        }
    }
}

/// Replace the site's fee payer with a hot wallet when that is possible, and
/// keep the result only if the node says it would succeed. A transaction
/// that holds a signature already, such as one made here for a one-time key,
/// is left alone: the swap would void it. Nothing is logged here:
/// [`PaidHere::log_taken`] does it if the offer is used.
#[cfg(not(target_arch = "wasm32"))]
pub async fn offer_paid_here(session: Session, wire: Vec<u8>, flow: u64) -> Option<PaidHere> {
    let tx = decode_v1_transaction(&wire).ok()?;
    if tx.signers.iter().any(|slot| slot.valid) {
        return None;
    }
    let from_message = hex::encode(sha256_bytes(&tx.message));
    let swap = session.plan_payer_swap(&tx)?;
    let rpc = session.rpc().ok()?;
    let swapped = wallet_flows::adapt::swap_payer_if_it_works(rpc.as_ref(), &tx, &swap).await?;
    let (wire, adapted, _) = rebuild_quietly(session, swapped.clone()).await.ok()?;
    Some(PaidHere {
        from: swap.from,
        to: swap.to,
        wire,
        adapted,
        flow,
        swapped,
        from_message,
    })
}
