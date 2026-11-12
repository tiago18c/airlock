//! IDLs a program's author can publish. The app reads one from the chain as
//! soon as a transaction names the program, so what these do has to end with
//! an instruction left unread. Each runs on a stack the size of a main
//! thread's, where the window explains a transaction.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use wallet_core::{
    compile_v1, decode_v1_transaction, explain_transaction, idl_instruction_form, IdlArgKind,
    IdlBuildInput, IdlOrigin, IdlStore, KeypairSigner, MemoryBodies, WalletSigner,
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

/// An Anchor file of the 0.30 spec with `types`, and one instruction tagged
/// `1..=8` whose argument is `argument`.
fn anchor_generics(types: &[String], argument: &str) -> String {
    format!(
        r#"{{"address":"{PROGRAM}","metadata":{{"name":"evil","version":"0.1.0","spec":"0.1.0"}},
          "instructions":[{{"name":"boom","discriminator":[1,2,3,4,5,6,7,8],"accounts":[],
            "args":[{{"name":"x","type":{argument}}}]}}],
          "types":[{}]}}"#,
        types.join(",")
    )
}

/// A generic struct `name<T>` of the 0.30 spec, with `fields` as
/// `(name, type)`.
fn template(name: &str, fields: &[(&str, String)]) -> String {
    let fields: Vec<String> = fields
        .iter()
        .map(|(name, ty)| format!(r#"{{"name":"{name}","type":{ty}}}"#))
        .collect();
    format!(
        r#"{{"name":"{name}","generics":[{{"kind":"type","name":"T"}}],
          "type":{{"kind":"struct","fields":[{}]}}}}"#,
        fields.join(",")
    )
}

/// The generic `name`, used with the type `argument`.
fn used(name: &str, argument: &str) -> String {
    format!(r#"{{"defined":{{"name":"{name}","generics":[{{"kind":"type","type":{argument}}}]}}}}"#)
}

const T: &str = r#"{"generic":"T"}"#;

/// [`reads`], for an Anchor file with generic `types` and `data` after the
/// instruction's tag.
fn reads_generics(types: &[String], argument: &str, data: &[u8]) -> bool {
    let mut tagged = vec![1, 2, 3, 4, 5, 6, 7, 8];
    tagged.extend_from_slice(data);
    reads(anchor_generics(types, argument), tagged)
}

/// A generic that hands its own parameter to another, under the same name,
/// as a program's price types do.
#[test]
fn an_anchor_generic_that_passes_its_parameter_on_is_read() {
    let types = [
        template("Price", &[("min", T.into()), ("max", T.into())]),
        template("Prices", &[("index", used("Price", T))]),
    ];
    assert!(reads_generics(&types, &used("Prices", r#""u8""#), &[1, 2]));
}

#[test]
fn an_anchor_generic_that_holds_itself_is_left_unread() {
    let own = format!(r#"{{"option":{}}}"#, used("Node", T));
    let types = [template("Node", &[("value", T.into()), ("next", own)])];
    assert!(!reads_generics(&types, &used("Node", r#""u8""#), &[1, 0]));

    let types = [
        template("Ping", &[("pong", used("Pong", T))]),
        template("Pong", &[("ping", used("Ping", T))]),
    ];
    assert!(!reads_generics(&types, &used("Ping", r#""u8""#), &[1]));
}

/// A few lines that expand to a tree of 2^14 structs, or hand on an argument
/// of 2^15 parts.
#[test]
fn anchor_generics_that_double_at_each_level_are_left_unread() {
    let level = |at: usize| format!("Level{at}");
    let mut wide: Vec<String> = (0..14)
        .map(|at| {
            let next = level(at + 1);
            template(&level(at), &[("a", used(&next, T)), ("b", used(&next, T))])
        })
        .collect();
    wide.push(template(&level(14), &[("v", T.into())]));
    assert!(!reads_generics(&wide, &used("Level0", r#""u8""#), &[0; 64]));

    let pair = format!(r#"{{"array":[{T},2]}}"#);
    let pairs = (0..3).fold(pair, |inner, _| format!(r#"{{"array":[{inner},2]}}"#));
    let mut heavy: Vec<String> = (0..15)
        .map(|at| template(&level(at), &[("next", used(&level(at + 1), &pairs))]))
        .collect();
    heavy.push(template(&level(15), &[("v", T.into())]));
    assert!(!reads_generics(
        &heavy,
        &used("Level0", r#""u8""#),
        &[0; 64]
    ));
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

/// Build's form follows a link a link or two deep to a plain enum, and no
/// further: types that link to themselves, or to each other, are a field
/// nobody can fill.
#[test]
fn builds_form_reads_a_type_that_links_to_itself_as_a_field_nobody_can_fill() {
    let plain = format!(
        r#"{{"kind":"enumTypeNode","variants":[{{"kind":"enumEmptyVariantTypeNode","name":"on"}}],"size":{U8}}}"#
    );
    let types = [
        defined("loop", &link("loop")),
        defined("ping", &link("pong")),
        defined("pong", &link("ping")),
        defined("switch", &plain),
    ];
    let instructions = [
        instruction("a", 1, U8, &link("loop")),
        instruction("b", 2, U8, &link("ping")),
        instruction("c", 3, U8, &link("switch")),
    ];
    let file = idl(&types, &instructions);
    let kinds = on_a_main_threads_stack(move || {
        let mut store = IdlStore::open(MemoryStore::new(), MemoryBodies::new());
        store
            .import_with_origin(file.as_bytes(), None, IdlOrigin::ProgramMetadata)
            .unwrap();
        let client = store.program_client(&PROGRAM.parse().unwrap()).unwrap();
        ["a", "b", "c"].map(|name| {
            idl_instruction_form(&client, name, &IdlBuildInput::default())
                .unwrap()
                .arguments[0]
                .kind
                .clone()
        })
    });
    let unread = IdlArgKind::Raw {
        note: "definedTypeLinkNode".into(),
    };
    assert_eq!(kinds[0], unread);
    assert_eq!(kinds[1], unread);
    assert_eq!(
        kinds[2],
        IdlArgKind::Enum {
            variants: vec!["on".into()]
        }
    );
}
