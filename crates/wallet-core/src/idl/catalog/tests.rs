use super::*;
use crate::idl::builtin::builtin_idl;
use wallet_vault::store::MemoryStore;

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn codama(program: Pubkey, version: &str) -> Vec<u8> {
    format!(
        r#"{{
            "kind": "rootNode",
            "standard": "codama",
            "version": "1.0.0",
            "program": {{
                "kind": "programNode",
                "name": "myProgram",
                "publicKey": "{program}",
                "version": "{version}"
            }},
            "additionalPrograms": []
        }}"#
    )
    .into_bytes()
}

fn open() -> IdlStore<MemoryStore, MemoryBodies> {
    IdlStore::open(MemoryStore::new(), MemoryBodies::new())
}

#[test]
fn round_trip_keeps_the_original_bytes() {
    let mut store = open();
    let program = pk(4);
    let bytes = codama(program, "0.1.0");
    let record = store.import(&bytes, None).unwrap();
    assert_eq!(record.spec, IdlSpec::Codama);
    assert_eq!(record.sha256, sha256(&bytes));

    let again = IdlStore::open(store.index_store, store.bodies);
    assert_eq!(again.records().len(), 1);
    assert_eq!(again.body(&program), Some(bytes.as_slice()));
    assert_eq!(again.records()[0].sha256, record.sha256);
}

#[test]
fn replace_keeps_one_record_and_changes_the_checksum() {
    let mut store = open();
    let program = pk(4);
    let first = store.import(&codama(program, "0.1.0"), None).unwrap();
    let second = store.import(&codama(program, "0.2.0"), None).unwrap();
    assert_eq!(store.records().len(), 1);
    assert_ne!(first.sha256, second.sha256);
    assert_eq!(store.records()[0].sha256, second.sha256);
}

#[test]
fn anchor_without_an_address_uses_the_given_program_id() {
    let mut store = open();
    let program = pk(9);
    let bytes = br#"{"name":"tiny","instructions":[]}"#;
    let record = store.import(bytes, Some(program)).unwrap();
    assert_eq!(record.program_id, program);
    assert_eq!(record.spec, IdlSpec::Anchor);
    assert_eq!(record.name, "tiny");
    assert_eq!(store.body(&program), Some(bytes.as_slice()));
}

#[test]
fn corrupt_json_does_not_replace_a_stored_body() {
    let mut store = open();
    let program = pk(4);
    let bytes = codama(program, "0.1.0");
    store.import(&bytes, None).unwrap();
    let err = store.import(b"{", None).unwrap_err();
    assert!(matches!(err, IdlError::NotJson));
    assert_eq!(store.records().len(), 1);
    assert_eq!(store.body(&program), Some(bytes.as_slice()));
}

#[test]
fn bundle_round_trip_and_remove() {
    let mut store = open();
    let program = pk(4);
    store.import(&codama(program, "0.1.0"), None).unwrap();
    let anchor = br#"{"name":"tiny","instructions":[{"name":"ping","accounts":[],"args":[]}]}"#;
    let anchor_id = pk(9);
    store.import(anchor, Some(anchor_id)).unwrap();

    let files = store.export_files().unwrap();
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let mut restored = open();
    let report = restored.import_bundle(&borrowed).unwrap();
    assert_eq!(report.imported.len(), 2);
    assert!(report.skipped.is_empty());
    assert_eq!(restored.body(&program), store.body(&program));

    let outline = outline_idl(anchor, anchor_id).unwrap();
    assert_eq!(outline.name, "tiny");
    assert_eq!(outline.instructions.len(), 1);
    assert_eq!(outline.instructions[0].name, "ping");

    restored.remove(&program).unwrap();
    assert_eq!(restored.records().len(), 1);
    assert!(restored.body(&program).is_none());
}

