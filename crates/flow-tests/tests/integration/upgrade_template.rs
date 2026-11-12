//! The shipped template "Upgrade a program", against the real upgradeable
//! loader, program-metadata and OtterSec's verify program.
//!
//! The chain holds what the command-line tools leave before an upgrade: a
//! program deployed by the wallet, a loader buffer holding the new build,
//! the program's canonical IDL account and a program-metadata buffer holding
//! the new IDL, both written by `program-metadata`, and the record
//! `solana-verify` wrote. The template is expanded and built with the IDLs
//! that ship with the app, landed, and every account read back.

use crate::support::node::Node;
use crate::support::{logs, Chain, SOL};
use solana_pubkey::Pubkey;
use std::path::PathBuf;
use std::sync::Mutex;
use wallet_core::program_code::BPF_LOADER_UPGRADEABLE_ID;
use wallet_core::template::{expand, Answers, Env, Expansion, Facts};
use wallet_core::{
    build_idl_instruction, builtin_template, idls_from_pack, program_metadata_idl_address,
    IdlBuildInput, IdlNetwork, IdlStore, KeypairSigner, MemoryBodies, MemoryStore, WalletSigner,
};

const PROGRAM_METADATA: Pubkey =
    Pubkey::from_str_const("ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S");
const OTTER_VERIFY: Pubkey = Pubkey::from_str_const("verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC");

/// Bytes before the code in a program data account, and in a loader buffer.
const PROGRAM_DATA_HEADER: usize = 45;
const BUFFER_HEADER: usize = 37;
/// Bytes before the data in a program-metadata account or buffer.
const METADATA_HEADER: usize = 96;

const OLD_IDL: &[u8] = br#"{"name":"old"}"#;

struct Desk {
    node: Node,
    wallet: KeypairSigner,
    idls: IdlStore<MemoryStore, MemoryBodies>,
    program: Pubkey,
    program_data: Pubkey,
    buffer: Pubkey,
    new_code: Vec<u8>,
}

