//! The desktop backend: Agave's `solana-client`.

use crate::convert::{
    rpc_error, signature_status, simulation_reply, stake_reward, token_account, SimulationParts,
};
use serde_json::json;
use solana_account_decoder_client_types::{UiAccountEncoding, UiDataSliceConfig};
use solana_client::client_error::{ClientError, ClientErrorKind};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{
    RpcAccountInfoConfig, RpcProgramAccountsConfig, RpcSimulateTransactionAccountsConfig,
    RpcSimulateTransactionConfig,
};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_client::rpc_request::RpcRequest;
use solana_client::rpc_response::{Response, RpcKeyedAccount};
use solana_commitment_config::CommitmentConfig;
use solana_hash::Hash;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_status_client_types::UiTransactionEncoding;
use std::time::Duration;
use wallet_core::simulate::{SimulationReply, SimulationRequest};
use wallet_core::stake::{EpochInfo, StakeReward};
use wallet_core::{DiscoveredTokenAccount, RawAccount, RpcError, SignatureStatus};

pub struct Backend {
    client: RpcClient,
}

/// The client's error, with a timeout and a node that could not be reached
/// told apart from an answer the node gave.
fn client_error(err: ClientError) -> RpcError {
    match err.kind() {
        ClientErrorKind::Reqwest(inner) if inner.is_timeout() => RpcError::Timeout,
        ClientErrorKind::Reqwest(inner) if inner.is_connect() => {
            RpcError::Unreachable(inner.to_string())
        }
        ClientErrorKind::Io(inner) => RpcError::Unreachable(inner.to_string()),
        _ => rpc_error(err),
    }
}

/// The client sends and simulates a typed transaction. It writes it back out
/// in the same wire layout it is read from here. Legacy, v0 and v1 all
/// deserialize as [`VersionedTransaction`].
fn typed(wire: &[u8]) -> Result<VersionedTransaction, RpcError> {
    wincode::deserialize(wire)
        .map_err(|err| RpcError::Message(format!("The transaction could not be read: {err}")))
}

impl Backend {
    pub fn new(url: &str, timeout: Duration) -> Self {
        Self {
            client: RpcClient::new_with_timeout_and_commitment(
                url.to_string(),
                timeout,
                CommitmentConfig::confirmed(),
            ),
        }
    }

    pub async fn genesis_hash(&self) -> Result<Hash, RpcError> {
        self.client.get_genesis_hash().await.map_err(client_error)
    }

    pub async fn latest_blockhash(&self) -> Result<Hash, RpcError> {
        self.latest_blockhash_with_height()
            .await
            .map(|(hash, _)| hash)
    }

    pub async fn latest_blockhash_with_height(&self) -> Result<(Hash, u64), RpcError> {
        self.client
            .get_latest_blockhash_with_commitment(self.client.commitment())
            .await
            .map_err(client_error)
    }

    pub async fn block_height(&self) -> Result<u64, RpcError> {
        self.client.get_block_height().await.map_err(client_error)
    }

    pub async fn blockhash_valid(&self, blockhash: &Hash) -> Result<bool, RpcError> {
        self.client
            .is_blockhash_valid(blockhash, self.client.commitment())
            .await
            .map_err(client_error)
    }

    pub async fn balance(&self, pubkey: &Pubkey) -> Result<u64, RpcError> {
        self.client.get_balance(pubkey).await.map_err(client_error)
    }

