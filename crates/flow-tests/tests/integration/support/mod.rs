//! A local chain with the real programs loaded, driven only through what
//! `wallet-core` builds.

use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
use solana_account::Account;
use solana_pubkey::Pubkey;
use solana_transaction::versioned::VersionedTransaction;
use std::path::PathBuf;
use wallet_core::programmatic::{
    ExecutorNonce, ED25519_SIGNER_PROGRAM_ID, EXECUTOR_NONCE_LEN, EXECUTOR_NONCE_PROGRAM_ID,
    MESSAGE_EXECUTOR_PROGRAM_ID,
};
use wallet_core::squads::SquadsProgram;
use wallet_core::{
    apply_local_signatures, compile_v1_budget, ComputeBudgetSpec, Instruction, KeypairSigner,
    WalletSigner, MAX_TRANSACTION_SIZE,
};

pub mod node;
pub mod vector;

/// Sign as a user on the Sign screen does: one press, then, when that press
/// signed an authorization and a key here signs the transaction, the
/// screen's simulation of the authorized transaction (which fits its limits
/// when `node` is there) and a second press. The counts add up both presses.
pub fn sign_through(
    node: Option<&dyn wallet_core::SolanaRpc>,
    wire: &[u8],
    signers: &[&dyn WalletSigner],
    vector: &mut wallet_flows::sign::VectorSigning<'_>,
) -> Result<wallet_flows::sign::Signed, wallet_core::SignError> {
    let first = pollster::block_on(wallet_flows::sign::sign_with(node, wire, signers, vector))?;
    if !first.transaction_next {
        return Ok(first);
    }
    let fitted = match node {
        Some(node) => pollster::block_on(wallet_flows::adapt::simulate_fitting(node, &first.wire))
            .map(|(fitted, _)| fitted)
            .unwrap_or_else(|_| first.wire.clone()),
        None => first.wire.clone(),
    };
    // Every authorization is in: the second press signs the transaction only.
    let mut again = wallet_flows::sign::VectorSigning {
        keys: &[],
        known: vector.known,
        cache: None,
        known_is_current: false,
    };
    let second = pollster::block_on(wallet_flows::sign::sign_with(
        node, &fitted, signers, &mut again,
    ))?;
    Ok(wallet_flows::sign::Signed {
        authorizations: first.authorizations,
        dropped: first.dropped + second.dropped,
        ..second
    })
}

/// What the other device reads from the code of `signed`: the object, sent
/// as one payload and decoded there.
pub fn carried(signed: &wallet_core::protocol::Signed) -> wallet_core::protocol::QrObject {
    use wallet_core::protocol::{decode_object, encode_object, QrObject};
    decode_object(&encode_object(&QrObject::Signature(signed.clone())).unwrap()).unwrap()
}

pub const SOL: u64 = 1_000_000_000;

const ADDRESS_LOOKUP_TABLE_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("AddressLookupTab1e1111111111111111111111111");

/// The smart account program numbers accounts from a counter. Start it
/// somewhere other than zero so a wrong number cannot pass by accident.
pub const SMART_ACCOUNTS_ALREADY_CREATED: u128 = 41;

/// What one landed transaction cost.
#[derive(Clone, Copy, Debug)]
pub struct Landed {
    /// Bytes on the wire, out of [`MAX_TRANSACTION_SIZE`].
    pub size: usize,
    pub compute_units: u64,
}

pub struct Chain {
    svm: LiteSVM,
    pub treasury: Pubkey,
}

impl Chain {
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        for (program_id, file) in [
            (ED25519_SIGNER_PROGRAM_ID, "ed25519_signer.so"),
            (MESSAGE_EXECUTOR_PROGRAM_ID, "message_executor.so"),
            (EXECUTOR_NONCE_PROGRAM_ID, "nonce.so"),
            (SquadsProgram::V4.program_id(), "squads_multisig.so"),
            (
                SquadsProgram::SmartAccount.program_id(),
                "squads_smart_account.so",
            ),
            (wallet_core::vector::PROGRAM_ID, "vector.so"),
        ] {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("programs")
                .join(file);
            svm.add_program_from_file(program_id, &path)
                .unwrap_or_else(|err| {
                    panic!(
                        "could not load {}: {err}\nRun `node scripts/fetch-programs.mjs` first.",
                        path.display()
                    )
                });
        }