#[test]
fn bundle_skips_a_missing_body() {
    let mut source = open();
    let program = pk(4);
    source.import(&codama(program, "0.1.0"), None).unwrap();
    let mut files = source.export_files().unwrap();
    files.retain(|(name, _)| name == "index.json");
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let mut store = open();
    let report = store.import_bundle(&borrowed).unwrap();
    assert!(report.imported.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(store.records().is_empty());
}

#[test]
fn check_compares_bytes_and_leaves_the_body_alone() {
    let mut store = open();
    let program = pk(4);
    let bytes = codama(program, "0.1.0");
    store.import(&bytes, None).unwrap();

    let same = store.record_check(&program, &bytes).unwrap();
    assert!(same.same);
    assert_eq!(same.remote_sha256, Some(sha256(&bytes)));
    assert_eq!(store.body(&program), Some(bytes.as_slice()));

    let other = codama(program, "9.9.9");
    let differs = store.record_check(&program, &other).unwrap();
    assert!(!differs.same);
    assert_eq!(store.body(&program), Some(bytes.as_slice()));
    assert_eq!(store.records()[0].sha256, sha256(&bytes));
}

#[test]
fn anchor_account_inflates_to_the_json() {
    let json = br#"{"name":"tiny","instructions":[]}"#;
    let mut compressed = Vec::new();
    {
        let mut encoder =
            flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, json).unwrap();
        encoder.finish().unwrap();
    }
    let mut data = anchor_idl_discriminator().to_vec();
    data.extend_from_slice(&[7u8; 32]);
    data.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    data.extend_from_slice(&compressed);
    assert_eq!(idl_bytes_from_anchor_account(&data).unwrap(), json);

    data[0] ^= 0xff;
    assert!(idl_bytes_from_anchor_account(&data).is_err());
    let program = pk(3);
    let address = anchor_idl_address(&program).unwrap();
    assert_ne!(address, program);
}

#[test]
fn the_account_bytes_built_for_a_test_chain_read_back_as_the_json() {
    let json = br#"{"name":"tiny","instructions":[]}"#;
    let anchor = anchor_idl_account_data(json);
    assert_eq!(idl_bytes_from_anchor_account(&anchor).unwrap(), json);
    let metadata = program_metadata_account_data(json);
    assert_eq!(idl_bytes_from_program_metadata(&metadata).unwrap(), json);
    // Neither layout reads as the other.
    assert!(idl_bytes_from_anchor_account(&metadata).is_err());
    assert!(idl_bytes_from_program_metadata(&anchor).is_err());
}

#[test]
fn jupiter_anchor_idl_address_is_the_deployed_account() {
    let program = Pubkey::from_str("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4").unwrap();
    let expected = Pubkey::from_str("C88XWfp26heEmDkmfSzeXP7Fd7GQJ2j9dDTUsyiZbUTa").unwrap();
    assert_eq!(anchor_idl_address(&program).unwrap(), expected);
}

#[test]
fn an_import_replaces_an_embedded_idl_and_removing_it_brings_it_back() {
    let system = crate::ids::SYSTEM_PROGRAM_ID;
    let index = MemoryStore::new();
    let mut store = IdlStore::open(index, MemoryBodies::new()).with_builtins();
    assert_eq!(files(&store), builtins_on(IdlNetwork::Devnet));
    let embedded = store
        .records()
        .iter()
        .find(|r| r.program_id == system)
        .unwrap()
        .clone();
    assert_eq!(embedded.origin, IdlOrigin::Builtin);
    assert!(store.program_client(&system).is_some());

    // Embedded IDLs are neither exported nor removable.
    assert_eq!(store.export_files().unwrap().len(), 1);
    assert!(matches!(store.remove(&system), Err(IdlError::Builtin(_))));

    // The same file with different whitespace is a different, newer import.
    let newer = format!(
        "{}
",
        builtin_idl(IdlNetwork::Devnet, &system).unwrap().json
    );
    let imported = store.import(newer.as_bytes(), None).unwrap();
    assert_eq!(imported.origin, IdlOrigin::File);
    assert_ne!(imported.sha256, embedded.sha256);
    assert_eq!(files(&store), builtins_on(IdlNetwork::Devnet));
    assert_eq!(store.export_files().unwrap().len(), 2);

    store.remove(&system).unwrap();
    let back = store
        .records()
        .iter()
        .find(|r| r.program_id == system)
        .unwrap();
    assert_eq!(back, &embedded);
    assert_eq!(
        store.body(&system).unwrap(),
        builtin_idl(IdlNetwork::Devnet, &system)
            .unwrap()
            .json
            .as_bytes()
    );
}

