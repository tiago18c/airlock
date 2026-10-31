//! IDLs a program's author can publish. The app reads one from the chain as
//! soon as a transaction names the program, so what these do has to end with
//! an instruction left unread. Each runs on a stack the size of a main
//! thread's, where the window explains a transaction.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use wallet_core::{
    compile_v1, decode_v1_transaction, explain_transaction, IdlOrigin, IdlStore, KeypairSigner,
    MemoryBodies, WalletSigner,
};
use wallet_vault::store::MemoryStore;

const PROGRAM: &str = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";
const U8: &str = r#"{"kind":"numberTypeNode","format":"u8","endian":"le"}"#;
const U32: &str = r#"{"kind":"numberTypeNode","format":"u32","endian":"le"}"#;
const EMPTY: &str = r#"{"kind":"structTypeNode","fields":[]}"#;

fn on_a_main_threads_stack<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap()
}

fn link(name: &str) -> String {
    format!(r#"{{"kind":"definedTypeLinkNode","name":"{name}"}}"#)
}

fn defined(name: &str, ty: &str) -> String {
    format!(r#"{{"kind":"definedTypeNode","name":"{name}","type":{ty}}}"#)
}

fn list(item: &str, count: &str) -> String {
    format!(r#"{{"kind":"arrayTypeNode","item":{item},"count":{count}}}"#)
}

fn fixed(count: u64) -> String {
    format!(r#"{{"kind":"fixedCountNode","value":{count}}}"#)
}

fn prefixed() -> String {
    format!(r#"{{"kind":"prefixedCountNode","prefix":{U32}}}"#)
}

fn remainder() -> String {
    r#"{"kind":"remainderCountNode"}"#.to_owned()
}

/// One instruction, tagged `tag`, with one argument of type `argument`.
fn instruction(name: &str, tag: u8, tag_type: &str, argument: &str) -> String {
    format!(
        r#"{{"kind":"instructionNode","name":"{name}",
          "accounts":[{{"kind":"instructionAccountNode","name":"who","isWritable":true,"isSigner":true}}],
          "arguments":[
            {{"kind":"instructionArgumentNode","name":"discriminator","defaultValueStrategy":"omitted",
              "type":{tag_type},"defaultValue":{{"kind":"numberValueNode","number":{tag}}}}},
            {{"kind":"instructionArgumentNode","name":"x","type":{argument}}}],
          "discriminators":[{{"kind":"fieldDiscriminatorNode","name":"discriminator","offset":0}}]}}"#
    )
}

fn idl(types: &[String], instructions: &[String]) -> String {
    format!(
        r#"{{"kind":"rootNode","standard":"codama","version":"1.8.0",
          "program":{{"kind":"programNode","name":"evil","publicKey":"{PROGRAM}","version":"0.0.0",
            "accounts":[],"pdas":[],"errors":[],
            "definedTypes":[{}],"instructions":[{}]}},
          "additionalPrograms":[]}}"#,
        types.join(","),
        instructions.join(",")
    )
}

/// Whether the catalog can read an instruction of `file`, with `data`.
fn reads(file: String, data: Vec<u8>) -> bool {
    on_a_main_threads_stack(move || {
        let mut store = IdlStore::open(MemoryStore::new(), MemoryBodies::new());
        if store
            .import_with_origin(file.as_bytes(), None, IdlOrigin::ProgramMetadata)
            .is_err()
        {
            return false;
        }
        let payer = KeypairSigner::new_random();
        let program: Pubkey = PROGRAM.parse().unwrap();
        let wire = compile_v1(
            payer.pubkey(),
            vec![Instruction {
                program_id: program,
                accounts: vec![AccountMeta::new(payer.pubkey(), true)],
                data,
            }],
            None,
            Some(solana_hash::Hash::new_from_array([1; 32])),
        )
        .unwrap()
        .wire;
        let tx = decode_v1_transaction(&wire).unwrap();
        explain_transaction(&store, &tx).instructions[0]
            .idl
            .is_some()
    })
}

/// [`reads`], for one instruction whose argument is `argument`.
fn reads_argument(types: &[String], argument: &str, data: &[u8]) -> bool {
    let file = idl(types, &[instruction("boom", 7, U8, argument)]);
    let mut tagged = vec![7];
    tagged.extend_from_slice(data);
    reads(file, tagged)
}

#[test]
fn a_fair_type_is_read() {
    let types = [defined("fair", U8)];
    assert!(reads_argument(&types, &link("fair"), &[1]));
    assert!(reads_argument(
        &types,
        &list(&link("fair"), &fixed(2)),
        &[1, 2]
    ));
    assert!(reads_argument(&types, &list(EMPTY, &fixed(3)), &[]));
}

#[test]
fn a_type_that_links_to_itself_is_left_unread() {
    let types = [defined("loop", &link("loop"))];
    assert!(!reads_argument(&types, &link("loop"), &[1, 2, 3]));
}

#[test]
fn types_that_link_to_each_other_are_left_unread() {
    let types = [
        defined("ping", &link("pong")),
        defined("pong", &link("ping")),
    ];
    assert!(!reads_argument(&types, &link("ping"), &[1, 2, 3]));
}

#[test]
fn a_type_that_holds_a_list_of_itself_is_left_unread() {
    let types = [defined("tree", &list(&link("tree"), &prefixed()))];
    assert!(!reads_argument(&types, &link("tree"), &[0, 0, 0, 0]));
}

#[test]
fn a_chain_of_links_past_the_limit_is_left_unread() {
    let last = 200;
    let mut types: Vec<String> = (0..last)
        .map(|at| defined(&format!("t{at}"), &link(&format!("t{}", at + 1))))
        .collect();
    types.push(defined(&format!("t{last}"), U8));
    assert!(!reads_argument(&types, &link("t0"), &[1]));
}

#[test]
fn lists_through_links_up_to_the_limit_are_read_decoded_and_shown() {
    let chain = |levels: usize| -> Vec<String> {
        let mut types: Vec<String> = (0..levels)
            .map(|at| {
                let next = link(&format!("t{}", at + 1));
                defined(&format!("t{at}"), &list(&next, &fixed(1)))
            })
            .collect();
        types.push(defined(&format!("t{levels}"), U8));
        types
    };
    // A link and a list for each level, and the arguments' struct, the link
    // in and the byte itself.
    assert!(reads_argument(&chain(30), &link("t0"), &[1]));
    assert!(!reads_argument(&chain(31), &link("t0"), &[1]));
}

#[test]
fn types_that_double_at_each_level_are_left_unread() {
    let levels = 60;
    let mut types: Vec<String> = (0..levels)
        .map(|at| {
            let next = link(&format!("t{}", at + 1));
            let fields = format!(
                r#"{{"kind":"structTypeNode","fields":[
                  {{"kind":"structFieldTypeNode","name":"a","type":{next}}},
                  {{"kind":"structFieldTypeNode","name":"b","type":{next}}}]}}"#
            );
            defined(&format!("t{at}"), &fields)
        })
        .collect();
    types.push(defined(&format!("t{levels}"), U8));
    assert!(!reads_argument(&types, &link("t0"), &[1, 2, 3]));
}

