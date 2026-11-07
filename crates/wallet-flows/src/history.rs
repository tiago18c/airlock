//! Reading what the wallets did on chain: the latest transactions that named
//! each address, then each of those in full. A caller keeps what it read, so
//! a transaction is read in full once.

use solana_pubkey::Pubkey;
use solana_signature::Signature;
use wallet_core::{AddressSignature, LandedTx, RpcError, SolanaRpc};

/// How many of an address's latest transactions are listed.
pub const HISTORY_PAGE: usize = 25;

/// How many transactions one read takes in full at most. The rest wait for
/// the next read.
pub const DETAILS_PER_READ: usize = 20;

/// What a listing found.
#[derive(Debug, Default)]
pub struct Listed {
    /// Each address listed, with its latest transactions, newest first.
    pub addresses: Vec<(Pubkey, Vec<AddressSignature>)>,
    /// Why it stopped before the last address. What was listed stands.
    pub stopped: Option<RpcError>,
}

/// The latest `limit` transactions of each of `addresses`, in turn. A node
/// that refuses one stops the listing there.
pub async fn list(rpc: &dyn SolanaRpc, addresses: &[Pubkey], limit: usize) -> Listed {
    let from: Vec<(Pubkey, Option<Signature>)> =
        addresses.iter().map(|address| (*address, None)).collect();
    list_from(rpc, &from, limit).await
}

/// The `limit` transactions of each address that came before the one given
/// with it, the oldest kept of it, in turn. Fewer than `limit` means that was
/// the first it ever took part in.
pub async fn list_older(rpc: &dyn SolanaRpc, from: &[(Pubkey, Signature)], limit: usize) -> Listed {
    let from: Vec<(Pubkey, Option<Signature>)> = from
        .iter()
        .map(|(address, before)| (*address, Some(*before)))
        .collect();
    list_from(rpc, &from, limit).await
}

async fn list_from(
    rpc: &dyn SolanaRpc,
    from: &[(Pubkey, Option<Signature>)],
    limit: usize,
) -> Listed {
    let mut out = Listed::default();
    for (address, before) in from {
        match rpc
            .get_address_history(address, limit, before.as_ref())
            .await
        {
            Ok(listed) => out.addresses.push((*address, listed)),
            Err(err) => {
                out.stopped = Some(err);
                break;
            }
        }
    }
    out
}

/// What reading transactions in full found.
#[derive(Debug, Default)]
pub struct Read {
    /// Each transaction read, and `None` for one the node does not have.
    pub landed: Vec<(Signature, Option<LandedTx>)>,
    /// Why it stopped before the last one. What was read stands.
    pub stopped: Option<RpcError>,
}

/// Each of `signatures` in full, in turn. A node that refuses one stops the
/// reading there.
pub async fn read(rpc: &dyn SolanaRpc, signatures: &[Signature]) -> Read {
    let mut out = Read::default();
    for signature in signatures {
        match rpc.get_landed_transaction(signature).await {
            Ok(landed) => out.landed.push((*signature, landed)),
            Err(err) => {
                out.stopped = Some(err);
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};

    fn listed(byte: u8, slot: u64) -> AddressSignature {
        AddressSignature {
            signature: Signature::from([byte; 64]),
            slot,
            block_time: Some(1_700_000_000),
            err: None,
        }
    }

    #[test]
    fn a_listing_takes_each_address_and_keeps_what_it_got_when_refused() {
        let mut rpc = FakeRpc::default();
        rpc.history
            .insert(pk(1), vec![listed(1, 30), listed(2, 20), listed(3, 10)]);
        let found = pollster::block_on(list(&rpc, &[pk(1), pk(2)], 2));
        assert_eq!(found.addresses.len(), 2);
        assert_eq!(found.addresses[0].1, vec![listed(1, 30), listed(2, 20)]);
        assert!(found.addresses[1].1.is_empty());
        assert!(found.stopped.is_none());

        let rpc = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        let found = pollster::block_on(list(&rpc, &[pk(1), pk(2)], 2));
        assert!(found.addresses.is_empty());
        assert_eq!(found.stopped, Some(RpcError::Offline));
    }

    #[test]
    fn an_older_listing_goes_on_from_the_transaction_given() {
        let mut rpc = FakeRpc::default();
        rpc.history.insert(
            pk(1),
            vec![listed(4, 40), listed(3, 30), listed(2, 20), listed(1, 10)],
        );
        let found = pollster::block_on(list_older(&rpc, &[(pk(1), Signature::from([3; 64]))], 5));
        assert_eq!(found.addresses[0].1, vec![listed(2, 20), listed(1, 10)]);
    }

    #[test]
    fn reading_says_which_the_node_does_not_have() {
        let mut rpc = FakeRpc::default();
        let have = Signature::from([1; 64]);
        let landed = LandedTx {
            slot: 5,
            block_time: None,
            wire: vec![1, 2, 3],
            err: None,
            fee: 5_000,
            loaded_writable: Vec::new(),
            loaded_readonly: Vec::new(),
            pre_balances: Vec::new(),
            post_balances: Vec::new(),
            token_balances: Vec::new(),
            logs: Vec::new(),
        };
        rpc.landed.insert(have, landed.clone());
        let missing = Signature::from([2; 64]);
        let read = pollster::block_on(read(&rpc, &[have, missing]));
        assert_eq!(read.landed, vec![(have, Some(landed)), (missing, None)]);
    }
}
