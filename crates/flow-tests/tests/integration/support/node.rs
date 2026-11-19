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

/// A transaction's error as a node's JSON has it: an instruction's error as
/// `{"InstructionError": [index, detail]}`, a program's own code as
/// `{"Custom": code}`, anything else by its name.
fn as_node_words(err: &solana_transaction::TransactionError) -> serde_json::Value {
    match err {
        solana_transaction::TransactionError::InstructionError(index, detail) => {
            let named = format!("{detail:?}");
            let code = named
                .strip_prefix("Custom(")
                .and_then(|rest| rest.strip_suffix(')'))
                .and_then(|code| code.parse::<u32>().ok());
            let detail = match code {
                Some(code) => serde_json::json!({ "Custom": code }),
                None => serde_json::Value::String(named),
            };
            serde_json::json!({ "InstructionError": [index, detail] })
        }
        other => serde_json::Value::String(format!("{other:?}")),
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

    /// As a node does with `sigVerify` off: a blockhash it is told to replace
    /// is not checked, the watched accounts' balances after are answered, and
    /// a failure is the runtime's error as a node words it in JSON.
    async fn simulate(&self, request: &SimulationRequest) -> Result<SimulationReply, RpcError> {
        let chain = self.chain();
        let (meta, err, balances) = match chain
            .simulate_wire_as_node(&request.wire, request.replace_recent_blockhash)
        {
            Ok(simulated) => {
                let balances = request
                    .watched
                    .iter()
                    .map(|watched| {
                        simulated
                            .post_accounts
                            .iter()
                            .find(|(address, _)| address == watched)
                            .map(|(_, account)| solana_account::ReadableAccount::lamports(account))
                            .unwrap_or_else(|| chain.balance(watched))
                    })
                    .collect();
                (simulated.meta, None, balances)
            }
            Err(failure) => {
                let err = as_node_words(&failure.err);
                let balances = request
                    .watched
                    .iter()
                    .map(|watched| chain.balance(watched))
                    .collect();
                (failure.meta, Some(err), balances)
            }
        };
        Ok(SimulationReply {
            err,
            logs: meta.logs,
            units_consumed: Some(meta.compute_units_consumed),
            loaded_accounts_data_size: Some(meta.loaded_accounts_data_size),
            balances,
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
