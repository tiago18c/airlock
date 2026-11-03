//! Transaction stages under `airlock::tx`, each with a flow id.

use std::sync::atomic::{AtomicU64, Ordering};

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
