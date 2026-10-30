//! Sending a transaction to a node.

use solana_signature::Signature;
use wallet_core::{parse_wire, RpcError, SolanaRpc};

/// Submit a signed transaction in any format the node takes: legacy, v0 or
/// v1. Bytes that are not a transaction are refused before they reach the
/// node.
pub async fn submit(rpc: &dyn SolanaRpc, wire: &[u8]) -> Result<Signature, RpcError> {
    if parse_wire(wire).is_err() {
        return Err(RpcError::Message(
            "payload is not a Solana transaction".into(),
        ));
    }
    rpc.send_transaction(wire).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{legacy_transfer, pk, transfer, FakeRpc};

    #[test]
    fn only_a_transaction_reaches_the_node() {
        let rpc = FakeRpc::default();
        let err = pollster::block_on(submit(&rpc, &[1, 2, 3])).unwrap_err();
        assert!(err.to_string().contains("not a Solana transaction"));
        assert_eq!(rpc.asked(), 0);

        let wire = transfer(pk(1), pk(2)).wire;
        assert!(pollster::block_on(submit(&rpc, &wire)).is_ok());
        assert_eq!(rpc.asked(), 1);
    }

    #[test]
    fn a_legacy_transaction_is_sent_as_it_is() {
        let rpc = FakeRpc::default();
        let wire = legacy_transfer(pk(1), pk(2));
        assert!(pollster::block_on(submit(&rpc, &wire)).is_ok());
        assert_eq!(rpc.asked(), 1);
    }
}
