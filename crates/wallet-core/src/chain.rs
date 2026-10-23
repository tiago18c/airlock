//! What the app asks a node, and the shapes its answers come back in.
//!
//! Nothing here opens a socket. [`SolanaRpc`] is the port: `wallet-rpc`
//! implements it over a real client, and tests implement it over made-up
//! answers.

use crate::error::RpcError;
use crate::ids::SYSTEM_PROGRAM_ID;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solana_hash::Hash;
use solana_nonce::{
    state::{Data, State},
    versions::Versions,
};
use solana_pubkey::Pubkey;
use solana_signature::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawAccount {
    pub lamports: u64,
    pub data: Vec<u8>,
    pub owner: Pubkey,
    pub executable: bool,
    pub rent_epoch: u64,
}

impl From<solana_account::Account> for RawAccount {
    fn from(account: solana_account::Account) -> Self {
        Self {
            lamports: account.lamports,
            data: account.data,
            owner: account.owner,
            executable: account.executable,
            rent_epoch: account.rent_epoch,
        }
    }
}

/// A token account a node listed for an owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredTokenAccount {
    pub pubkey: Pubkey,
    pub owner: Pubkey,
    pub mint: Pubkey,
    pub amount: u64,
    pub decimals: u8,
}

/// An initialized nonce account and the value it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredNonce {
    pub pubkey: Pubkey,
    pub authority: Pubkey,
    pub hash: Hash,
}

/// What a read of the chain found: balances, token accounts, and nonces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChainSnapshot {
    pub balances: Vec<(Pubkey, u64)>,
    pub tokens: Vec<DiscoveredTokenAccount>,
    /// Owners whose token accounts were fetched. An owner whose request failed
    /// is absent, so its stored holdings and flags are left alone.
    pub token_owners: Vec<Pubkey>,
    pub nonces: Vec<DiscoveredNonce>,
}

/// Where the cluster is in its current epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EpochInfo {
    pub epoch: u64,
    pub slot_index: u64,
    pub slots_in_epoch: u64,
}

impl EpochInfo {
    /// How far the epoch is, in percent.
    pub fn percent(&self) -> u64 {
        if self.slots_in_epoch == 0 {
            return 0;
        }
        (self.slot_index.saturating_mul(100) / self.slots_in_epoch).min(100)
    }
}

/// What one stake account earned in one epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StakeReward {
    pub account: Pubkey,
    pub epoch: u64,
    pub amount: u64,
    /// The account's balance once the reward was paid.
    pub post_balance: u64,
    /// The validator's commission when the reward was paid, in percent.
    pub commission: Option<u8>,
}

/// What the node is asked when `tx` is simulated.
///
/// Signatures are not checked, so a transaction can be simulated before
/// anyone has signed. A transaction on a recent blockhash is simulated on the
/// node's newest one, so an old blockhash does not hide what it would do. One
/// on a durable nonce keeps its own, which is what the node checks it against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulationRequest {
    pub wire: Vec<u8>,
    pub replace_recent_blockhash: bool,
    /// The accounts whose balance afterwards is asked for.
    pub watched: Vec<Pubkey>,
}

/// A node's answer to a simulation, as an RPC client hands it over.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SimulationReply {
    /// The runtime's `TransactionError`, in the JSON the node writes it in.
    /// `None` when the transaction would succeed.
    pub err: Option<Value>,
    pub logs: Vec<String>,
    pub units_consumed: Option<u64>,
    pub loaded_accounts_data_size: Option<u32>,
    /// Lamports of each watched account afterwards, in the order asked. An
    /// account that does not exist holds nothing.
    pub balances: Vec<u64>,
    pub inner: Vec<InnerReply>,
    pub return_data: Option<(Pubkey, Vec<u8>)>,
}

/// The calls one instruction made, by account index into the transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InnerReply {
    pub instruction: usize,
    pub calls: Vec<CompiledCall>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledCall {
    pub program_id_index: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
    /// How deep the call is. A node that does not say means 2.
    pub stack_height: Option<u32>,
}

/// What a transaction's lifetime is taken from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lifetime {
    Blockhash(Hash),
    Nonce(DiscoveredNonce),
}

/// How far a transaction has been confirmed, from `getSignatureStatuses`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmationLevel {
    Processed,
    Confirmed,
    Finalized,
}

impl ConfirmationLevel {
    pub fn at_least_confirmed(self) -> bool {
        matches!(self, Self::Confirmed | Self::Finalized)
    }
}

/// One signature's status from the node. `err` is the on-chain failure as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureStatus {
    pub slot: u64,
    pub err: Option<String>,
    pub confirmation: Option<ConfirmationLevel>,
}