#[test]
fn an_idl_that_inflates_past_the_limit_is_refused() {
    let mut compressed = Vec::new();
    {
        let mut encoder =
            flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::best());
        let zeros = vec![0u8; 1 << 20];
        for _ in 0..8 {
            std::io::Write::write_all(&mut encoder, &zeros).unwrap();
        }
        encoder.finish().unwrap();
    }
    assert!(compressed.len() < 100_000);
    let mut data = anchor_idl_discriminator().to_vec();
    data.extend_from_slice(&[0u8; 32]);
    data.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    data.extend_from_slice(&compressed);
    let err = idl_bytes_from_anchor_account(&data).unwrap_err();
    assert!(err.to_string().contains("larger than 2 MB"), "{err}");
}

#[test]
fn internal_idl_discriminator_inflates() {
    let json = br#"{"name":"tiny","instructions":[]}"#;
    let mut compressed = Vec::new();
    {
        let mut encoder =
            flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, json).unwrap();
        encoder.finish().unwrap();
    }
    let mut data = internal_idl_discriminator().to_vec();
    data.extend_from_slice(&[7u8; 32]);
    data.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    data.extend_from_slice(&compressed);
    assert_eq!(idl_bytes_from_anchor_account(&data).unwrap(), json);
    assert_eq!(
        internal_idl_discriminator(),
        [0x18, 0x46, 0x62, 0xbf, 0x3a, 0x90, 0x7b, 0x9e]
    );
}

#[test]
fn program_metadata_inflates_inline_zlib_utf8() {
    let json = br#"{"name":"tiny","instructions":[]}"#;
    let mut compressed = Vec::new();
    {
        let mut encoder =
            flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, json).unwrap();
        encoder.finish().unwrap();
    }
    let mut data = vec![0u8; 96];
    data[0] = 2;
    data[83] = 1;
    data[84] = 2;
    data[87..91].copy_from_slice(&(compressed.len() as u32).to_le_bytes());
    data.extend_from_slice(&compressed);
    assert_eq!(idl_bytes_from_program_metadata(&data).unwrap(), json);

    data[86] = 1;
    assert!(idl_bytes_from_program_metadata(&data).is_err());
    let program = pk(9);
    let pda = program_metadata_idl_address(&program).unwrap();
    assert_ne!(pda, program);
    assert_ne!(pda, anchor_idl_address(&program).unwrap());
}

#[test]
fn file_program_id_must_match_the_one_given() {
    let mut store = open();
    let err = store
        .import(&codama(pk(4), "0.1.0"), Some(pk(5)))
        .unwrap_err();
    assert!(matches!(err, IdlError::ProgramIdMismatch { .. }));
    assert!(store.records().is_empty());
}

