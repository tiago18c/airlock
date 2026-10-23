//! The browser backend: the vendored `solana-client-wasm`.

use crate::convert::{
    rpc_error, signature_status, simulation_reply, stake_reward, token_account, SimulationParts,
};
use solana_account_decoder_client_types::{UiAccountEncoding, UiDataSliceConfig};
use solana_client_wasm::utils::rpc_config::{
    RpcAccountInfoConfig, RpcProgramAccountsConfig, RpcSimulateTransactionAccountsConfig,
    RpcSimulateTransactionConfig,
};
use solana_client_wasm::utils::rpc_filter::{
    Memcmp, MemcmpEncodedBytes, RpcFilterType, TokenAccountsFilter,
};
use solana_client_wasm::WasmClient;
use solana_hash::Hash;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction_status_client_types::UiTransactionEncoding;
use std::time::Duration;
use wallet_core::simulate::{SimulationReply, SimulationRequest};
use wallet_core::stake::{EpochInfo, StakeReward};
use wallet_core::{DiscoveredTokenAccount, RawAccount, RpcError, SignatureStatus};

pub struct Backend {
    client: WasmClient,
}

impl Backend {
    pub fn new(url: &str, timeout: Duration) -> Self {
        Self {
            client: WasmClient::new_with_timeout(url, timeout),
        }
    }

    pub async fn genesis_hash(&self) -> Result<Hash, RpcError> {
        self.client.get_genesis_hash().await.map_err(rpc_error)
    }

    pub async fn latest_blockhash(&self) -> Result<Hash, RpcError> {
        self.latest_blockhash_with_height()
            .await
            .map(|(hash, _)| hash)
    }

    pub async fn latest_blockhash_with_height(&self) -> Result<(Hash, u64), RpcError> {
        self.client
            .get_latest_blockhash_with_commitment(self.client.commitment_config())
            .await
            .map_err(rpc_error)
    }

    pub async fn block_height(&self) -> Result<u64, RpcError> {
        self.client.get_block_height().await.map_err(rpc_error)
    }

    pub async fn blockhash_valid(&self, blockhash: &Hash) -> Result<bool, RpcError> {
        self.client
            .is_blockhash_valid(blockhash, self.client.commitment_config())
            .await
            .map_err(rpc_error)
    }

    pub async fn balance(&self, pubkey: &Pubkey) -> Result<u64, RpcError> {
        self.client.get_balance(pubkey).await.map_err(rpc_error)
    }

    pub async fn account(&self, pubkey: &Pubkey) -> Result<Option<RawAccount>, RpcError> {
        self.client
            .get_account_with_commitment(pubkey, self.client.commitment_config())
            .await
            .map(|account| account.map(RawAccount::from))
            .map_err(rpc_error)
    }

    /// At most 100 accounts, in the order asked.
    pub async fn accounts(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<RawAccount>>, RpcError> {
        self.client
            .get_multiple_accounts(pubkeys)
            .await
            .map(|accounts| {
                accounts
                    .into_iter()
                    .map(|account| account.map(RawAccount::from))
                    .collect()
            })
            .map_err(rpc_error)
    }

    /// The balance of at most 100 accounts, in the order asked, without
    /// their data, and the slot the node answered at. `None` for one that
    /// does not exist. A `min_slot` above zero is one the node must have seen.
    pub async fn lamports(
        &self,
        pubkeys: &[Pubkey],
        min_slot: u64,
    ) -> Result<(u64, Vec<Option<u64>>), RpcError> {
        let config = RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            data_slice: Some(UiDataSliceConfig {
                offset: 0,
                length: 0,
            }),
            commitment: Some(self.client.commitment_config()),
            min_context_slot: (min_slot > 0).then_some(min_slot),
        };
        self.client
            .get_multiple_accounts_with_config_and_slot(pubkeys, config)
            .await
            .map(|(slot, accounts)| {
                let lamports = accounts
                    .into_iter()
                    .map(|account| account.map(|account| account.lamports))
                    .collect();
                (slot, lamports)
            })
            .map_err(rpc_error)
    }