/// Chain access, one request per method. What is done with the answers is in
/// the `wallet-flows` crate.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait SolanaRpc: Send + Sync {
    async fn get_balance(&self, pubkey: &Pubkey) -> Result<u64, RpcError>;
    async fn get_account(&self, pubkey: &Pubkey) -> Result<Option<RawAccount>, RpcError>;
    /// Several accounts, in the order asked, with `None` for one that does
    /// not exist. Asks one at a time unless the implementation batches.
    async fn get_accounts(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<RawAccount>>, RpcError> {
        let mut out = Vec::with_capacity(pubkeys.len());
        for pubkey in pubkeys {
            out.push(self.get_account(pubkey).await?);
        }
        Ok(out)
    }
    /// The balance of each account, in the order asked, with `None` for one
    /// that does not exist. An implementation that can leave the data out does.
    async fn get_lamports(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<u64>>, RpcError> {
        Ok(self
            .get_accounts(pubkeys)
            .await?
            .into_iter()
            .map(|account| account.map(|account| account.lamports))
            .collect())
    }
    /// [`get_lamports`](Self::get_lamports) from a node that has seen at
    /// least `min_slot`, with the slot it answered at. A node behind it
    /// answers [`RpcError::Behind`]. Zero asks any node. An implementation
    /// that cannot tell answers at `min_slot`.
    async fn get_lamports_since(
        &self,
        pubkeys: &[Pubkey],
        min_slot: u64,
    ) -> Result<(u64, Vec<Option<u64>>), RpcError> {
        Ok((min_slot, self.get_lamports(pubkeys).await?))
    }
    async fn get_token_accounts_by_owner(
        &self,
        owner: &Pubkey,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError>;
    /// [`get_token_accounts_by_owner`](Self::get_token_accounts_by_owner)
    /// from a node that has seen at least `min_slot`, as in
    /// [`get_lamports_since`](Self::get_lamports_since).
    async fn get_token_accounts_by_owner_since(
        &self,
        owner: &Pubkey,
        _min_slot: u64,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        self.get_token_accounts_by_owner(owner).await
    }
    /// The accounts of `program` that are `data_size` bytes long and hold
    /// `address` at `offset`.
    async fn get_program_accounts(
        &self,
        program: &Pubkey,
        data_size: u64,
        offset: usize,
        address: &Pubkey,
    ) -> Result<Vec<(Pubkey, RawAccount)>, RpcError>;
    async fn get_latest_blockhash(&self) -> Result<Hash, RpcError>;
    /// Latest blockhash and the height at which it stops being valid.
    async fn get_latest_blockhash_with_height(&self) -> Result<(Hash, u64), RpcError>;
    async fn get_block_height(&self) -> Result<u64, RpcError>;
    async fn is_blockhash_valid(&self, blockhash: &Hash) -> Result<bool, RpcError>;
    async fn send_transaction(&self, wire: &[u8]) -> Result<Signature, RpcError>;
    /// Status of each signature, in the same order. `None` means the node has
    /// not seen it yet. Answers at the confirmed commitment when the
    /// implementation can choose.
    async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<SignatureStatus>>, RpcError>;
    /// Up to `limit` of the latest transactions that named `address`, newest
    /// first. Empty for an address the chain has never seen.
    async fn get_signatures_for_address(
        &self,
        _address: &Pubkey,
        _limit: usize,
    ) -> Result<Vec<Signature>, RpcError> {
        Err(RpcError::Message(
            "This node cannot list the transactions of an address.".into(),
        ))
    }
    /// What `request` would do if it were sent now. Nothing is sent.
    async fn simulate(&self, request: &SimulationRequest) -> Result<SimulationReply, RpcError>;
    async fn get_epoch_info(&self) -> Result<EpochInfo, RpcError>;
    /// Lamports an account of `data_len` bytes has to keep to exist.
    async fn get_rent_exemption(&self, data_len: usize) -> Result<u64, RpcError>;
    /// The least a stake delegation may be, in lamports.
    async fn get_stake_minimum_delegation(&self) -> Result<u64, RpcError>;
    /// What each of `accounts` earned in `epoch`. An account that earned
    /// nothing has no entry.
    async fn get_stake_rewards(
        &self,
        accounts: &[Pubkey],
        epoch: u64,
    ) -> Result<Vec<StakeReward>, RpcError>;
}

pub fn parse_nonce_account(pubkey: Pubkey, account: &RawAccount) -> Option<DiscoveredNonce> {
    if account.owner != SYSTEM_PROGRAM_ID || account.data.len() < State::size() {
        return None;
    }
    let versions: Versions = wincode::deserialize(&account.data).ok()?;
    match State::from(versions) {
        State::Initialized(Data {
            authority,
            durable_nonce,
            ..
        }) => Some(DiscoveredNonce {
            pubkey,
            authority,
            hash: *durable_nonce.as_hash(),
        }),
        State::Uninitialized => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_nonce::state::DurableNonce;

    #[test]
    fn parse_initialized_nonce_and_reject_other() {
        let authority = Pubkey::new_from_array([7; 32]);
        let durable = DurableNonce::default();
        let versions = Versions::new(State::Initialized(Data::new(authority, durable, 5000)));
        let data = wincode::serialize(&versions).unwrap();
        let auth_off = data
            .windows(32)
            .position(|w| w == authority.as_ref())
            .unwrap();
        assert_eq!(auth_off, 8, "GPA memcmp offset for nonce authority");
        assert_eq!(data.len(), State::size());

        let account = RawAccount {
            lamports: 1,
            data,
            owner: SYSTEM_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        };
        let nonce = parse_nonce_account(Pubkey::new_from_array([3; 32]), &account).unwrap();
        assert_eq!(nonce.authority, authority);
        assert_eq!(nonce.hash, *durable.as_hash());

        let wrong_owner = RawAccount {
            owner: Pubkey::new_from_array([1; 32]),
            ..account.clone()
        };
        assert!(parse_nonce_account(Pubkey::new_from_array([3; 32]), &wrong_owner).is_none());
    }
}
