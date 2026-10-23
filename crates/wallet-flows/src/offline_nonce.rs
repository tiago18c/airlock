//! Offline wallets: the durable nonce a key signs its own transactions on.
//! Deprecated once the programmatic signer or Vector is on mainnet.

use crate::compile::compile_fresh;
use crate::error::FlowError;
use crate::programmatic::NewNonce;
use crate::refresh::fetch_lifetime;
use solana_pubkey::Pubkey;
use wallet_core::offline_nonce::{create_durable_nonce, DURABLE_NONCE_LEN};
use wallet_core::{onto_nonce, DecodedTx, Lifetime, NonceSpec, RouteStep, SigningRoute, SolanaRpc};

/// Build the transaction that creates the durable nonce account of the
/// offline key that acts through `route`, with that key as its authority.
/// Its fee payer pays the rent and is the base the address is derived from,
/// so the creation needs no signature from the offline key.
pub async fn create(rpc: &dyn SolanaRpc, route: &SigningRoute) -> Result<NewNonce, FlowError> {
    let [RouteStep::OutsideKey { pubkey }] = route.steps.as_slice() else {
        return Err(FlowError::refused(
            "Only an offline wallet that signs for itself gets a durable nonce.",
        ));
    };
    let Some((_, payer)) = route.fee_payer else {
        return Err(FlowError::NoFeePayer);
    };
    let rent = rpc.get_rent_exemption(DURABLE_NONCE_LEN).await?;
    let (account, instructions) =
        create_durable_nonce(&payer, pubkey, rent).map_err(FlowError::refused)?;
    let (wire, _) = compile_fresh(rpc, payer, instructions).await?;
    Ok(NewNonce { account, wire })
}

/// Move `tx`, which `key` has to sign, onto the durable nonce `nonce` of
/// that key, read from the chain. Refused when the nonce is controlled by
/// another key, and for what [`onto_nonce`] refuses.
pub async fn move_onto(
    rpc: &dyn SolanaRpc,
    tx: &DecodedTx,
    key: Pubkey,
    nonce: Pubkey,
) -> Result<Vec<u8>, FlowError> {
    let Lifetime::Nonce(found) = fetch_lifetime(rpc, Some(nonce)).await? else {
        return Err(FlowError::refused(format!(
            "Nonce account {nonce} could not be read."
        )));
    };
    if found.authority != key {
        return Err(FlowError::refused(format!(
            "The durable nonce {nonce} is controlled by {}, not by {key}.",
            found.authority
        )));
    }
    let spec = NonceSpec {
        account: found.pubkey,
        authority: found.authority,
        hash: found.hash,
    };
    onto_nonce(tx, &spec).map_err(FlowError::refused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::{decode_v1_transaction, WalletId};

    fn route(steps: Vec<RouteStep>, payer: Option<solana_pubkey::Pubkey>) -> SigningRoute {
        SigningRoute {
            address: pk(5),
            steps,
            fee_payer: payer.map(|payer| (WalletId::new(), payer)),
            durable_nonce: None,
        }
    }

    #[test]
    fn the_fee_payer_pays_for_the_nonce_of_the_offline_key() {
        let rpc = FakeRpc::default();
        let key = vec![RouteStep::OutsideKey { pubkey: pk(5) }];
        let made = pollster::block_on(create(&rpc, &route(key, Some(pk(2))))).unwrap();
        let tx = decode_v1_transaction(&made.wire).unwrap();
        assert_eq!(tx.fee_payer(), Some(pk(2)));
        // The offline key is named as the authority, and does not sign.
        assert_eq!(tx.signers.len(), 1);
        assert!(tx
            .accounts
            .iter()
            .any(|account| account.pubkey == made.account));
        assert_eq!(tx.instructions[1].data[4..36], pk(5).to_bytes());
    }

    #[test]
    fn a_request_moves_onto_the_nonce_of_its_key_and_no_other() {
        let (account, hash) = crate::testing::nonce_account(pk(5));
        let rpc = FakeRpc::default().with_account(pk(7), account);
        let wire = wallet_core::compile_v1(
            pk(2),
            vec![crate::testing::transfer_instruction(pk(5), pk(9))],
            None,
            Some(solana_hash::Hash::new_from_array([1; 32])),
        )
        .unwrap()
        .wire;
        let tx = decode_v1_transaction(&wire).unwrap();
        let moved = pollster::block_on(move_onto(&rpc, &tx, pk(5), pk(7))).unwrap();
        let after = decode_v1_transaction(&moved).unwrap();
        assert!(after.uses_durable_nonce);
        assert_eq!(after.lifetime, hash);
        assert!(pollster::block_on(move_onto(&rpc, &tx, pk(6), pk(7))).is_err());
    }

    #[test]
    fn only_an_offline_key_with_a_fee_payer_gets_one() {
        let rpc = FakeRpc::default();
        let key = vec![RouteStep::OutsideKey { pubkey: pk(5) }];
        let err = pollster::block_on(create(&rpc, &route(key, None))).unwrap_err();
        assert_eq!(err, FlowError::NoFeePayer);
        let hot = vec![RouteStep::Key {
            wallet: WalletId::new(),
            pubkey: pk(5),
        }];
        assert!(pollster::block_on(create(&rpc, &route(hot, Some(pk(2))))).is_err());
    }
}