    pub async fn account(&self, pubkey: &Pubkey) -> Result<Option<RawAccount>, RpcError> {
        self.client
            .get_account_with_commitment(pubkey, self.client.commitment())
            .await
            .map(|answer| answer.value.map(RawAccount::from))
            .map_err(client_error)
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
            .map_err(client_error)
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
            commitment: Some(self.client.commitment()),
            min_context_slot: (min_slot > 0).then_some(min_slot),
        };
        self.client
            .get_multiple_ui_accounts_with_config(pubkeys, config)
            .await
            .map(|answer| {
                let lamports = answer
                    .value
                    .into_iter()
                    .map(|account| account.map(|account| account.lamports))
                    .collect();
                (answer.context.slot, lamports)
            })
            .map_err(client_error)
    }

    /// The token accounts `owner` holds under `program`, from a node that
    /// has seen `min_slot` when it is above zero.
    pub async fn token_accounts(
        &self,
        owner: &Pubkey,
        program: Pubkey,
        min_slot: u64,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        // The client's own call cannot ask for a slot.
        let mut config = json!({
            "encoding": "jsonParsed",
            "commitment": self.client.commitment().commitment,
        });
        if min_slot > 0 {
            config["minContextSlot"] = json!(min_slot);
        }
        let keyed: Response<Vec<RpcKeyedAccount>> = self
            .client
            .send(
                RpcRequest::GetTokenAccountsByOwner,
                json!([
                    owner.to_string(),
                    { "programId": program.to_string() },
                    config
                ]),
            )
            .await
            .map_err(client_error)?;
        Ok(keyed
            .value
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
        let config = RpcProgramAccountsConfig {
            filters: Some(vec![
                RpcFilterType::DataSize(data_size),
                RpcFilterType::Memcmp(Memcmp::new_raw_bytes(offset, address.to_bytes().to_vec())),
            ]),
            account_config: RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                ..RpcAccountInfoConfig::default()
            },
            with_context: Some(false),
            sort_results: Some(false),
        };
        let accounts = self
            .client
            .get_program_ui_accounts_with_config(program, config)
            .await
            .map_err(client_error)?;
        Ok(accounts
            .into_iter()
            .filter_map(|(pubkey, ui)| {
                ui.to_account()
                    .map(|account| (pubkey, RawAccount::from(account)))
            })
            .collect())
    }

    pub async fn send(&self, wire: &[u8]) -> Result<Signature, RpcError> {
        self.client
            .send_transaction(&typed(wire)?)
            .await
            .map_err(client_error)
    }

    pub async fn signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<SignatureStatus>>, RpcError> {
        let answered = self
            .client
            .get_signature_statuses(signatures)
            .await
            .map_err(client_error)?
            .value;
        Ok(answered
            .into_iter()
            .map(|status| {
                status.map(|s| {
                    signature_status(
                        s.slot,
                        s.confirmations.map(|c| c as u64),
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
        let config = solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config {
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
            commitment: Some(self.client.commitment()),
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
            .simulate_transaction_with_config(&typed(&request.wire)?, config)
            .await
            .map_err(client_error)?
            .value;
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
            .map_err(client_error)
    }

    pub async fn rent_exemption(&self, data_len: usize) -> Result<u64, RpcError> {
        self.client
            .get_minimum_balance_for_rent_exemption(data_len)
            .await
            .map_err(client_error)
    }

    pub async fn stake_minimum_delegation(&self) -> Result<u64, RpcError> {
        self.client
            .get_stake_minimum_delegation()
            .await
            .map_err(client_error)
    }

    pub async fn inflation_rewards(
        &self,
        accounts: &[Pubkey],
        epoch: u64,
    ) -> Result<Vec<StakeReward>, RpcError> {
        let rows = self
            .client
            .get_inflation_reward(accounts, Some(epoch))
            .await
            .map_err(client_error)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use solana_message::{Message as LegacyMessage, MessageHeader, VersionedMessage};
    use solana_signature::Signature;
    use wallet_core::{KeypairSigner, WalletSigner, SYSTEM_PROGRAM_ID};

    fn transfer_data() -> Vec<u8> {
        let mut data = 2u32.to_le_bytes().to_vec();
        data.extend_from_slice(&1u64.to_le_bytes());
        data
    }

    #[test]
    fn typed_reads_legacy_and_v0_wires() {
        let payer = KeypairSigner::new_random().pubkey();
        let to = Pubkey::new_from_array([2; 32]);
        let header = MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 1,
        };
        let legacy = LegacyMessage {
            header,
            account_keys: vec![payer, SYSTEM_PROGRAM_ID, to],
            recent_blockhash: Hash::new_from_array([1; 32]),
            instructions: vec![solana_message::compiled_instruction::CompiledInstruction {
                program_id_index: 1,
                accounts: vec![0, 2],
                data: transfer_data(),
            }],
        };
        let legacy_wire = wincode::serialize(&VersionedTransaction {
            signatures: vec![Signature::default()],
            message: VersionedMessage::Legacy(legacy),
        })
        .unwrap();
        assert!(typed(&legacy_wire).is_ok());

        let v0 = solana_message::v0::Message {
            header,
            account_keys: vec![payer, SYSTEM_PROGRAM_ID, to],
            recent_blockhash: Hash::new_from_array([1; 32]),
            instructions: vec![solana_message::compiled_instruction::CompiledInstruction {
                program_id_index: 1,
                accounts: vec![0, 2],
                data: transfer_data(),
            }],
            address_table_lookups: vec![],
        };
        let v0_wire = wincode::serialize(&VersionedTransaction {
            signatures: vec![Signature::default()],
            message: VersionedMessage::V0(v0),
        })
        .unwrap();
        assert!(typed(&v0_wire).is_ok());
    }
}
