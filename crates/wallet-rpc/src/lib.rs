//! Everything the app asks a Solana node.
//!
//! [`Rpc`] is one client for one node. Underneath it is Agave's
//! `solana-client` on desktop and the vendored `solana-client-wasm` in a
//! browser. Each backend calls its client's own methods and answers in the
//! types of `wallet-core`, so nothing above this crate knows which one is in
//! use. This crate only asks and translates: what the app does with the
//! answers is in `wallet-flows`.

#[cfg(not(target_arch = "wasm32"))]
#[path = "activity_native.rs"]
mod activity;
#[cfg(target_arch = "wasm32")]
#[path = "activity_web.rs"]
mod activity;
#[cfg(not(target_arch = "wasm32"))]
#[path = "native.rs"]
mod backend;
#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod backend;
mod convert;

pub use activity::{subscribe_activity, ActivityStream};

use backend::Backend;
use solana_hash::Hash;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use std::sync::Arc;
use std::time::Duration;
use wallet_core::simulate::{SimulationReply, SimulationRequest};
use wallet_core::stake::{EpochInfo, StakeReward};
use wallet_core::{
    DiscoveredTokenAccount, RawAccount, RpcError, SignatureStatus, SolanaRpc,
    TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};

/// How long the probe that decides online or offline may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// How long any other request may take.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// A node answers for at most this many accounts in one request.
const ACCOUNTS_PER_REQUEST: usize = 100;

/// What the node's WebSocket told about a watched address, at the confirmed
/// commitment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    /// A transaction mentions the address.
    Transaction {
        address: Pubkey,
        signature: Signature,
        /// The transaction landed but failed.
        failed: bool,
        slot: u64,
    },
    /// The address's balance changed. An account that is gone holds nothing.
    Balance {
        address: Pubkey,
        lamports: u64,
        slot: u64,
    },
}

impl Activity {
    pub fn address(&self) -> Pubkey {
        match self {
            Self::Transaction { address, .. } | Self::Balance { address, .. } => *address,
        }
    }

    /// The slot the node told this at.
    pub fn slot(&self) -> u64 {
        match self {
            Self::Transaction { slot, .. } | Self::Balance { slot, .. } => *slot,
        }
    }
}

/// A client for the node at one URL. Cloning it shares the connection.
#[derive(Clone)]
pub struct Rpc {
    url: Arc<str>,
    backend: Arc<Backend>,
}

impl Rpc {
    pub fn connect(url: &str) -> Self {
        Self {
            url: url.into(),
            backend: Arc::new(Backend::new(url, RPC_TIMEOUT)),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// `getGenesisHash` against `url`, with a short timeout. An answer means
    /// the node is reachable. A timeout or any other failure means it is not.
    pub async fn probe(url: &str) -> Result<Hash, RpcError> {
        Backend::new(url, PROBE_TIMEOUT).genesis_hash().await
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SolanaRpc for Rpc {
    async fn get_balance(&self, pubkey: &Pubkey) -> Result<u64, RpcError> {
        self.backend.balance(pubkey).await
    }

    async fn get_account(&self, pubkey: &Pubkey) -> Result<Option<RawAccount>, RpcError> {
        self.backend.account(pubkey).await
    }

    async fn get_accounts(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<RawAccount>>, RpcError> {
        let mut out = Vec::with_capacity(pubkeys.len());
        for batch in pubkeys.chunks(ACCOUNTS_PER_REQUEST) {
            out.extend(self.backend.accounts(batch).await?);
        }
        Ok(out)
    }

    async fn get_lamports(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<u64>>, RpcError> {
        Ok(self.get_lamports_since(pubkeys, 0).await?.1)
    }

    async fn get_lamports_since(
        &self,
        pubkeys: &[Pubkey],
        min_slot: u64,
    ) -> Result<(u64, Vec<Option<u64>>), RpcError> {
        let mut out = Vec::with_capacity(pubkeys.len());
        let mut slot = u64::MAX;
        for batch in pubkeys.chunks(ACCOUNTS_PER_REQUEST) {
            let (answered, lamports) = self.backend.lamports(batch, min_slot).await?;
            slot = slot.min(answered);
            out.extend(lamports);
        }
        Ok((if out.is_empty() { min_slot } else { slot }, out))
    }

    async fn get_token_accounts_by_owner(
        &self,
        owner: &Pubkey,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        self.get_token_accounts_by_owner_since(owner, 0).await
    }

    async fn get_token_accounts_by_owner_since(
        &self,
        owner: &Pubkey,
        min_slot: u64,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        let mut out = Vec::new();
        for program in [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID] {
            out.extend(
                self.backend
                    .token_accounts(owner, program, min_slot)
                    .await?,
            );
        }
        Ok(out)
    }

    async fn get_program_accounts(
        &self,
        program: &Pubkey,
        data_size: u64,
        offset: usize,
        address: &Pubkey,
    ) -> Result<Vec<(Pubkey, RawAccount)>, RpcError> {
        self.backend
            .program_accounts(program, data_size, offset, address)
            .await
    }

    async fn get_latest_blockhash(&self) -> Result<Hash, RpcError> {
        self.backend.latest_blockhash().await
    }

    async fn get_latest_blockhash_with_height(&self) -> Result<(Hash, u64), RpcError> {
        self.backend.latest_blockhash_with_height().await
    }

    async fn get_block_height(&self) -> Result<u64, RpcError> {
        self.backend.block_height().await
    }

    async fn is_blockhash_valid(&self, blockhash: &Hash) -> Result<bool, RpcError> {
        self.backend.blockhash_valid(blockhash).await
    }

    async fn send_transaction(&self, wire: &[u8]) -> Result<Signature, RpcError> {
        self.backend.send(wire).await
    }

    async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<SignatureStatus>>, RpcError> {
        self.backend.signature_statuses(signatures).await
    }

    async fn get_signatures_for_address(
        &self,
        address: &Pubkey,
        limit: usize,
    ) -> Result<Vec<Signature>, RpcError> {
        self.backend.signatures_for_address(address, limit).await
    }

    async fn simulate(&self, request: &SimulationRequest) -> Result<SimulationReply, RpcError> {
        self.backend.simulate(request).await
    }

    async fn get_epoch_info(&self) -> Result<EpochInfo, RpcError> {
        self.backend.epoch_info().await
    }

    async fn get_rent_exemption(&self, data_len: usize) -> Result<u64, RpcError> {
        self.backend.rent_exemption(data_len).await
    }

    async fn get_stake_minimum_delegation(&self) -> Result<u64, RpcError> {
        self.backend.stake_minimum_delegation().await
    }

    async fn get_stake_rewards(
        &self,
        accounts: &[Pubkey],
        epoch: u64,
    ) -> Result<Vec<StakeReward>, RpcError> {
        self.backend.inflation_rewards(accounts, epoch).await
    }
}
