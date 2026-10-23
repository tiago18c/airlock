//! The local chain behind the port the app reaches a node through.

use super::{logs, Chain};
use async_trait::async_trait;
use solana_hash::Hash;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use std::sync::Mutex;
use wallet_core::simulate::{SimulationReply, SimulationRequest};
use wallet_core::stake::{EpochInfo, StakeReward};
use wallet_core::{DiscoveredTokenAccount, RawAccount, RpcError, SignatureStatus, SolanaRpc};

/// It answers what a node reads from accounts, and simulates as a node does,
/// without checking signatures. It refuses what a node keeps an index for,
/// and makes up the history of an address from whether it has an account.
pub struct Node(pub Mutex<Chain>);

impl Node {
    pub fn chain(&self) -> std::sync::MutexGuard<'_, Chain> {
        self.0.lock().unwrap()
    }
}

fn not_served<T>(what: &str) -> Result<T, RpcError> {
    Err(RpcError::Message(format!(
        "The test chain does not serve {what}."
    )))
}

#[async_trait]
impl SolanaRpc for Node {
    async fn get_balance(&self, pubkey: &Pubkey) -> Result<u64, RpcError> {
        Ok(self.chain().balance(pubkey))
    }

    async fn get_account(&self, pubkey: &Pubkey) -> Result<Option<RawAccount>, RpcError> {
        Ok(self.chain().account(pubkey))
    }

    async fn get_token_accounts_by_owner(
        &self,
        _owner: &Pubkey,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        not_served("token accounts by owner")
    }

    async fn get_program_accounts(
        &self,
        _program: &Pubkey,
        _data_size: u64,
        _offset: usize,
        _address: &Pubkey,
    ) -> Result<Vec<(Pubkey, RawAccount)>, RpcError> {
        not_served("program accounts")
    }

    async fn get_latest_blockhash(&self) -> Result<Hash, RpcError> {
        self.get_latest_blockhash_with_height()
            .await
            .map(|(hash, _)| hash)
    }

    async fn get_latest_blockhash_with_height(&self) -> Result<(Hash, u64), RpcError> {
        Ok((self.chain().latest_blockhash(), 150))
    }

    async fn get_block_height(&self) -> Result<u64, RpcError> {
        Ok(0)
    }

    async fn is_blockhash_valid(&self, _blockhash: &Hash) -> Result<bool, RpcError> {
        Ok(true)
    }

    async fn send_transaction(&self, wire: &[u8]) -> Result<Signature, RpcError> {
        // A node answers with the fee payer's signature: the first after the message.
        let message = wallet_core::decode_v1_transaction(wire)
            .map_err(|err| RpcError::Message(err.to_string()))?
            .message;
        let first = wire
            .get(message.len()..message.len() + wallet_core::SIGNATURE_BYTES)
            .and_then(|bytes| Signature::try_from(bytes).ok())
            .unwrap_or_default();
        self.chain()
            .send_wire(wire)
            .map(|_| first)
            .map_err(|failure| RpcError::Message(logs(&failure)))
    }

    async fn get_signature_statuses(
        &self,
        _signatures: &[Signature],
    ) -> Result<Vec<Option<SignatureStatus>>, RpcError> {
        not_served("signature statuses")
    }

    /// The test chain keeps no history: an address with an account counts as
    /// used, and any other as never seen.
    async fn get_signatures_for_address(
        &self,
        address: &Pubkey,
        limit: usize,
    ) -> Result<Vec<Signature>, RpcError> {
        Ok(match self.chain().account(address) {
            Some(_) => vec![Signature::default(); limit.min(1)],
            None => Vec::new(),
        })
    }

    async fn simulate(&self, request: &SimulationRequest) -> Result<SimulationReply, RpcError> {
        if !request.watched.is_empty() {
            return not_served("balances after a simulation");
        }
        let (meta, err) = match self.chain().simulate_wire(&request.wire) {
            Ok(meta) => (meta, None),
            Err(failure) => (
                failure.meta,
                Some(serde_json::Value::String(format!("{:?}", failure.err))),
            ),
        };
        Ok(SimulationReply {
            err,
            logs: meta.logs,
            units_consumed: Some(meta.compute_units_consumed),
            loaded_accounts_data_size: Some(meta.loaded_accounts_data_size),
            ..SimulationReply::default()
        })
    }

    async fn get_epoch_info(&self) -> Result<EpochInfo, RpcError> {
        not_served("the epoch")
    }

    async fn get_rent_exemption(&self, data_len: usize) -> Result<u64, RpcError> {
        Ok(self.chain().rent(data_len))
    }

    async fn get_stake_minimum_delegation(&self) -> Result<u64, RpcError> {
        not_served("the minimum delegation")
    }

    async fn get_stake_rewards(
        &self,
        _accounts: &[Pubkey],
        _epoch: u64,
    ) -> Result<Vec<StakeReward>, RpcError> {
        not_served("stake rewards")
    }
}
