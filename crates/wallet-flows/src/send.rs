//! Sending a transaction to a node.

use solana_signature::Signature;
use wallet_core::{RpcError, SolanaRpc, V1_PREFIX};

/// Submit a compiled v1 transaction (`0x81` wire, signatures at the tail).
/// Anything else is refused before it reaches the node.
pub async fn submit(rpc: &dyn SolanaRpc, wire: &[u8]) -> Result<Signature, RpcError> {
    if wire.first().copied() != Some(V1_PREFIX) {
        return Err(RpcError::Message(
            "payload is not a Solana transaction v1 (expected 0x81 prefix)".into(),
        ));
    }
    rpc.send_transaction(wire).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, transfer, FakeRpc};

    #[test]
    fn only_a_v1_wire_reaches_the_node() {
        let rpc = FakeRpc::default();
        let err = pollster::block_on(submit(&rpc, &[1, 2, 3])).unwrap_err();
        assert!(err.to_string().contains("0x81"));
        assert_eq!(rpc.asked(), 0);

        let wire = transfer(pk(1), pk(2)).wire;
        assert!(pollster::block_on(submit(&rpc, &wire)).is_ok());
        assert_eq!(rpc.asked(), 1);
    }
}