    /// The token accounts `owner` holds under `program`, from a node that
    /// has seen `min_slot` when it is above zero.
    pub async fn token_accounts(
        &self,
        owner: &Pubkey,
        program: Pubkey,
        min_slot: u64,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        let config = RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::JsonParsed),
            data_slice: None,
            commitment: Some(self.client.commitment_config()),
            min_context_slot: (min_slot > 0).then_some(min_slot),
        };
        let keyed = self
            .client
            .get_token_accounts_by_owner_with_config(
                owner,
                TokenAccountsFilter::ProgramId(program),
                config,
            )
            .await
            .map_err(rpc_error)?;
        Ok(keyed
            .iter()
            .filter_map(|row| token_account(*owner, &row.pubkey, &row.account.data))
            .collect())
    }

    /// The accounts of `program` that are `data_size` bytes long and hold
    /// `address` at `offset`.
    pub async fn program_accounts(
        &self,
        program: &Pubkey,
        data_size: u64,
        offset: usize,
        address: &Pubkey,
    ) -> Result<Vec<(Pubkey, RawAccount)>, RpcError> {
        accounts_of(&self.client, program, data_size, offset, address).await
    }

    pub async fn send(&self, wire: &[u8]) -> Result<Signature, RpcError> {
        self.client
            .send_raw_transaction(wire)
            .await
            .map_err(rpc_error)
    }

    pub async fn signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<SignatureStatus>>, RpcError> {
        let answered = self
            .client
            .get_signature_statuses(signatures)
            .await
            .map_err(rpc_error)?;
        Ok(answered
            .into_iter()
            .map(|status| {
                status.map(|s| {
                    signature_status(
                        s.slot,
                        s.confirmations,
                        s.err.as_ref().map(|err| err.to_string()),
                        s.confirmation_status.as_ref(),
                    )
                })
            })
            .collect())
    }

    pub async fn signatures_for_address(
        &self,
        address: &Pubkey,
        limit: usize,
    ) -> Result<Vec<Signature>, RpcError> {
        let config =
            solana_client_wasm::utils::rpc_config::GetConfirmedSignaturesForAddress2Config {
                limit: Some(limit),
                ..Default::default()
            };
        self.client
            .get_signatures_for_address_with_config(address, config)
            .await
            .map_err(rpc_error)?
            .into_iter()
            .map(|entry| {
                entry.signature.parse().map_err(|_| {
                    RpcError::Message("The node returned an unreadable signature.".into())
                })
            })
            .collect()
    }

    pub async fn simulate(&self, request: &SimulationRequest) -> Result<SimulationReply, RpcError> {
        let config = RpcSimulateTransactionConfig {
            sig_verify: false,
            replace_recent_blockhash: request.replace_recent_blockhash,
            commitment: Some(self.client.commitment_config()),
            encoding: Some(UiTransactionEncoding::Base64),
            accounts: Some(RpcSimulateTransactionAccountsConfig {
                encoding: Some(UiAccountEncoding::Base64),
                addresses: request.watched.iter().map(Pubkey::to_string).collect(),
            }),
            min_context_slot: None,
            inner_instructions: true,
        };
        let result = self
            .client
            .simulate_raw_transaction(&request.wire, config)
            .await
            .map_err(rpc_error)?;
        Ok(simulation_reply(SimulationParts {
            err: result.err.and_then(|err| serde_json::to_value(err).ok()),
            logs: result.logs,
            units_consumed: result.units_consumed,
            loaded_accounts_data_size: result.loaded_accounts_data_size,
            accounts: result.accounts,
            inner_instructions: result.inner_instructions,
            return_data: result
                .return_data
                .map(|data| (data.program_id, data.data.0)),
        }))
    }

    pub async fn epoch_info(&self) -> Result<EpochInfo, RpcError> {
        self.client
            .get_epoch_info()
            .await
            .map(|info| EpochInfo {
                epoch: info.epoch,
                slot_index: info.slot_index,
                slots_in_epoch: info.slots_in_epoch,
            })
            .map_err(rpc_error)
    }

    pub async fn rent_exemption(&self, data_len: usize) -> Result<u64, RpcError> {
        self.client
            .get_minimum_balance_for_rent_exemption(data_len)
            .await
            .map_err(rpc_error)
    }

    pub async fn stake_minimum_delegation(&self) -> Result<u64, RpcError> {
        self.client
            .get_stake_minimum_delegation()
            .await
            .map_err(rpc_error)
    }

    pub async fn inflation_rewards(
        &self,
        accounts: &[Pubkey],
        epoch: u64,
    ) -> Result<Vec<StakeReward>, RpcError> {
        let rows = self
            .client
            .get_inflation_reward_with_config(accounts, Some(epoch))
            .await
            .map_err(rpc_error)?;
        Ok(accounts
            .iter()
            .zip(rows)
            .filter_map(|(account, row)| {
                let row = row?;
                stake_reward(
                    *account,
                    row.epoch,
                    row.amount,
                    row.post_balance,
                    row.commission,
                )
            })
            .collect())
    }
}

async fn accounts_of(
    client: &WasmClient,
    program: &Pubkey,
    data_size: u64,
    offset: usize,
    address: &Pubkey,
) -> Result<Vec<(Pubkey, RawAccount)>, RpcError> {
    let config = RpcProgramAccountsConfig {
        filters: Some(vec![
            RpcFilterType::DataSize(data_size),
            RpcFilterType::Memcmp(Memcmp {
                offset,
                bytes: MemcmpEncodedBytes::Bytes(address.to_bytes().to_vec()),
                encoding: None,
            }),
        ]),
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            ..RpcAccountInfoConfig::default()
        },
        with_context: Some(false),
    };
    client
        .get_program_accounts_with_config(program, config)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|(pubkey, account)| (pubkey, RawAccount::from(account)))
                .collect()
        })
        .map_err(rpc_error)
}