/// A Codama file for `program` carrying `extras` in `additionalPrograms`, each
/// with that many instructions.
fn with_additional(program: Pubkey, extras: &[(Pubkey, &str, usize)]) -> Vec<u8> {
    let instruction = |index: usize| {
        serde_json::json!({
            "kind": "instructionNode",
            "name": format!("ix{index}"),
            "accounts": [],
            "arguments": [],
        })
    };
    let additional: Vec<serde_json::Value> = extras
        .iter()
        .map(|(id, name, instructions)| {
            serde_json::json!({
                "kind": "programNode",
                "name": name,
                "publicKey": id.to_string(),
                "version": "1.0.0",
                "instructions": (0..*instructions).map(instruction).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({
        "kind": "rootNode",
        "standard": "codama",
        "version": "1.0.0",
        "program": {
            "kind": "programNode",
            "name": format!("host{}", program.to_bytes()[0]),
            "publicKey": program.to_string(),
            "version": "1.0.0",
        },
        "additionalPrograms": additional,
    }))
    .unwrap()
}

fn record(store: &IdlStore<MemoryStore, MemoryBodies>, program_id: Pubkey) -> Option<IdlRecord> {
    store
        .records()
        .iter()
        .find(|record| record.program_id == program_id)
        .cloned()
}

#[test]
fn an_additional_program_is_read_from_its_host_and_goes_with_it() {
    let mut store = open();
    let host = store
        .import(&with_additional(pk(1), &[(pk(2), "extra", 1)]), None)
        .unwrap();
    let extra = record(&store, pk(2)).unwrap();
    assert_eq!(extra.name, "extra");
    assert_eq!(extra.host, Some(pk(1)));
    assert_eq!(extra.sha256, host.sha256);
    assert_eq!(extra.origin, IdlOrigin::File);
    assert_eq!(store.body(&pk(2)), store.body(&pk(1)));
    let client = store.program_client(&pk(2)).unwrap();
    assert_eq!(client.program_address(), pk(2));
    assert_eq!(client.instruction_names(), vec!["ix0"]);
    assert_eq!(store.outline(&pk(2)).unwrap().name, "extra");

    // Neither stored nor exported, and found again when the store is opened.
    assert_eq!(store.export_files().unwrap().len(), 2);
    assert!(matches!(
        store.remove(&pk(2)),
        Err(IdlError::Companion { .. })
    ));
    let mut store = IdlStore::open(store.index_store, store.bodies);
    assert_eq!(record(&store, pk(2)), Some(extra));

    store.remove(&pk(1)).unwrap();
    assert!(store.records().is_empty());
    assert!(store.program_client(&pk(2)).is_none());
}

#[test]
fn a_file_of_its_own_wins_over_an_additional_program() {
    let mut store = open();
    store
        .import(&with_additional(pk(1), &[(pk(2), "extra", 3)]), None)
        .unwrap();
    let own = store.import(&codama(pk(2), "0.2.0"), None).unwrap();
    assert_eq!(record(&store, pk(2)), Some(own));
    assert!(store
        .program_client(&pk(2))
        .unwrap()
        .instruction_names()
        .is_empty());

    store.remove(&pk(2)).unwrap();
    assert_eq!(record(&store, pk(2)).unwrap().host, Some(pk(1)));
    assert_eq!(
        store
            .program_client(&pk(2))
            .unwrap()
            .instruction_names()
            .len(),
        3
    );
}

#[test]
fn the_fullest_entry_wins_when_several_files_carry_a_program() {
    let mut store = open();
    store
        .import(&with_additional(pk(1), &[(pk(3), "short", 1)]), None)
        .unwrap();
    store
        .import(&with_additional(pk(2), &[(pk(3), "full", 2)]), None)
        .unwrap();
    assert_eq!(record(&store, pk(3)).unwrap().host, Some(pk(2)));
    assert_eq!(
        store
            .program_client(&pk(3))
            .unwrap()
            .instruction_names()
            .len(),
        2
    );

    // The fuller one is replaced by a shorter one: the other file wins now.
    store
        .import(&with_additional(pk(2), &[(pk(3), "full", 0)]), None)
        .unwrap();
    assert_eq!(record(&store, pk(3)).unwrap().host, Some(pk(1)));
    assert_eq!(
        store
            .program_client(&pk(3))
            .unwrap()
            .instruction_names()
            .len(),
        1
    );
}

#[test]
fn checking_an_additional_program_checks_its_host() {
    let mut store = open();
    let bytes = with_additional(pk(1), &[(pk(2), "extra", 1)]);
    store.import(&bytes, None).unwrap();
    let check = store.record_check(&pk(2), &bytes).unwrap();
    assert!(check.same);
    assert_eq!(record(&store, pk(1)).unwrap().check, Some(check.clone()));
    assert_eq!(record(&store, pk(2)).unwrap().check, Some(check));
}

/// Token and Token-2022 carry the same associated-token program, apart from
/// the token program default. Neither is fuller, so Token, listed first in
/// [`BUILTIN_IDLS`], is the one read.
#[test]
fn the_associated_token_program_is_read_from_the_token_idl() {
    let entry = |program_id: &Pubkey| {
        let json = builtin_idl(IdlNetwork::Devnet, program_id).unwrap().json;
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        value["additionalPrograms"][0].clone()
    };
    let token = entry(&crate::ids::TOKEN_PROGRAM_ID);
    let token_2022 = entry(&crate::ids::TOKEN_2022_PROGRAM_ID);
    assert_eq!(token["publicKey"], token_2022["publicKey"]);
    assert_eq!(
        companions::completeness(&token),
        companions::completeness(&token_2022)
    );

    let store = open().with_builtins();
    let ata = record(&store, crate::ids::ASSOCIATED_TOKEN_PROGRAM_ID).unwrap();
    assert_eq!(ata.name, "associatedToken");
    assert_eq!(ata.host, Some(crate::ids::TOKEN_PROGRAM_ID));
    assert_eq!(ata.origin, IdlOrigin::Builtin);
    for (program_id, name) in [
        (
            crate::wallets::programmatic::MESSAGE_EXECUTOR_PROGRAM_ID,
            "messageExecutor",
        ),
        (
            crate::wallets::programmatic::EXECUTOR_NONCE_PROGRAM_ID,
            "nonce",
        ),
    ] {
        let record = record(&store, program_id).unwrap();
        assert_eq!(record.name, name);
        assert_eq!(
            record.host,
            Some(crate::idl::builtin::ED25519_SIGNER_PROGRAM_ID)
        );
    }
    // Embedded files and the programs they carry are still not exported.
    assert_eq!(store.export_files().unwrap().len(), 1);
}

#[test]
fn each_network_reads_the_embedded_files_of_the_programs_that_run_there() {
    use crate::fixtures::{
        shipped_catalog, ORCA_WHIRLPOOL_PROGRAM_ID, RAYDIUM_CLMM_MAINNET_PROGRAM_ID,
        RAYDIUM_CLMM_PROGRAM_ID, REALMS_GOVERNANCE_PROGRAM_ID,
    };
    let mut store = shipped_catalog(IdlNetwork::Devnet);
    let has = |store: &IdlStore<MemoryStore, MemoryBodies>, id| record(store, id).is_some();
    assert!(has(&store, RAYDIUM_CLMM_PROGRAM_ID));
    assert!(!has(&store, RAYDIUM_CLMM_MAINNET_PROGRAM_ID));
    assert!(has(&store, ORCA_WHIRLPOOL_PROGRAM_ID));
    assert!(has(&store, REALMS_GOVERNANCE_PROGRAM_ID));
    assert!(has(&store, crate::idl::builtin::ED25519_SIGNER_PROGRAM_ID));
    assert!(has(
        &store,
        crate::wallets::programmatic::EXECUTOR_NONCE_PROGRAM_ID
    ));

    store.set_network(IdlNetwork::Mainnet);
    assert_eq!(store.network(), IdlNetwork::Mainnet);
    assert!(!has(&store, RAYDIUM_CLMM_PROGRAM_ID));
    assert!(has(&store, RAYDIUM_CLMM_MAINNET_PROGRAM_ID));
    assert!(has(&store, ORCA_WHIRLPOOL_PROGRAM_ID));
    assert!(has(&store, REALMS_GOVERNANCE_PROGRAM_ID));
    assert!(!has(&store, crate::idl::builtin::ED25519_SIGNER_PROGRAM_ID));
    // Its companions go with it.
    assert!(!has(
        &store,
        crate::wallets::programmatic::EXECUTOR_NONCE_PROGRAM_ID
    ));
    assert!(has(&store, crate::ids::ASSOCIATED_TOKEN_PROGRAM_ID));
    assert!(store
        .program_client(&RAYDIUM_CLMM_MAINNET_PROGRAM_ID)
        .is_some());

    store.set_network(IdlNetwork::Localnet);
    assert!(has(&store, crate::ids::SYSTEM_PROGRAM_ID));
    assert!(!has(&store, ORCA_WHIRLPOOL_PROGRAM_ID));
    assert!(!has(&store, REALMS_GOVERNANCE_PROGRAM_ID));
}

#[test]
fn an_import_is_kept_for_the_network_it_was_made_on() {
    let program = pk(4);
    let on_devnet = codama(program, "0.1.0");
    let on_mainnet = codama(program, "0.2.0");
    let mut store = open();
    store.import(&on_devnet, None).unwrap();

    store.set_network(IdlNetwork::Mainnet);
    assert!(record(&store, program).is_none());
    assert!(store.program_client(&program).is_none());
    store.import(&on_mainnet, None).unwrap();
    assert_eq!(store.body(&program), Some(on_mainnet.as_slice()));

    // Each network keeps its own file across a restart.
    let mut store = IdlStore::open_on(store.index_store, store.bodies, IdlNetwork::Mainnet);
    assert_eq!(store.body(&program), Some(on_mainnet.as_slice()));
    store.set_network(IdlNetwork::Devnet);
    assert_eq!(store.body(&program), Some(on_devnet.as_slice()));

    // Removing one leaves the other.
    store.remove(&program).unwrap();
    store.set_network(IdlNetwork::Mainnet);
    assert_eq!(store.body(&program), Some(on_mainnet.as_slice()));
}

#[test]
fn an_import_for_another_network_is_stored_without_showing() {
    let mut store = open();
    let bytes = codama(pk(4), "0.1.0");
    let saved = store.import_for(IdlNetwork::Mainnet, &bytes, None).unwrap();
    assert_eq!(saved.program_id, pk(4));
    assert!(record(&store, pk(4)).is_none());
    store.set_network(IdlNetwork::Mainnet);
    assert_eq!(store.body(&pk(4)), Some(bytes.as_slice()));
}

#[test]
fn an_index_from_before_networks_moves_under_the_network_in_use() {
    let program = pk(4);
    let bytes = codama(program, "0.1.0");
    let legacy = format!(
        r#"{{"records":[{{"programId":"{program}","name":"myProgram","sha256":"{}","spec":"codama","origin":"programMetadata"}}]}}"#,
        hex::encode(sha256(&bytes))
    );
    let index = MemoryStore::new();
    index.save(legacy.as_bytes()).unwrap();
    let bodies = MemoryBodies::new();
    bodies
        .inner
        .lock()
        .unwrap()
        .insert(program.to_string(), bytes.clone());

    let store = IdlStore::open_on(index, bodies, IdlNetwork::Mainnet);
    assert_eq!(store.body(&program), Some(bytes.as_slice()));
    assert_eq!(
        record(&store, program).unwrap().origin,
        IdlOrigin::ProgramMetadata
    );
    assert_eq!(
        store.bodies.load(IdlNetwork::Mainnet, &program).unwrap(),
        Some(bytes.clone())
    );
    assert_eq!(store.bodies.load_unscoped(&program).unwrap(), None);
    let written: serde_json::Value =
        serde_json::from_slice(&store.index_store.load().unwrap().unwrap()).unwrap();
    assert_eq!(written["records"][0]["network"], "mainnet");

    let mut store = IdlStore::open(store.index_store, store.bodies);
    assert!(record(&store, program).is_none());
    store.set_network(IdlNetwork::Mainnet);
    assert_eq!(store.body(&program), Some(bytes.as_slice()));
}

/// The chain publishes C2, the file was made from C2 and is C1: the file is
/// current, and says so, though the bytes differ.
#[test]
fn a_check_that_finds_what_the_file_was_made_from_says_it_is_current() {
    let mut store = open();
    let ours = codama(pk(1), "0.1.0");
    let theirs = br#"{"name":"theirs","instructions":[]}"#;
    store.import(&ours, None).unwrap();
    store.records[0].source = Some(IdlChainSource {
        network: IdlNetwork::Devnet,
        account: IdlOrigin::ProgramMetadata,
        sha256: sha256(theirs),
    });
    assert_eq!(
        store.records[0].compared_account(),
        IdlOrigin::ProgramMetadata
    );

    let check = store.record_check(&pk(1), theirs).unwrap();
    assert!(!check.same && check.source && check.current());
    assert_eq!(store.body(&pk(1)), Some(ours.as_slice()));

    let check = store.record_check(&pk(1), b"{}").unwrap();
    assert!(!check.same && !check.source && !check.current());
    let check = store.record_check(&pk(1), &ours).unwrap();
    assert!(check.same && !check.source && check.current());
}

#[test]
fn embedded_files_made_from_the_chain_carry_their_source() {
    let store = crate::fixtures::shipped_catalog(IdlNetwork::Devnet);
    let clmm = record(&store, crate::fixtures::RAYDIUM_CLMM_PROGRAM_ID).unwrap();
    let source = clmm.source.unwrap();
    assert_eq!(source.account, IdlOrigin::ProgramMetadata);
    assert_ne!(source.sha256, clmm.sha256);
    assert_eq!(clmm.compared_account(), IdlOrigin::ProgramMetadata);
    // A file taken from a repository has no on-chain source.
    let system = record(&store, crate::ids::SYSTEM_PROGRAM_ID).unwrap();
    assert_eq!(system.source, None);
    assert_eq!(system.compared_account(), IdlOrigin::Anchor);
    // The companions of a file share its source, as they share its checks.
    let nonce = record(
        &store,
        crate::wallets::programmatic::EXECUTOR_NONCE_PROGRAM_ID,
    )
    .unwrap();
    let signer = record(&store, crate::idl::builtin::ED25519_SIGNER_PROGRAM_ID).unwrap();
    assert_eq!(nonce.source, signer.source);
    assert!(signer.source.is_some());
}

#[test]
fn a_bundle_carries_every_network() {
    let mut store = open();
    let on_devnet = codama(pk(4), "0.1.0");
    let on_mainnet = codama(pk(4), "0.2.0");
    store.import(&on_devnet, None).unwrap();
    store
        .import_for(IdlNetwork::Mainnet, &on_mainnet, None)
        .unwrap();
    let files = store.export_files().unwrap();
    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names[0], "index.json");
    assert!(names.contains(&format!("devnet.{}.json", pk(4)).as_str()));
    assert!(names.contains(&format!("mainnet.{}.json", pk(4)).as_str()));

    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let mut other = open();
    let report = other.import_bundle(&borrowed).unwrap();
    assert_eq!(report.imported.len(), 2, "{:?}", report.skipped);
    assert_eq!(other.body(&pk(4)), Some(on_devnet.as_slice()));
    other.set_network(IdlNetwork::Mainnet);
    assert_eq!(other.body(&pk(4)), Some(on_mainnet.as_slice()));
}

#[test]
fn a_bundle_from_before_networks_is_imported_for_the_network_shown() {
    let bytes = codama(pk(4), "0.1.0");
    let index = format!(
        r#"{{"records":[{{"programId":"{}","name":"myProgram","sha256":"{}","spec":"codama","origin":"file"}}]}}"#,
        pk(4),
        hex::encode(sha256(&bytes))
    );
    let name = format!("{}.json", pk(4));
    let mut store = open();
    store.set_network(IdlNetwork::Mainnet);
    let report = store
        .import_bundle(&[("index.json", index.as_bytes()), (&name, &bytes)])
        .unwrap();
    assert_eq!(report.imported.len(), 1, "{:?}", report.skipped);
    assert_eq!(store.body(&pk(4)), Some(bytes.as_slice()));
}

/// Records with a file of their own.
fn files(store: &IdlStore<MemoryStore, MemoryBodies>) -> usize {
    store
        .records()
        .iter()
        .filter(|record| record.host.is_none())
        .count()
}

fn builtins_on(network: IdlNetwork) -> usize {
    BUILTIN_IDLS
        .iter()
        .filter(|idl| idl.runs_on(network))
        .count()
}

/// The IDL pack cannot take a program's slot from the core on a network the
/// core already covers.
#[test]
fn an_embedded_idl_that_claims_a_core_program_is_refused() {
    let mut store = open().with_builtins();
    let system = crate::ids::SYSTEM_PROGRAM_ID;
    let impostor = crate::idl::builtin::EmbeddedIdl {
        program_id: system,
        name: "notSystem".into(),
        json: std::sync::Arc::from(codama(system, "9.9.9")),
        networks: vec![IdlNetwork::Devnet],
        source: crate::idl::builtin::IdlSource::Hand,
        changes: String::new(),
    };
    let elsewhere = crate::idl::builtin::EmbeddedIdl {
        program_id: pk(7),
        name: "myProgram".into(),
        json: std::sync::Arc::from(codama(pk(7), "0.1.0")),
        networks: vec![IdlNetwork::Devnet],
        source: crate::idl::builtin::IdlSource::Hand,
        changes: "made up".into(),
    };
    let refused = store.add_embedded(vec![impostor, elsewhere]);
    assert_eq!(refused, vec!["notSystem".to_string()]);
    assert_eq!(record(&store, system).unwrap().name, "system");
    let added = record(&store, pk(7)).unwrap();
    assert_eq!(added.origin, IdlOrigin::Builtin);
    assert_eq!(store.embedded(&pk(7)).unwrap().changes, "made up");
    assert!(store.remove(&pk(7)).is_err());
    store.set_network(IdlNetwork::Mainnet);
    assert!(record(&store, pk(7)).is_none());
}
