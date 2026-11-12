//! A desk for the shipped templates whose programs' IDLs ship for mainnet:
//! the chain carries those programs' mainnet builds and copies of the
//! mainnet accounts they read, the IDLs are mainnet's, and each template is
//! expanded with the facts the node gives, built block by block with the
//! IDLs, signed, landed and read back as Review and Sign reads it.

use super::node::Node;
use super::{logs, Chain, SOL};
use base64::Engine;
use solana_account::Account;
use solana_pubkey::Pubkey;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use wallet_core::template::{expand, Answers, Env, Expansion, Facts, Template};
use wallet_core::{
    builtin_template, compile_v1_budget, decode_transaction, display_accounts, explain_transaction,
    explain_with_note, idls_from_pack, ComputeBudgetSpec, DisplayNote, IdlNetwork, IdlStore,
    Instruction, KeypairSigner, LookupTables, MemoryBodies, MemoryStore, WalletSigner,
};

const COMPUTE_BUDGET: Pubkey =
    Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");

pub struct TemplateDesk {
    pub node: Node,
    pub wallet: KeypairSigner,
    pub idls: IdlStore<MemoryStore, MemoryBodies>,
}

/// What a landed template wrote.
pub struct Landed {
    pub expansion: Expansion,
    pub instructions: Vec<Instruction>,
}