fn program_file(file: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("programs")
        .join(file);
    std::fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// The header of the program's canonical IDL account, holding `len` bytes.
fn metadata_header(program: &Pubkey, len: usize) -> Vec<u8> {
    let mut seed = [0u8; 16];
    seed[..3].copy_from_slice(b"idl");
    let mut header = vec![2];
    header.extend_from_slice(program.as_ref());
    // No authority of its own: the program's upgrade authority manages it.
    header.extend_from_slice(&[0; 32]);
    header.extend_from_slice(&[1, 1]); // mutable, canonical
    header.extend_from_slice(&seed);
    header.extend_from_slice(&[1, 0, 1, 0]); // utf8, no compression, json, direct
    header.extend_from_slice(&(len as u32).to_le_bytes());
    header.extend_from_slice(&[0; 5]);
    assert_eq!(header.len(), METADATA_HEADER);
    header
}

/// The header of a program-metadata buffer for the program's canonical
/// IDL, written by `authority`.
fn buffer_header(program: &Pubkey, authority: &Pubkey) -> Vec<u8> {
    let mut seed = [0u8; 16];
    seed[..3].copy_from_slice(b"idl");
    let mut header = vec![1];
    header.extend_from_slice(program.as_ref());
    header.extend_from_slice(authority.as_ref());
    header.push(1); // canonical
    header.extend_from_slice(&seed);
    header.extend_from_slice(&[0; 14]);
    assert_eq!(header.len(), METADATA_HEADER);
    header
}

impl Desk {
    fn new() -> Self {
        let wallet = KeypairSigner::new_random();
        let mut chain = Chain::new();
        chain.fund(&wallet.pubkey(), 100 * SOL);

        // Two builds that run: the one deployed and the one to deploy.
        let old_code = program_file("nonce.so");
        let new_code = program_file("ed25519_signer.so");
        let program = KeypairSigner::new_random().pubkey();
        let room = old_code.len().max(new_code.len());
        let program_data = chain.plant_upgradeable(program, wallet.pubkey(), &old_code, room);
        let buffer = KeypairSigner::new_random().pubkey();
        let mut data = vec![1, 0, 0, 0, 1];
        data.extend_from_slice(wallet.pubkey().as_ref());
        data.extend_from_slice(&new_code);
        assert_eq!(data.len(), BUFFER_HEADER + new_code.len());
        chain.plant(buffer, BPF_LOADER_UPGRADEABLE_ID, data);
        // The loader refuses to upgrade in the slot a program was deployed.
        chain.warp(10);

        let mut idls =
            IdlStore::open_on(MemoryStore::new(), MemoryBodies::new(), IdlNetwork::Devnet)
                .with_builtins();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/idls");
        let packed = idl_pack::pack_dir(&dir).unwrap();
        let refused = idls.add_embedded(idls_from_pack(&packed.bytes, &packed.sha256).unwrap());
        assert!(refused.is_empty(), "{refused:?}");

        Self {
            node: Node(Mutex::new(chain)),
            wallet,
            idls,
            program,
            program_data,
            buffer,
            new_code,
        }
    }

    fn me(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    /// The program's IDL account holding `OLD_IDL`, and a buffer holding
    /// `idl`, as `program-metadata write idl` leaves them. Returns both.
    fn plant_idl(&self, idl: &[u8]) -> (Pubkey, Pubkey) {
        let account = program_metadata_idl_address(&self.program).unwrap();
        let mut data = metadata_header(&self.program, OLD_IDL.len());
        data.extend_from_slice(OLD_IDL);
        let mut chain = self.node.chain();
        chain.plant(account, PROGRAM_METADATA, data);
        let buffer = KeypairSigner::new_random().pubkey();
        let mut data = buffer_header(&self.program, &self.me());
        data.extend_from_slice(idl);
        chain.plant(buffer, PROGRAM_METADATA, data);
        (account, buffer)
    }

    /// The record `solana-verify` writes, made with the IDL as Build makes
    /// it. Returns its address.
    fn record_build(&self, commit: &str) -> Pubkey {
        let client = self.idls.program_client(&OTTER_VERIFY).unwrap();
        let input = IdlBuildInput {
            accounts: vec![
                ("authority".to_string(), Some(self.me())),
                ("programAddress".to_string(), Some(self.program)),
            ],
            arguments: vec![
                ("params.version".to_string(), "0.4.10".to_string()),
                (
                    "params.gitUrl".to_string(),
                    "https://example.com/p".to_string(),
                ),
                ("params.commit".to_string(), commit.to_string()),
                ("params.args".to_string(), String::new()),
                ("params.deploySlot".to_string(), "1".to_string()),
            ],
        };
        let initialize = build_idl_instruction(&client, "initialize", &input).unwrap();
        self.node
            .chain()
            .send(&[&self.wallet], vec![initialize])
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
        Pubkey::find_program_address(
            &[b"otter_verify", self.me().as_ref(), self.program.as_ref()],
            &OTTER_VERIFY,
        )
        .0
    }

    /// Expands the template with these options and answers, builds every
    /// block with its IDL and lands the transaction.
    fn land(&self, options: &[&str], answers: &[(&str, String)]) -> Result<Expansion, String> {
        let template = builtin_template("upgrade-program")
            .unwrap()
            .template()
            .unwrap();
        let mut form = Answers::for_template(&template)
            .answer("program", &self.program.to_string())
            .answer("buffer", &self.buffer.to_string());
        for option in options {
            form = form.turn_on(option);
        }
        for (input, text) in answers {
            form = form.answer(input, text);
        }
        let env = Env {
            wallet: Some(self.me()),
            fee_payer: Some(self.me()),
            keypairs: Default::default(),
        };
        let mut expansion = expand(&template, &form, &env, &Facts::default(), &self.idls)
            .unwrap_or_else(|problems| panic!("{problems:#?}"));
        assert!(expansion.needs.is_empty(), "{:?}", expansion.needs);
        assert!(expansion.missing.is_empty(), "{:?}", expansion.missing);
        let mut instructions = Vec::new();
        for block in &mut expansion.blocks {
            let program = block.draft.program_id.expect("every program is known");
            let client = self.idls.program_client(&program).unwrap();
            instructions.push(
                block
                    .draft
                    .build(&client)
                    .unwrap_or_else(|err| panic!("block {}: {err}", block.number)),
            );
        }
        self.node
            .chain()
            .send(&[&self.wallet], instructions)
            .map_err(|failure| logs(&failure))?;
        Ok(expansion)
    }

    fn data(&self, address: &Pubkey) -> Vec<u8> {
        self.node.chain().data(address)
    }

    fn balance(&self, address: &Pubkey) -> u64 {
        self.node.chain().balance(address)
    }

    fn exists(&self, address: &Pubkey) -> bool {
        self.node.chain().account(address).is_some()
    }

    /// The program runs the new build: its program data holds it, from the
    /// slot it landed in.
    fn assert_upgraded(&self) {
        let data = self.data(&self.program_data);
        assert_eq!(&data[4..12], &10u64.to_le_bytes(), "the deploy slot");
        assert_eq!(
            &data[PROGRAM_DATA_HEADER..PROGRAM_DATA_HEADER + self.new_code.len()],
            &self.new_code[..]
        );
        assert!(
            data[PROGRAM_DATA_HEADER + self.new_code.len()..]
                .iter()
                .all(|byte| *byte == 0),
            "the rest of the program data is cleared"
        );
        assert!(!self.exists(&self.buffer), "the buffer is closed");
    }
}

#[test]
fn an_upgrade_alone_deploys_the_buffer_and_refunds_it() {
    let desk = Desk::new();
    let refund = KeypairSigner::new_random().pubkey();
    let buffer_lamports = desk.balance(&desk.buffer);
    let expansion = desk
        .land(&[], &[("refund", refund.to_string())])
        .unwrap_or_else(|logs| panic!("{logs}"));
    assert_eq!(expansion.blocks.len(), 1);
    desk.assert_upgraded();
    assert_eq!(desk.balance(&refund), buffer_lamports);
}

#[test]
fn an_upgrade_with_its_idl_and_verified_build_lands_all_three() {
    let desk = Desk::new();
    let refund = KeypairSigner::new_random().pubkey();
    // Larger than the IDL it replaces by more than its buffer's header, so
    // the IDL account grows on what the closed buffer held.
    let new_idl = format!(r#"{{"name":"new","pad":"{}"}}"#, "x".repeat(400));
    let (idl_account, idl_buffer) = desk.plant_idl(new_idl.as_bytes());
    let record = desk.record_build("abc");
    let paid = desk.balance(&desk.buffer) + desk.balance(&idl_account) + desk.balance(&idl_buffer);

    desk.land(
        &["idl", "verified_build"],
        &[
            ("refund", refund.to_string()),
            ("idl_buffer", idl_buffer.to_string()),
            ("compression", "none".to_string()),
            ("repository", "https://example.com/p".to_string()),
            ("commit", "def".to_string()),
            ("version", "0.4.11".to_string()),
            ("build_args", r#"["--library-name", "p"]"#.to_string()),
            ("deploy_slot", "10".to_string()),
        ],
    )
    .unwrap_or_else(|logs| panic!("{logs}"));
    desk.assert_upgraded();

    // The IDL account holds the new IDL, at exactly its size, and the
    // buffer is gone.
    let data = desk.data(&idl_account);
    assert_eq!(data.len(), METADATA_HEADER + new_idl.len());
    assert_eq!(&data[METADATA_HEADER..], new_idl.as_bytes());
    assert_eq!(&data[83..87], &[1, 0, 1, 0], "utf8, no compression, json");
    assert_eq!(&data[87..91], &(new_idl.len() as u32).to_le_bytes());
    assert!(!desk.exists(&idl_buffer), "the IDL buffer is closed");
    // It keeps its rent and no more; the rest went to the refund address,
    // with the loader buffer's lamports.
    let rent = desk.node.chain().rent(data.len());
    assert_eq!(desk.balance(&idl_account), rent);
    assert_eq!(desk.balance(&refund), paid - rent);

    // The record names the new commit, version, arguments and slot.
    let data = desk.data(&record);
    let mut at = 8 + 32 + 32;
    let mut text = || {
        let len = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let text = String::from_utf8(data[at + 4..at + 4 + len].to_vec()).unwrap();
        at += 4 + len;
        text
    };
    assert_eq!(text(), "0.4.11");
    assert_eq!(text(), "https://example.com/p");
    assert_eq!(text(), "def");
    let count = u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    assert_eq!(count, 2);
}

#[test]
fn an_idl_buffer_someone_else_controls_refuses_the_whole_upgrade() {
    let desk = Desk::new();
    let (_, idl_buffer) = desk.plant_idl(b"{}");
    let stranger = KeypairSigner::new_random().pubkey();
    // Not canonical either, so the program's upgrade authority cannot close
    // it in the stranger's place.
    desk.node.chain().patch_data(&idl_buffer, |data| {
        data[33..65].copy_from_slice(stranger.as_ref());
        data[65] = 0;
    });
    let program_data = desk.data(&desk.program_data);
    let refused = desk
        .land(&["idl"], &[("idl_buffer", idl_buffer.to_string())])
        .unwrap_err();
    // Closing it (instruction 2) is refused, and the upgrade with it.
    assert!(
        refused.starts_with("InstructionError(2, IncorrectAuthority)"),
        "{refused}"
    );
    assert_eq!(
        desk.data(&desk.program_data),
        program_data,
        "nothing upgraded"
    );
}
