//! A payload pasted as text.

use super::*;
use base64::Engine;

/// The objects of pasted text: JSON of a payload, or a transaction alone in
/// base64 or base58, as other tools print one. `is_transaction` says whether
/// bytes are a transaction the caller can read, in any format.
pub fn parse_payload_text(
    text: &str,
    is_transaction: impl Fn(&[u8]) -> bool,
) -> Result<Vec<QrObject>, ProtocolError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ProtocolError::Empty);
    }
    if text.starts_with('{') || text.starts_with('[') {
        return parse_payload_json(text);
    }
    [
        base64::engine::general_purpose::STANDARD.decode(text).ok(),
        bs58::decode(text).into_vec().ok(),
    ]
    .into_iter()
    .flatten()
    .find(|bytes| is_transaction(bytes))
    .map(|wire| vec![QrObject::Transaction(wire)])
    .ok_or(ProtocolError::BadObject)
}
