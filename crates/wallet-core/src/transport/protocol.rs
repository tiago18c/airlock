//! The QR transfer protocol of the `qr-protocol` crate, with a transaction
//! taken to be what this crate decodes.

pub use qr_protocol::*;

use crate::tx::decode::parse_wire;

// The protocol crate writes Solana's sizes out. They have to stay Solana's.
const _: () = {
    assert!(qr_protocol::V1_PREFIX == solana_message::v1::V1_PREFIX);
    assert!(qr_protocol::MAX_TRANSACTION_SIZE == solana_message::v1::MAX_TRANSACTION_SIZE);
    assert!(qr_protocol::PUBKEY_BYTES == solana_pubkey::PUBKEY_BYTES);
    assert!(qr_protocol::SIGNATURE_BYTES == solana_signature::SIGNATURE_BYTES);
};

/// The objects of pasted text: base64 of a payload, or a transaction alone
/// in base64 or base58, legacy, v0 or v1.
pub fn parse_payload_text(text: &str) -> Result<Vec<QrObject>, ProtocolError> {
    qr_protocol::parse_payload_text(text, |wire| parse_wire(wire).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{compile, pk, transfer};
    use base64::Engine;

    #[test]
    fn pasted_text_is_read_as_the_transaction_that_decodes() {
        let wire = compile(pk(1), vec![transfer(pk(1), pk(2), 5)]).wire;
        let text = base64::engine::general_purpose::STANDARD.encode(&wire);
        assert_eq!(
            parse_payload_text(&text).unwrap(),
            vec![QrObject::Transaction(wire)]
        );
    }
}
