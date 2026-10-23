//! Asking a node what a transaction would do.

use solana_pubkey::Pubkey;
use wallet_core::simulate::{read_simulation, watched_accounts, Simulation, SimulationRequest};
use wallet_core::{DecodedTx, RpcError, SolanaRpc};

/// A node answers for at most this many accounts in one request.
const ACCOUNTS_PER_REQUEST: usize = 100;

/// Ask the node what `tx` would do if it were sent now, with the balances
/// that would change. Nothing is sent, and the transaction does not have to
/// be signed.
pub async fn simulate(rpc: &dyn SolanaRpc, tx: &DecodedTx) -> Result<Simulation, RpcError> {
    let watched = watched_accounts(tx);
    let before = balances(rpc, &watched).await?;
    let reply = rpc.simulate(&SimulationRequest::new(tx, &watched)).await?;
    Ok(read_simulation(reply, tx, &watched, &before))
}

/// Simulate `tx` only to learn what it uses: one request, no balances.
pub async fn measure(rpc: &dyn SolanaRpc, tx: &DecodedTx) -> Result<Simulation, RpcError> {
    let reply = rpc.simulate(&SimulationRequest::new(tx, &[])).await?;
    Ok(read_simulation(reply, tx, &[], &[]))
}

/// Lamports of each of `pubkeys`, in order. An account that does not exist
/// holds nothing.
async fn balances(rpc: &dyn SolanaRpc, pubkeys: &[Pubkey]) -> Result<Vec<u64>, RpcError> {
    let mut out = Vec::with_capacity(pubkeys.len());
    for batch in pubkeys.chunks(ACCOUNTS_PER_REQUEST) {
        let held = rpc.get_lamports(batch).await?;
        out.extend(held.into_iter().map(Option::unwrap_or_default));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, transfer, FakeRpc};
    use wallet_core::simulate::{BalanceChange, SimulationReply};

    #[test]
    fn a_simulation_reads_balances_first_and_reports_what_changes() {
        let rpc = FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                balances: vec![90, 5],
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        }
        .with_balance(pk(1), 100);
        let simulation = pollster::block_on(simulate(&rpc, &transfer(pk(1), pk(2)))).unwrap();
        assert_eq!(simulation.units_consumed, Some(150));
        assert_eq!(
            simulation.balance_changes,
            vec![
                BalanceChange {
                    pubkey: pk(1),
                    before: 100,
                    after: 90
                },
                BalanceChange {
                    pubkey: pk(2),
                    before: 0,
                    after: 5
                },
            ]
        );
        // Balances, then the simulation.
        assert_eq!(rpc.asked(), 2);
    }

    #[test]
    fn measuring_is_one_request_and_lists_no_balances() {
        let rpc = FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        };
        let simulation = pollster::block_on(measure(&rpc, &transfer(pk(1), pk(2)))).unwrap();
        assert_eq!(simulation.units_consumed, Some(150));
        assert!(simulation.balance_changes.is_empty());
        assert_eq!(rpc.asked(), 1);
    }
}
