//! Test doubles: a node made of answers written out in a test.

use async_trait::async_trait;
use solana_hash::Hash;
use solana_nonce::state::{Data, DurableNonce, State};
use solana_nonce::versions::Versions;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_stake_interface::stake_history::StakeHistory;
use solana_stake_interface::state::{Authorized, Lockup, Meta, StakeStateV2};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use wallet_core::simulate::{SimulationReply, SimulationRequest};
use wallet_core::stake::{EpochInfo, StakeReward};
use wallet_core::{
    compile_v1, decode_v1_transaction, DecodedTx, DiscoveredTokenAccount, RawAccount, RpcError,
    SignatureStatus, SolanaRpc, STAKE_PROGRAM_ID, SYSTEM_PROGRAM_ID,
};

pub fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

/// Answers from what it was given. Anything it was not given is absent, not
/// an error, unless a `*_fail` switch says otherwise.
#[derive(Default)]
pub struct FakeRpc {
    /// Every request fails with [`RpcError::Offline`].
    pub offline: bool,
    /// Token account requests are refused.
    pub tokens_fail: bool,
    pub balances: HashMap<Pubkey, u64>,
    /// Token accounts by owner.
    pub tokens: HashMap<Pubkey, Vec<DiscoveredTokenAccount>>,
    pub accounts: HashMap<Pubkey, RawAccount>,
    /// By the address searched for and the offset it is searched at.
    pub program_accounts: HashMap<(Pubkey, usize), Vec<(Pubkey, RawAccount)>>,
    /// Addresses a program account search times out for.
    pub program_accounts_fail: Vec<Pubkey>,
    pub simulation: Option<SimulationReply>,
    /// Addresses that were used on chain. Every other one has no history.
    pub used: Vec<Pubkey>,
    pub rewards: Vec<StakeReward>,
    /// Statuses returned by [`SolanaRpc::get_signature_statuses`].
    pub signature_statuses: Mutex<HashMap<Signature, Option<SignatureStatus>>>,
    /// Block height returned by [`SolanaRpc::get_block_height`].
    pub block_height: AtomicU64,
    /// The newest slot the node has seen. A read that asks for a later one
    /// is answered [`RpcError::Behind`]. Zero answers at the slot asked for.
    pub slot: AtomicU64,
    /// The slot each read that can ask for one asked for.
    pub since: Mutex<Vec<u64>>,
    pub asked: AtomicUsize,
}

impl FakeRpc {
    pub fn with_balance(mut self, pubkey: Pubkey, lamports: u64) -> Self {
        self.balances.insert(pubkey, lamports);
        self
    }

    pub fn with_tokens(mut self, owner: Pubkey, tokens: Vec<DiscoveredTokenAccount>) -> Self {
        self.tokens.insert(owner, tokens);
        self
    }

    pub fn with_account(mut self, pubkey: Pubkey, account: RawAccount) -> Self {
        self.accounts.insert(pubkey, account);
        self
    }

    pub fn with_program_accounts(
        mut self,
        address: Pubkey,
        offset: usize,
        rows: Vec<(Pubkey, RawAccount)>,
    ) -> Self {
        self.program_accounts.insert((address, offset), rows);
        self
    }

    pub fn failing_program_accounts(mut self, address: Pubkey) -> Self {
        self.program_accounts_fail.push(address);
        self
    }

    pub fn with_signature_status(self, signature: Signature, status: SignatureStatus) -> Self {
        self.set_signature_status(signature, Some(status));
        self
    }

    pub fn with_block_height(self, height: u64) -> Self {
        self.block_height.store(height, Ordering::SeqCst);
        self
    }

    /// Update a signature's status while a tracker is polling.
    pub fn set_signature_status(&self, signature: Signature, status: Option<SignatureStatus>) {
        self.signature_statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(signature, status);
    }

    pub fn set_block_height(&self, height: u64) {
        self.block_height.store(height, Ordering::SeqCst);
    }

    /// How many requests were made.
    pub fn asked(&self) -> usize {
        self.asked.load(Ordering::Relaxed)
    }