        let treasury = KeypairSigner::new_random().pubkey();
        let mut chain = Self { svm, treasury };
        chain.fund(&treasury, SOL);
        for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
            chain.set_program_config(program);
        }
        chain
    }

    /// Both Squads programs read a config account that only their deployer can
    /// create. Write it directly: no creation fee, our treasury.
    fn set_program_config(&mut self, program: SquadsProgram) {
        // Anchor's account tag for `ProgramConfig`, as found on devnet.
        let mut data = vec![0xc4, 0xd2, 0x5a, 0xe7, 0x90, 0x95, 0x8c, 0x3f];
        if program == SquadsProgram::SmartAccount {
            data.extend_from_slice(&SMART_ACCOUNTS_ALREADY_CREATED.to_le_bytes());
        }
        data.extend_from_slice(&[0u8; 32]);
        data.extend_from_slice(&0u64.to_le_bytes());
        data.extend_from_slice(self.treasury.as_ref());
        data.extend_from_slice(&[0u8; 64]);
        assert_eq!(program.treasury(&data), Some(self.treasury));

        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                program.program_config(),
                Account {
                    lamports,
                    data,
                    owner: program.program_id(),
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    /// Change an existing account's data in place, keeping its lamports.
    pub fn patch_data(&mut self, address: &Pubkey, patch: impl FnOnce(&mut Vec<u8>)) {
        let mut account = self.svm.get_account(address).expect("account");
        patch(&mut account.data);
        self.svm.set_account(*address, account).unwrap();
    }

    /// Put an account holding `data` at `address`, as if `owner` had written it.
    pub fn plant(&mut self, address: Pubkey, owner: Pubkey, data: Vec<u8>) {
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                address,
                Account {
                    lamports,
                    data,
                    owner,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    /// Put an active address lookup table holding `addresses` on the chain,
    /// as the lookup table program would have written it.
    pub fn plant_lookup_table(&mut self, addresses: &[Pubkey]) -> Pubkey {
        let table = KeypairSigner::new_random().pubkey();
        // ProgramState::LookupTable, then LookupTableMeta.
        let mut data = 1u32.to_le_bytes().to_vec();
        data.extend_from_slice(&u64::MAX.to_le_bytes()); // never deactivated
        data.extend_from_slice(&0u64.to_le_bytes()); // last extended slot
                                                     // Every address counts as extended before that slot, so all are usable now.
        data.push(u8::try_from(addresses.len()).expect("at most 256 addresses"));
        data.push(1);
        data.extend_from_slice(self.treasury.as_ref()); // authority
        data.extend_from_slice(&[0u8; 2]);
        assert_eq!(data.len(), wallet_core::LOOKUP_TABLE_META_SIZE);
        for address in addresses {
            data.extend_from_slice(address.as_ref());
        }
        self.plant(table, ADDRESS_LOOKUP_TABLE_PROGRAM_ID, data);
        table
    }

    pub fn fund(&mut self, address: &Pubkey, lamports: u64) {
        self.svm.airdrop(address, lamports).unwrap();
    }

    pub fn balance(&self, address: &Pubkey) -> u64 {
        self.svm.get_balance(address).unwrap_or(0)
    }

    pub fn data(&self, address: &Pubkey) -> Vec<u8> {
        self.svm
            .get_account(address)
            .map(|account| account.data)
            .unwrap_or_default()
    }

    /// An account as the app reads it over RPC. `None` when it does not exist.
    pub fn account(&self, address: &Pubkey) -> Option<wallet_core::RawAccount> {
        self.svm.get_account(address).map(Into::into)
    }

    /// What an account of `len` bytes has to hold to exist.
    pub fn rent(&self, len: usize) -> u64 {
        self.svm.minimum_balance_for_rent_exemption(len)
    }

    pub fn nonce_rent(&self) -> u64 {
        self.svm
            .minimum_balance_for_rent_exemption(EXECUTOR_NONCE_LEN)
    }

    pub fn nonce(&self, account: &Pubkey) -> ExecutorNonce {
        ExecutorNonce::parse(*account, &self.data(account)).expect("an initialized nonce account")
    }

    /// Compile with `wallet-core`, sign with `signers`, and run the exact wire
    /// bytes the app would hand to a node. The first signer pays the fee.
    pub fn send(
        &mut self,
        signers: &[&KeypairSigner],
        instructions: Vec<Instruction>,
    ) -> Result<Landed, Box<FailedTransactionMetadata>> {
        self.send_with(signers, instructions, ComputeBudgetSpec::default())
    }

    pub fn send_with(
        &mut self,
        signers: &[&KeypairSigner],
        instructions: Vec<Instruction>,
        budget: ComputeBudgetSpec,
    ) -> Result<Landed, Box<FailedTransactionMetadata>> {
        let compiled = compile_v1_budget(
            signers[0].pubkey(),
            instructions,
            None,
            Some(self.svm.latest_blockhash()),
            budget,
        )
        .expect("compiles");
        let signers: Vec<&dyn WalletSigner> = signers
            .iter()
            .map(|signer| *signer as &dyn WalletSigner)
            .collect();
        let wire = apply_local_signatures(&compiled.wire, &signers).expect("signs");
        self.send_wire(&wire)
    }

    /// Let `blocks` blockhashes go by, as a slow trip to another device does.
    pub fn age_blockhashes(&mut self, blocks: usize) {
        for _ in 0..blocks {
            self.svm.expire_blockhash();
        }
    }

    pub fn latest_blockhash(&self) -> solana_hash::Hash {
        self.svm.latest_blockhash()
    }

    /// Run wire bytes without keeping what they change or checking their
    /// signatures, the way a node simulates.
    pub fn simulate_wire(
        &self,
        wire: &[u8],
    ) -> Result<TransactionMetadata, Box<FailedTransactionMetadata>> {
        let transaction: VersionedTransaction =
            wincode::deserialize(wire).expect("the wire is a transaction");
        self.svm
            .clone()
            .with_sigverify(false)
            .simulate_transaction(transaction)
            .map(|simulated| simulated.meta)
            .map_err(Box::new)
    }

    /// Run wire bytes exactly as given, the way the app submits them.
    pub fn send_wire(&mut self, wire: &[u8]) -> Result<Landed, Box<FailedTransactionMetadata>> {
        assert!(wire.len() <= MAX_TRANSACTION_SIZE);

        let transaction: VersionedTransaction =
            wincode::deserialize(wire).expect("the wire is a transaction");
        let result = self.svm.send_transaction(transaction);
        // A fresh blockhash each time, so two identical transactions are both accepted
        // by the chain and only a program can refuse the second.
        self.svm.expire_blockhash();
        let meta: TransactionMetadata = result.map_err(Box::new)?;
        Ok(Landed {
            size: wire.len(),
            compute_units: meta.compute_units_consumed,
        })
    }
}

/// The program logs of a failed transaction, for assertions on why it failed.
pub fn logs(failure: &FailedTransactionMetadata) -> String {
    format!("{:?}\n{}", failure.err, failure.meta.logs.join("\n"))
}