#[test]
fn a_list_of_empty_items_is_left_unread() {
    let many = [0xff, 0xff, 0xff, 0xff];
    for argument in [
        list(EMPTY, &fixed(4_000_000_000)),
        list(EMPTY, &prefixed()),
        list(EMPTY, &remainder()),
        // 3^40 in all, and no one list long.
        (0..40).fold(list(EMPTY, &fixed(3)), |inner, _| list(&inner, &fixed(3))),
    ] {
        assert!(!reads_argument(&[], &argument, &many));
    }
}

/// `levels` lists of one, around a byte.
fn nested_lists(levels: usize) -> String {
    (0..levels).fold(U8.to_owned(), |inner, _| list(&inner, &fixed(1)))
}

#[test]
fn nesting_up_to_the_limit_is_read_and_past_it_is_refused() {
    // The file's own levels, and the count of the innermost list, are 7.
    assert!(reads_argument(&[], &nested_lists(25), &[1]));
    assert!(!reads_argument(&[], &nested_lists(26), &[1]));
    assert!(!reads_argument(&[], &nested_lists(120), &[1]));
}

/// An Anchor file with one instruction whose argument is `levels` vectors,
/// around a byte.
fn anchor_vectors(levels: usize) -> String {
    let deep = (0..levels).fold(r#""u8""#.to_owned(), |inner, _| {
        format!(r#"{{"vec":{inner}}}"#)
    });
    format!(
        r#"{{"version":"0.1.0","name":"evil","metadata":{{"address":"{PROGRAM}"}},
          "instructions":[{{"name":"boom","accounts":[],"args":[{{"name":"x","type":{deep}}}]}}]}}"#
    )
}

#[test]
fn an_anchor_idl_is_held_to_the_same_limit() {
    // An eight byte tag, then an empty vector.
    let tag = wallet_core::sha256_bytes(b"global:boom");
    let data: Vec<u8> = tag[..8].iter().chain(&[0; 4]).copied().collect();
    assert!(reads(anchor_vectors(27), data.clone()));
    assert!(!reads(anchor_vectors(120), data));
}

#[test]
fn discriminators_that_each_copy_the_tree_stop_at_a_limit() {
    let types = [defined("tag", U8)];
    let instructions: Vec<String> = (0..200u8)
        .map(|at| instruction(&format!("i{at}"), at, &link("tag"), U8))
        .collect();
    let file = || idl(&types, &instructions);
    assert!(reads(file(), vec![3, 1]), "found before the limit");
    assert!(!reads(file(), vec![199, 1]), "past the limit");
}