    fn count(&self) -> Result<(), RpcError> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        if self.offline {
            return Err(RpcError::Offline);
        }
        Ok(())
    }

    /// The slot a read that asked for `min_slot` is answered at.
    fn answer_at(&self, min_slot: u64) -> Result<u64, RpcError> {
        self.since
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(min_slot);
        match self.slot.load(Ordering::Relaxed) {
            0 => Ok(min_slot),
            seen if min_slot > seen => Err(RpcError::Behind),
            seen => Ok(seen),
        }
    }

    fn lamports_of(&self, pubkey: &Pubkey) -> Option<u64> {
        self.balances
            .get(pubkey)
            .copied()
            .or_else(|| self.accounts.get(pubkey).map(|account| account.lamports))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl SolanaRpc for FakeRpc {
    async fn get_balance(&self, pubkey: &Pubkey) -> Result<u64, RpcError> {
        self.count()?;
        Ok(self.lamports_of(pubkey).unwrap_or(0))
    }

    async fn get_account(&self, pubkey: &Pubkey) -> Result<Option<RawAccount>, RpcError> {
        self.count()?;
        Ok(self.accounts.get(pubkey).cloned())
    }

    /// One request, as a node answers for many accounts at once. Asking for
    /// none asks nothing.
    async fn get_accounts(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<RawAccount>>, RpcError> {
        if pubkeys.is_empty() {
            return Ok(Vec::new());
        }
        self.count()?;
        Ok(pubkeys
            .iter()
            .map(|pubkey| self.accounts.get(pubkey).cloned())
            .collect())
    }

    async fn get_lamports(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<u64>>, RpcError> {
        if pubkeys.is_empty() {
            return Ok(Vec::new());
        }
        self.count()?;
        Ok(pubkeys.iter().map(|key| self.lamports_of(key)).collect())
    }

    async fn get_lamports_since(
        &self,
        pubkeys: &[Pubkey],
        min_slot: u64,
    ) -> Result<(u64, Vec<Option<u64>>), RpcError> {
        if pubkeys.is_empty() {
            return Ok((min_slot, Vec::new()));
        }
        self.count()?;
        let slot = self.answer_at(min_slot)?;
        Ok((
            slot,
            pubkeys.iter().map(|key| self.lamports_of(key)).collect(),
        ))
    }

    async fn get_token_accounts_by_owner(
        &self,
        owner: &Pubkey,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        self.count()?;
        if self.tokens_fail {
            return Err(RpcError::Message("token accounts are not served".into()));
        }
        Ok(self.tokens.get(owner).cloned().unwrap_or_default())
    }

    async fn get_token_accounts_by_owner_since(
        &self,
        owner: &Pubkey,
        min_slot: u64,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        self.count()?;
        self.answer_at(min_slot)?;
        if self.tokens_fail {
            return Err(RpcError::Message("token accounts are not served".into()));
        }
        Ok(self.tokens.get(owner).cloned().unwrap_or_default())
    }

    async fn get_program_accounts(
        &self,
        _program: &Pubkey,
        _data_size: u64,
        offset: usize,
        address: &Pubkey,
    ) -> Result<Vec<(Pubkey, RawAccount)>, RpcError> {
        self.count()?;
        if self.program_accounts_fail.contains(address) {
            return Err(RpcError::Timeout);
        }
        Ok(self
            .program_accounts
            .get(&(*address, offset))
            .cloned()
            .unwrap_or_default())
    }

    async fn get_latest_blockhash(&self) -> Result<Hash, RpcError> {
        self.get_latest_blockhash_with_height()
            .await
            .map(|(hash, _)| hash)
    }

    async fn get_latest_blockhash_with_height(&self) -> Result<(Hash, u64), RpcError> {
        self.count()?;
        Ok((Hash::new_from_array([9; 32]), 150))
    }

    async fn get_block_height(&self) -> Result<u64, RpcError> {
        self.count()?;
        Ok(self.block_height.load(Ordering::SeqCst))
    }

    async fn is_blockhash_valid(&self, _blockhash: &Hash) -> Result<bool, RpcError> {
        self.count()?;
        Ok(true)
    }

    async fn send_transaction(&self, _wire: &[u8]) -> Result<Signature, RpcError> {
        self.count()?;
        Ok(Signature::default())
    }

    async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<SignatureStatus>>, RpcError> {
        self.count()?;
        let statuses = self
            .signature_statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(signatures
            .iter()
            .map(|sig| statuses.get(sig).cloned().unwrap_or(None))
            .collect())
    }

    async fn get_signatures_for_address(
        &self,
        address: &Pubkey,
        limit: usize,
    ) -> Result<Vec<Signature>, RpcError> {
        self.count()?;
        Ok(if self.used.contains(address) {
            vec![Signature::default(); limit.min(1)]
        } else {
            Vec::new()
        })
    }

    async fn simulate(&self, _request: &SimulationRequest) -> Result<SimulationReply, RpcError> {
        self.count()?;
        self.simulation
            .clone()
            .ok_or_else(|| RpcError::Message("no simulation was set up".into()))
    }

    async fn get_epoch_info(&self) -> Result<EpochInfo, RpcError> {
        self.count()?;
        Ok(EpochInfo {
            epoch: 20,
            slot_index: 108_000,
            slots_in_epoch: 432_000,
        })
    }

    async fn get_rent_exemption(&self, _data_len: usize) -> Result<u64, RpcError> {
        self.count()?;
        Ok(2_282_880)
    }

    async fn get_stake_minimum_delegation(&self) -> Result<u64, RpcError> {
        self.count()?;
        Ok(1_000_000_000)
    }

    async fn get_stake_rewards(
        &self,
        _accounts: &[Pubkey],
        _epoch: u64,
    ) -> Result<Vec<StakeReward>, RpcError> {
        self.count()?;
        Ok(self.rewards.clone())
    }
}

/// An initialized nonce account of `authority`, and the value it holds.
pub fn nonce_account(authority: Pubkey) -> (RawAccount, Hash) {
    let durable = DurableNonce::default();
    let versions = Versions::new(State::Initialized(Data::new(authority, durable, 5000)));
    let account = RawAccount {
        lamports: 1,
        data: wincode::serialize(&versions).unwrap(),
        owner: SYSTEM_PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    };
    (account, *durable.as_hash())
}

/// An initialized, undelegated stake account holding `sol` SOL.
pub fn stake_account(staker: Pubkey, withdrawer: Pubkey, sol: u64) -> RawAccount {
    let meta = Meta {
        authorized: Authorized { staker, withdrawer },
        lockup: Lockup::default(),
        ..Meta::default()
    };
    let mut data = wincode::serialize(&StakeStateV2::Initialized(meta)).unwrap();
    data.resize(StakeStateV2::size_of(), 0);
    RawAccount {
        lamports: sol * 1_000_000_000,
        data,
        owner: STAKE_PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    }
}

/// An initialized mint of `program` with `decimals`, and no extensions.
pub fn mint_account(program: Pubkey, decimals: u8) -> RawAccount {
    let mut data = vec![0u8; 82];
    data[44] = decimals;
    data[45] = 1;
    RawAccount {
        lamports: 1,
        data,
        owner: program,
        executable: false,
        rent_epoch: 0,
    }
}

/// An initialized token account of `program` for `mint`, which `owner` may
/// move, holding `amount`.
pub fn token_account(program: Pubkey, mint: Pubkey, owner: Pubkey, amount: u64) -> RawAccount {
    let mut data = vec![0u8; 165];
    data[0..32].copy_from_slice(mint.as_ref());
    data[32..64].copy_from_slice(owner.as_ref());
    data[64..72].copy_from_slice(&amount.to_le_bytes());
    data[108] = 1;
    RawAccount {
        lamports: 1,
        data,
        owner: program,
        executable: false,
        rent_epoch: 0,
    }
}

/// The stake history sysvar, with nothing recorded.
pub fn stake_history_account() -> RawAccount {
    RawAccount {
        lamports: 1,
        data: wincode::serialize(&StakeHistory::default()).unwrap(),
        owner: pk(0),
        executable: false,
        rent_epoch: 0,
    }
}

/// The System program's transfer of 5 lamports from `from` to `to`.
pub fn transfer_instruction(from: Pubkey, to: Pubkey) -> solana_instruction::Instruction {
    solana_instruction::Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![
            solana_instruction::AccountMeta::new(from, true),
            solana_instruction::AccountMeta::new(to, false),
        ],
        data: vec![2, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0],
    }
}

/// A transfer of 5 lamports from `from`, which pays, to `to`.
pub fn transfer(from: Pubkey, to: Pubkey) -> DecodedTx {
    let wire = compile_v1(
        from,
        vec![transfer_instruction(from, to)],
        None,
        Some(Hash::default()),
    )
    .unwrap()
    .wire;
    decode_v1_transaction(&wire).unwrap()
}

/// A legacy transfer of 5 lamports from `from`, which pays, to `to`, laid
/// out by hand with its one signature slot empty.
pub fn legacy_transfer(from: Pubkey, to: Pubkey) -> Vec<u8> {
    let mut wire = vec![1u8];
    wire.extend_from_slice(&[0u8; 64]);
    wire.extend_from_slice(&[1, 0, 1, 3]);
    for key in [from, to, SYSTEM_PROGRAM_ID] {
        wire.extend_from_slice(key.as_ref());
    }
    wire.extend_from_slice(Hash::default().as_ref());
    wire.extend_from_slice(&[1, 2, 2, 0, 1, 12]);
    wire.extend_from_slice(&2u32.to_le_bytes());
    wire.extend_from_slice(&5u64.to_le_bytes());
    wire
}
