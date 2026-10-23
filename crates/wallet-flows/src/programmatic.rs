//! Programmatic signers: setting up what one needs before it can act.

use crate::compile::compile_fresh;
use crate::error::FlowError;
use solana_pubkey::Pubkey;
use wallet_core::programmatic::{create_executor_nonce, EXECUTOR_NONCE_RENT_LAMPORTS};
use wallet_core::{SigningRoute, SolanaRpc};

/// A nonce account that does not exist yet, and the transaction that creates it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewNonce {
    pub account: Pubkey,
    pub wire: Vec<u8>,
}

/// Build the transaction that creates the executor nonce account of the
/// signer that acts through `route`. There is one per signer, at an address
/// derived from the wallet that pays for it.
pub async fn create_nonce(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
) -> Result<NewNonce, FlowError> {
    let Some((_, payer)) = route.fee_payer else {
        return Err(FlowError::NoFeePayer);
    };
    let text = route.address.to_string();
    let seed = format!("airlock:{}", &text[..text.len().min(16)]);
    let (account, instructions) = create_executor_nonce(
        &payer,
        &payer,
        &seed,
        &route.address,
        EXECUTOR_NONCE_RENT_LAMPORTS,
    )
    .map_err(FlowError::refused)?;
    let (wire, _) = compile_fresh(rpc, payer, instructions.to_vec()).await?;
    Ok(NewNonce { account, wire })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::{decode_v1_transaction, RouteStep, WalletId};

    fn route(payer: Option<Pubkey>) -> SigningRoute {
        SigningRoute {
            address: pk(7),
            steps: vec![RouteStep::OutsideKey { pubkey: pk(5) }],
            fee_payer: payer.map(|payer| (WalletId::new(), payer)),
            durable_nonce: None,
        }
    }

    #[test]
    fn the_nonce_account_is_derived_from_the_payer_and_the_payer_pays() {
        let rpc = FakeRpc::default();
        let made = pollster::block_on(create_nonce(&rpc, &route(Some(pk(2))))).unwrap();
        let again = pollster::block_on(create_nonce(&rpc, &route(Some(pk(2))))).unwrap();
        assert_eq!(made.account, again.account);
        let other = pollster::block_on(create_nonce(&rpc, &route(Some(pk(3))))).unwrap();
        assert_ne!(made.account, other.account);
        let tx = decode_v1_transaction(&made.wire).unwrap();
        assert_eq!(tx.fee_payer(), Some(pk(2)));
        assert!(tx
            .accounts
            .iter()
            .any(|account| account.pubkey == made.account));
    }

    #[test]
    fn a_signer_nobody_pays_for_cannot_get_a_nonce() {
        let err = pollster::block_on(create_nonce(&FakeRpc::default(), &route(None))).unwrap_err();
        assert_eq!(err, FlowError::NoFeePayer);
    }
}
