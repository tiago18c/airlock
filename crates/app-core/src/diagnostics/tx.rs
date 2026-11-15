//! Transaction stages under `airlock::tx`, each with a flow id.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use base64::Engine;
use solana_signature::{Signature, SIGNATURE_BYTES};
use tracing::{info, info_span};
use wallet_core::{parse_wire, sha256_bytes, Cluster};

static FLOW: AtomicU64 = AtomicU64::new(1);

pub fn next_flow_id() -> u64 {
    FLOW.fetch_add(1, Ordering::Relaxed)
}

/// Fields shared by every stage line.
pub struct TxLog<'a> {
    pub flow: u64,
    pub wire: &'a [u8],
    pub cluster: Option<&'a Cluster>,
}

/// Span, wire/cluster fields, and the seven common keys every stage line shares.
macro_rules! log_stage {
    ($self:expr, $stage:literal $(, $field:ident = %$value:expr)* $(,)?) => {{
        let this = $self;
        let _span = info_span!(target: "airlock::tx", "tx", flow = this.flow).entered();
        let (message, wire_b64, signatures) = wire_fields(this.wire);
        let (cluster_name, rpc) = cluster_fields(this.cluster);
        info!(
            target: "airlock::tx",
            stage = $stage,
            flow = this.flow,
            message = %message,
            wire = %wire_b64,
            signatures = %signatures,
            cluster = %cluster_name,
            rpc = %rpc,
            $($field = %$value,)*
            "transaction"
        );
    }};
}

impl<'a> TxLog<'a> {
    pub fn log_received(self, source: &str, origin: Option<&str>, code: Option<&str>) {
        log_stage!(
            self,
            "received",
            source = %source,
            origin = %origin.unwrap_or(""),
            code = %code.unwrap_or(""),
        );
    }

    pub fn log_transformed(self, transform: &str, from_message: &str) {
        log_stage!(
            self,
            "transformed",
            transform = %transform,
            from_message = %from_message,
        );
    }

    pub fn log_signed(self, by: &str) {
        log_stage!(self, "signed", by = %by);
    }

    pub fn log_returned(self, origin: &str, code: &str) {
        log_stage!(self, "returned", origin = %origin, code = %code);
    }

    /// Signed here for a resolution delivered `cosign`: held until the site
    /// adds its signatures, then sent under this flow (see [`hold_flow`]).
    pub fn log_held(self, origin: &str, code: &str) {
        log_stage!(self, "held", origin = %origin, code = %code);
    }

    pub fn log_sent(self, signature: &str) {
        log_stage!(self, "sent", signature = %signature);
    }

    pub fn log_rejected(self, error: &str) {
        log_stage!(self, "rejected", error = %error);
    }

    /// The node gave no answer, so it may have taken it.
    pub fn log_unanswered(self, signature: &str, error: &str) {
        log_stage!(self, "unanswered", signature = %signature, error = %error);
    }
}

/// Flows of transactions held for a site's signatures, by the SHA-256 of
/// their message, which the site's signatures leave as it is. Bounded: one
/// never sent drops out, and is then sent under a flow of its own.
static HELD_FLOWS: Mutex<VecDeque<([u8; 32], u64)>> = Mutex::new(VecDeque::new());
const HELD_FLOWS_KEPT: usize = 64;

fn message_hash(wire: &[u8]) -> Option<[u8; 32]> {
    parse_wire(wire)
        .ok()
        .map(|view| sha256_bytes(&view.message))
}

/// Remember that `wire`, signed here and held for the site's signatures,
/// belongs to `flow`, so what is sent once the site has signed is logged
/// under the same flow ([`held_flow`]).
pub fn hold_flow(wire: &[u8], flow: u64) {
    let Some(hash) = message_hash(wire) else {
        return;
    };
    let mut held = HELD_FLOWS.lock().unwrap_or_else(PoisonError::into_inner);
    held.retain(|(kept, _)| *kept != hash);
    if held.len() >= HELD_FLOWS_KEPT {
        held.pop_front();
    }
    held.push_back((hash, flow));
}

/// The flow [`hold_flow`] kept for `wire`'s message, taken out.
pub fn held_flow(wire: &[u8]) -> Option<u64> {
    let hash = message_hash(wire)?;
    let mut held = HELD_FLOWS.lock().unwrap_or_else(PoisonError::into_inner);
    let at = held.iter().position(|(kept, _)| *kept == hash)?;
    held.remove(at).map(|(_, flow)| flow)
}

/// A tracker (or any status update) logs the outcome under `airlock::tx`.
pub fn log_status(flow: u64, signature: &Signature, status: &crate::notices::TxStatus) {
    let _span = info_span!(target: "airlock::tx", "tx", flow).entered();
    info!(
        target: "airlock::tx",
        stage = "status",
        flow,
        signature = %signature,
        status = status.as_str(),
        error = status.error().unwrap_or(""),
        slot = status.slot().unwrap_or(0),
        "transaction"
    );
}

fn wire_fields(wire: &[u8]) -> (String, String, String) {
    let wire_b64 = base64::engine::general_purpose::STANDARD.encode(wire);
    let Ok(view) = parse_wire(wire) else {
        return (hex::encode(sha256_bytes(wire)), wire_b64, String::new());
    };
    let message = hex::encode(sha256_bytes(&view.message));
    let mut parts = Vec::new();
    for (i, offset) in view.signature_offsets.iter().enumerate() {
        if !view.slot_valid(i) {
            continue;
        }
        let Some(pubkey) = view.static_keys.get(i) else {
            continue;
        };
        let Some(bytes) = view.wire.get(*offset..*offset + SIGNATURE_BYTES) else {
            continue;
        };
        let sig = bs58_encode(bytes);
        parts.push(format!("{pubkey}={sig}"));
    }
    (message, wire_b64, parts.join(","))
}

/// The cluster's name, and its redacted RPC URL only when it is custom: a
/// public cluster's name already says which node it is.
fn cluster_fields(cluster: Option<&Cluster>) -> (String, String) {
    match cluster {
        Some(cluster) if cluster.is_custom() => (
            cluster.name().to_string(),
            super::redact_url(cluster.rpc_url()),
        ),
        Some(cluster) => (cluster.name().to_string(), String::new()),
        None => (String::new(), String::new()),
    }
}

fn bs58_encode(bytes: &[u8]) -> String {
    match <[u8; SIGNATURE_BYTES]>::try_from(bytes) {
        Ok(arr) => Signature::from(arr).to_string(),
        Err(_) => hex::encode(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A legacy memo by `payer`, with its one signature slot set to `sig`.
    fn memo(payer: u8, text: &[u8], sig: u8) -> Vec<u8> {
        let mut wire = vec![1];
        wire.extend([sig; 64]);
        wire.extend([1, 0, 1, 2]);
        wire.extend([payer; 32]);
        wire.extend([9; 32]);
        wire.extend([7; 32]);
        wire.extend([1, 1, 1, 0, u8::try_from(text.len()).unwrap()]);
        wire.extend(text);
        wire
    }

    #[test]
    fn a_held_flow_is_found_again_once_the_site_has_signed() {
        let held = memo(1, b"held for the site", 0);
        hold_flow(&held, 41);
        // The site's signature changes the wire, not the message.
        let signed = memo(1, b"held for the site", 5);
        assert_eq!(held_flow(&memo(2, b"held for the site", 5)), None);
        assert_eq!(held_flow(&signed), Some(41));
        // Taken once.
        assert_eq!(held_flow(&signed), None);
        assert_eq!(held_flow(b"not a transaction"), None);
    }
}