/// A file in `crates/flow-tests/`.
pub fn flow_file(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

impl TemplateDesk {
    /// A chain carrying `programs` (address, file in `programs/`) beside
    /// the ones every chain has, the accounts of each snapshot in
    /// `fixtures/`, and a wallet holding 100 SOL. The IDLs are those that
    /// ship for `network`.
    pub fn new(network: IdlNetwork, programs: &[(Pubkey, &str)], snapshots: &[&str]) -> Self {
        let wallet = KeypairSigner::new_random();
        let mut chain = Chain::new();
        for (program, file) in programs {
            chain.load_program(*program, file);
        }
        for snapshot in snapshots {
            chain.plant_snapshot(&flow_file(&format!("fixtures/{snapshot}")));
        }
        chain.fund(&wallet.pubkey(), 100 * SOL);
        let mut idls =
            IdlStore::open_on(MemoryStore::new(), MemoryBodies::new(), network).with_builtins();
        let packed = idl_pack::pack_dir(&flow_file("../../resources/idls")).unwrap();
        let refused = idls.add_embedded(idls_from_pack(&packed.bytes, &packed.sha256).unwrap());
        assert!(refused.is_empty(), "{refused:?}");
        Self {
            node: Node(Mutex::new(chain)),
            wallet,
            idls,
        }
    }

    pub fn me(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    /// Expands, asking the node for each fact until none is needed.
    pub fn expand(&self, template: &Template, answers: &Answers, env: &Env) -> Expansion {
        let mut facts = Facts::default();
        for _ in 0..4 {
            let expansion = expand(template, answers, env, &facts, &self.idls)
                .unwrap_or_else(|problems| panic!("{problems:#?}"));
            if expansion.needs.is_empty() {
                assert!(expansion.missing.is_empty(), "{:?}", expansion.missing);
                return expansion;
            }
            let read =
                pollster::block_on(wallet_flows::template_facts(&self.node, &expansion.needs))
                    .unwrap_or_else(|err| panic!("{err}"));
            facts.merge(read);
        }
        panic!("facts kept being needed");
    }

    /// Expands the shipped template `id` with these options and answers,
    /// the wallet as `$wallet` and the fee payer, and `keys` as its
    /// `new-keypair` values; builds every block with its IDL, and lands the
    /// transaction signed by the wallet, the keys and `others`.
    pub fn land(
        &self,
        id: &str,
        options: &[&str],
        answers: &[(&str, String)],
        keys: &[(&str, &KeypairSigner)],
        others: &[&KeypairSigner],
    ) -> Result<Landed, String> {
        let template = builtin_template(id).unwrap().template().unwrap();
        let mut form = Answers::for_template(&template);
        for option in options {
            form = form.turn_on(option);
        }
        for (input, text) in answers {
            form = form.answer(input, text);
        }
        let env = Env {
            wallet: Some(self.me()),
            fee_payer: Some(self.me()),
            keypairs: keys
                .iter()
                .map(|(name, key)| (name.to_string(), key.pubkey()))
                .collect(),
        };
        let mut expansion = self.expand(&template, &form, &env);
        let mut instructions = Vec::new();
        for block in &mut expansion.blocks {
            let program = block.draft.program_id.expect("every program is known");
            let client = self.idls.program_client(&program).unwrap();
            instructions.push(
                block
                    .draft
                    .build(&client)
                    .unwrap_or_else(|err| panic!("{id} block {}: {err}", block.number)),
            );
        }
        let mut signers = vec![&self.wallet];
        signers.extend(keys.iter().map(|(_, key)| *key));
        signers.extend(others.iter().copied());
        // A key can be named twice; it signs once.
        signers.dedup_by_key(|signer| signer.pubkey());
        self.node
            .chain()
            .send(&signers, instructions.clone())
            .map_err(|failure| logs(&failure))?;
        Ok(Landed {
            expansion,
            instructions,
        })
    }

    /// What Review and Sign says each instruction does, its sentence with
    /// the values put in, in order. As Review and Sign does, the
    /// transaction is read once, the accounts its sentences need (a mint's
    /// decimals) are read from the node, and it is read again with them.
    pub fn sentences(&self, instructions: &[Instruction]) -> Vec<String> {
        let compiled = compile_v1_budget(
            self.me(),
            instructions.to_vec(),
            None,
            Some(self.node.chain().latest_blockhash()),
            ComputeBudgetSpec::default(),
        )
        .expect("compiles");
        let tx = decode_transaction(&compiled.wire, &LookupTables::default()).unwrap();
        let first = explain_transaction(&self.idls, &tx);
        let needed: Vec<(Pubkey, Pubkey)> = first
            .instructions
            .iter()
            .flat_map(|explained| {
                let reader = explained.program_id;
                explained
                    .idl
                    .iter()
                    .flat_map(move |idl| idl.accounts_needed.iter().map(move |pk| (*pk, reader)))
            })
            .collect();
        let fetched: Vec<_> = needed
            .iter()
            .map(|(address, _)| self.node.chain().account(address))
            .collect();
        let note = DisplayNote {
            accounts: display_accounts(&needed, &fetched),
            ..DisplayNote::default()
        };
        explain_with_note(&self.idls, &tx, &note)
            .instructions
            .into_iter()
            .filter(|explained| explained.program_id != COMPUTE_BUDGET)
            .map(|explained| {
                let idl = explained.idl.expect("every instruction reads with its IDL");
                idl.interpolated_intent.unwrap_or(idl.intent)
            })
            .collect()
    }

    pub fn data(&self, address: &Pubkey) -> Vec<u8> {
        self.node.chain().data(address)
    }

    pub fn balance(&self, address: &Pubkey) -> u64 {
        self.node.chain().balance(address)
    }

    pub fn exists(&self, address: &Pubkey) -> bool {
        self.node.chain().account(address).is_some()
    }
}

const CLOCK: Pubkey = Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111");

impl Chain {
    /// Set the clock's time and epoch, and when that epoch started: the
    /// Clock sysvar's `unix_timestamp`, `epoch` and `epoch_start_timestamp`.
    pub fn set_time(&mut self, unix_timestamp: i64, epoch: u64, epoch_start: i64) {
        let mut clock = self.svm.get_account(&CLOCK).expect("the clock");
        clock.data[8..16].copy_from_slice(&epoch_start.to_le_bytes());
        clock.data[16..24].copy_from_slice(&epoch.to_le_bytes());
        clock.data[24..32].copy_from_slice(&(epoch + 1).to_le_bytes());
        clock.data[32..40].copy_from_slice(&unix_timestamp.to_le_bytes());
        self.svm.set_account(CLOCK, clock).unwrap();
    }

    /// Load the program in `programs/<file>` at `program_id`.
    pub fn load_program(&mut self, program_id: Pubkey, file: &str) {
        let path = flow_file(&format!("programs/{file}"));
        self.svm
            .add_program_from_file(program_id, &path)
            .unwrap_or_else(|err| {
                panic!(
                    "could not load {}: {err}\nRun `node scripts/fetch-programs.mjs` first.",
                    path.display()
                )
            });
    }

    /// Put every account of a snapshot file on the chain, as it was read:
    /// `{"accounts": {address: {owner, lamports, executable, data}}}`, the
    /// data in base64.
    pub fn plant_snapshot(&mut self, path: &std::path::Path) {
        let text =
            std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let file: serde_json::Value = serde_json::from_str(&text).unwrap();
        for (address, account) in file["accounts"].as_object().unwrap() {
            let data = base64::engine::general_purpose::STANDARD
                .decode(account["data"].as_str().unwrap())
                .unwrap();
            self.svm
                .set_account(
                    Pubkey::from_str(address).unwrap(),
                    Account {
                        lamports: account["lamports"].as_u64().unwrap(),
                        data,
                        owner: Pubkey::from_str(account["owner"].as_str().unwrap()).unwrap(),
                        executable: account["executable"].as_bool().unwrap(),
                        rent_epoch: 0,
                    },
                )
                .unwrap();
        }
    }
}

/// Reads Borsh as Metaplex and Streamflow write their accounts.
pub struct Reader<'a> {
    pub data: &'a [u8],
    pub at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], at: usize) -> Self {
        Self { data, at }
    }

    pub fn take(&mut self, len: usize) -> &'a [u8] {
        let bytes = &self.data[self.at..self.at + len];
        self.at += len;
        bytes
    }

    pub fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }

    pub fn bool(&mut self) -> bool {
        self.u8() == 1
    }

    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take(2).try_into().unwrap())
    }

    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }

    pub fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take(8).try_into().unwrap())
    }

    pub fn pubkey(&mut self) -> Pubkey {
        Pubkey::try_from(self.take(32)).unwrap()
    }

    /// A string with a u32 length, without the zeros a program pads it with.
    pub fn string(&mut self) -> String {
        let len = self.u32() as usize;
        String::from_utf8(self.take(len).to_vec())
            .unwrap()
            .trim_end_matches('\0')
            .to_string()
    }

    /// Whether an option holds a value.
    pub fn some(&mut self) -> bool {
        self.bool()
    }
}
