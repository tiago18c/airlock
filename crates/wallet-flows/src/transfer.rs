//! Sending SOL from a wallet to another address.

use crate::compile::{compile_for_route, with_nonce_note};
use crate::error::{FlowError, Prepared};
use solana_pubkey::Pubkey;
use wallet_core::{format_sol, sol_transfer, Phrase, RouteStep, SigningRoute, SolanaRpc};

/// A transfer of `lamports` from the wallet whose route is given to `to`, on
/// a fresh blockhash or the wallet's durable nonce. Returns it and the
/// height at which a blockhash expires.
///
/// The wallet's fee payer pays. A plain key with none set pays for itself.
/// A programmatic signer, a Vector account or a multisig vault is named as
/// the sender as it is; Sign rebuilds the transfer around it.
pub async fn prepare(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
    to: Pubkey,
    lamports: u64,
) -> Result<(Prepared, Option<u64>), FlowError> {
    if lamports == 0 {
        return Err(FlowError::refused("Enter an amount above zero."));
    }
    let from = route.address;
    if to == from {
        return Err(FlowError::refused("The recipient is this wallet."));
    }
    let payer = fee_payer(route)?;
    let (wire, last_valid_height) =
        compile_for_route(rpc, route, payer, vec![sol_transfer(&from, &to, lamports)]).await?;
    let note = Phrase::default()
        .t(format!("This sends {} from ", format_sol(lamports)))
        .a(from)
        .t(" to ")
        .a(to)
        .t(".");
    let note = with_nonce_note(note, route);
    Ok((Prepared { wire, note }, last_valid_height))
}

/// Whether the wallet of `route` is a key that signs for itself, rather than
/// an address Sign wraps the transaction around.
pub fn is_plain_key(route: &SigningRoute) -> bool {
    matches!(
        route.steps.as_slice(),
        [RouteStep::Key { .. } | RouteStep::OutsideKey { .. }]
    )
}

/// Who pays the fee of a transaction the wallet of `route` sends: the fee
/// payer set for it, or a plain key itself.
pub fn fee_payer(route: &SigningRoute) -> Result<Pubkey, FlowError> {
    match route.fee_payer {
        Some((_, payer)) => Ok(payer),
        None if is_plain_key(route) => Ok(route.address),
        None => Err(FlowError::NoFeePayer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::{decode_v1_transaction, WalletId};

    fn route(owner: Pubkey, payer: Option<Pubkey>) -> SigningRoute {
        SigningRoute {
            address: owner,
            steps: vec![RouteStep::OutsideKey { pubkey: owner }],
            fee_payer: payer.map(|payer| (WalletId::new(), payer)),
            durable_nonce: None,
        }
    }

    #[test]
    fn an_offline_key_with_a_durable_nonce_sends_on_it_and_nothing_expires() {
        let (account, hash) = crate::testing::nonce_account(pk(1));
        let rpc = FakeRpc::default().with_account(pk(7), account);
        let mut route = route(pk(1), Some(pk(3)));
        route.durable_nonce = Some(pk(7));
        let (prepared, height) = pollster::block_on(prepare(&rpc, &route, pk(2), 5)).unwrap();
        assert_eq!(height, None);
        let tx = decode_v1_transaction(&prepared.wire).unwrap();
        assert!(tx.uses_durable_nonce);
        assert_eq!(tx.lifetime, hash);
        assert_eq!(tx.fee_payer(), Some(pk(3)));
        assert!(prepared.note.plain().contains("durable nonce"));

        // A nonce the key does not control is refused, not signed for nothing.
        let (foreign, _) = crate::testing::nonce_account(pk(9));
        let rpc = FakeRpc::default().with_account(pk(7), foreign);
        assert!(pollster::block_on(prepare(&rpc, &route, pk(2), 5)).is_err());
    }

    #[test]
    fn a_key_without_a_fee_payer_pays_for_itself() {
        let rpc = FakeRpc::default();
        let (prepared, _) =
            pollster::block_on(prepare(&rpc, &route(pk(1), None), pk(2), 5)).unwrap();
        let tx = decode_v1_transaction(&prepared.wire).unwrap();
        assert_eq!(tx.fee_payer(), Some(pk(1)));
        assert!(prepared.note.plain().contains("SOL"));
    }

    #[test]
    fn the_fee_payer_set_on_the_wallet_pays() {
        let rpc = FakeRpc::default();
        let (prepared, _) =
            pollster::block_on(prepare(&rpc, &route(pk(1), Some(pk(3))), pk(2), 5)).unwrap();
        let tx = decode_v1_transaction(&prepared.wire).unwrap();
        assert_eq!(tx.fee_payer(), Some(pk(3)));
    }

    #[test]
    fn nothing_and_sending_to_itself_are_refused() {
        let rpc = FakeRpc::default();
        assert!(pollster::block_on(prepare(&rpc, &route(pk(1), None), pk(2), 0)).is_err());
        assert!(pollster::block_on(prepare(&rpc, &route(pk(1), None), pk(1), 5)).is_err());
    }
}
