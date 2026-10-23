//! Reading what a node knows about a set of addresses.

use solana_pubkey::Pubkey;
use wallet_core::{parse_nonce_account, ChainSnapshot, Lifetime, RpcError, SolanaRpc};

/// Fetch the one thing a transaction's lifetime needs, in one request: the
/// state of `nonce` when a durable nonce is used, the latest blockhash
/// otherwise.
pub async fn fetch_lifetime(
    rpc: &dyn SolanaRpc,
    nonce: Option<Pubkey>,
) -> Result<Lifetime, RpcError> {
    let Some(pubkey) = nonce else {
        return rpc.get_latest_blockhash().await.map(Lifetime::Blockhash);
    };
    let account = rpc
        .get_account(&pubkey)
        .await?
        .ok_or_else(|| RpcError::Message(format!("Nonce account {pubkey} does not exist.")))?;
    parse_nonce_account(pubkey, &account)
        .map(Lifetime::Nonce)
        .ok_or_else(|| RpcError::Message(format!("{pubkey} is not an initialized nonce account.")))
}

/// A node answers for at most this many accounts in one request.
const ACCOUNTS_PER_REQUEST: usize = 100;

/// The lamports of each of `owners`, one request per hundred. An address
/// with no account holds nothing.
pub async fn read_balances(
    rpc: &dyn SolanaRpc,
    owners: &[Pubkey],
) -> Result<Vec<(Pubkey, u64)>, RpcError> {
    let mut balances = Vec::with_capacity(owners.len());
    for batch in owners.chunks(ACCOUNTS_PER_REQUEST) {
        let held = rpc.get_lamports(batch).await?;
        balances.extend(
            batch
                .iter()
                .zip(held)
                .map(|(owner, lamports)| (*owner, lamports.unwrap_or(0))),
        );
    }
    Ok(balances)
}

/// [`read_balances`] from a node that has seen at least `min_slot`, with
/// the oldest slot a request was answered at.
pub async fn read_balances_since(
    rpc: &dyn SolanaRpc,
    owners: &[Pubkey],
    min_slot: u64,
) -> Result<(u64, Vec<(Pubkey, u64)>), RpcError> {
    let mut balances = Vec::with_capacity(owners.len());
    let mut slot = u64::MAX;
    for batch in owners.chunks(ACCOUNTS_PER_REQUEST) {
        let (answered, held) = rpc.get_lamports_since(batch, min_slot).await?;
        slot = slot.min(answered);
        balances.extend(
            batch
                .iter()
                .zip(held)
                .map(|(owner, lamports)| (*owner, lamports.unwrap_or(0))),
        );
    }
    Ok((if balances.is_empty() { min_slot } else { slot }, balances))
}

/// The token accounts of each of `owners`.
///
/// An owner whose request the node refused is left out of
/// [`ChainSnapshot::token_owners`], so what the app holds for it is kept. A
/// node that does not answer at all stops the read: asking it about the next
/// owner would only wait as long again.
pub async fn read_tokens(
    rpc: &dyn SolanaRpc,
    owners: &[Pubkey],
) -> Result<ChainSnapshot, RpcError> {
    read_tokens_since(rpc, owners, 0).await
}

/// [`read_tokens`] from a node that has seen at least `min_slot`. A node
/// behind it stops the read with [`RpcError::Behind`], to be asked again.
pub async fn read_tokens_since(
    rpc: &dyn SolanaRpc,
    owners: &[Pubkey],
    min_slot: u64,
) -> Result<ChainSnapshot, RpcError> {
    let mut snapshot = ChainSnapshot::default();
    for owner in owners {
        match rpc.get_token_accounts_by_owner_since(owner, min_slot).await {
            Ok(mut tokens) => {
                snapshot.tokens.append(&mut tokens);
                snapshot.token_owners.push(*owner);
            }
            Err(err) if err.is_down() || matches!(err, RpcError::Behind) => return Err(err),
            Err(_) => {}
        }
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{nonce_account, pk, FakeRpc};
    use solana_hash::Hash;
    use wallet_core::{DiscoveredNonce, DiscoveredTokenAccount, Holdings, TokenFlag};

    #[test]
    fn a_failed_token_request_keeps_stored_holdings_and_flags() {
        let (owner, token) = (pk(1), pk(2));
        let mut holdings = Holdings::default();
        holdings.replace_owners(
            &[owner],
            &[DiscoveredTokenAccount {
                pubkey: token,
                owner,
                mint: pk(3),
                amount: 5,
                decimals: 0,
            }],
        );
        assert!(holdings.set_flag(&owner, &token, Some(TokenFlag::Red)));

        let rpc = FakeRpc {
            tokens_fail: true,
            ..FakeRpc::default()
        };
        let snapshot = pollster::block_on(read_tokens(&rpc, &[owner])).unwrap();
        assert!(snapshot.token_owners.is_empty());

        holdings.replace_owners(&snapshot.token_owners, &snapshot.tokens);
        assert_eq!(holdings.flag_of(&token), Some(TokenFlag::Red));
    }

    #[test]
    fn a_lifetime_is_one_request_for_the_blockhash_or_the_nonce() {
        let (authority, nonce) = (pk(7), pk(3));
        let (account, hash) = nonce_account(authority);
        let rpc = FakeRpc::default().with_account(nonce, account);

        let recent = pollster::block_on(fetch_lifetime(&rpc, None)).unwrap();
        assert_eq!(recent, Lifetime::Blockhash(Hash::new_from_array([9; 32])));
        assert_eq!(rpc.asked(), 1);

        let found = pollster::block_on(fetch_lifetime(&rpc, Some(nonce))).unwrap();
        assert_eq!(
            found,
            Lifetime::Nonce(DiscoveredNonce {
                pubkey: nonce,
                authority,
                hash,
            })
        );
        assert_eq!(rpc.asked(), 2);

        let err = pollster::block_on(fetch_lifetime(&FakeRpc::default(), Some(nonce))).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn balances_are_one_request_per_hundred_addresses() {
        let owners: Vec<Pubkey> = (0..150u8).map(pk).collect();
        let rpc = FakeRpc::default().with_balance(pk(3), 7);
        let balances = pollster::block_on(read_balances(&rpc, &owners)).unwrap();
        assert_eq!(rpc.asked(), 2);
        assert_eq!(balances.len(), 150);
        assert_eq!(balances[3], (pk(3), 7));
        assert_eq!(balances[4], (pk(4), 0));
    }

    #[test]
    fn a_node_that_is_down_stops_the_token_read() {
        let rpc = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        let err = pollster::block_on(read_tokens(&rpc, &[pk(1), pk(2)])).unwrap_err();
        assert_eq!(err, RpcError::Offline);
        assert_eq!(rpc.asked(), 1);
    }
}
