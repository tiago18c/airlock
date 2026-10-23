//! Conversion tests against real on-chain IDLs: the SVM governance program
//! (new v01 spec) and Squads v4 (legacy v00 spec).

use codama_nodes::{
    DefaultValueStrategy, DiscriminatorNode, HasName, InstructionInputValueNode, IsSigner,
    NestedTypeNodeTrait, Node, TypeNode, ValueNode,
};
use codama_nodes_from_anchor::{is_anchor_idl, root_node_from_anchor};

const SVMGOV: &str = include_str!("fixtures/svmgov-anchor-v01.json");
const SQUADS: &str = include_str!("fixtures/squads-anchor-v00.json");

#[test]
fn detects_anchor_idls() {
    assert!(is_anchor_idl(&serde_json::from_str(SVMGOV).unwrap()));
    assert!(is_anchor_idl(&serde_json::from_str(SQUADS).unwrap()));
    let codama = serde_json::json!({ "kind": "rootNode", "program": {} });
    assert!(!is_anchor_idl(&codama));
}

#[test]
fn converts_svmgov_v01() {
    let root = root_node_from_anchor(SVMGOV).unwrap();
    let program = &root.program;
    assert_eq!(program.name.as_str(), "svmgovProgram");
    assert_eq!(
        program.public_key,
        "govYkyQ3ePtGULAtY6V75qjWE8UH4vCUVQ1W4HdCAZU"
    );
    assert_eq!(program.instructions.len(), 14);
    assert_eq!(program.accounts.len(), 7);
    assert_eq!(program.events.len(), 11);

    // `update_config` carries its explicit discriminator bytes as an omitted
    // prepended argument plus a field discriminator node.
    let ix = program
        .instructions
        .iter()
        .find(|i| i.name.as_str() == "updateConfig")
        .expect("updateConfig instruction");
    assert_eq!(ix.arguments.len(), 11); // discriminator + 10 option args
    let disc = &ix.arguments[0];
    assert_eq!(disc.name.as_str(), "discriminator");
    assert_eq!(
        disc.default_value_strategy,
        Some(DefaultValueStrategy::Omitted)
    );
    match disc.default_value.as_ref().as_ref() {
        Some(InstructionInputValueNode::BytesValue(bytes)) => {
            assert_eq!(bytes.data, "1d9efcbf0a53db63");
        }
        other => panic!("expected bytes default, got {other:?}"),
    }
    assert!(matches!(ix.discriminators[0], DiscriminatorNode::Field(_)));
    assert!(matches!(
        ix.arguments[1].r#type.as_ref(),
        TypeNode::Option(_)
    ));
    assert_eq!(ix.accounts.len(), 3);
    assert_eq!(ix.accounts[0].name.as_str(), "admin");
    assert_eq!(ix.accounts[0].is_signer, IsSigner::True);
    // `global_config` gets an inline PDA default from its const seed.
    assert!(ix.accounts[1].default_value.is_some());
    // `system_program` has a fixed address default.
    assert!(matches!(
        ix.accounts[2].default_value.as_ref().as_ref(),
        Some(InstructionInputValueNode::PublicKeyValue(_))
    ));

    // Accounts get the discriminator field prepended.
    let config = program
        .accounts
        .iter()
        .find(|a| a.name.as_str() == "globalConfig")
        .expect("globalConfig account");
    let data = config.data.get_nested_type_node();
    assert_eq!(data.fields[0].name.as_str(), "discriminator");
}

#[test]
fn converts_squads_v00() {
    let root = root_node_from_anchor(SQUADS).unwrap();
    let program = &root.program;
    assert_eq!(program.name.as_str(), "squadsMultisigProgram");
    assert_eq!(program.instructions.len(), 36);
    assert_eq!(program.accounts.len(), 9);

    // v00 discriminators are computed: sha256("account:VaultTransaction")[..8].
    let vault_tx = program
        .accounts
        .iter()
        .find(|a| a.name.as_str() == "vaultTransaction")
        .expect("vaultTransaction account");
    let data = vault_tx.data.get_nested_type_node();
    assert_eq!(data.fields[0].name.as_str(), "discriminator");
    match data.fields[0].default_value.as_ref().as_ref() {
        Some(ValueNode::Bytes(bytes)) => assert_eq!(bytes.data, "a8faa264510ea2cf"),
        other => panic!("expected bytes default, got {other:?}"),
    }

    // Defined types resolve as links: VaultTransactionMessage is referenced.
    assert!(program
        .defined_types
        .iter()
        .any(|t| t.name.as_str() == "vaultTransactionMessage"));

    // Instruction discriminator computed from the global: preimage.
    let ix = program
        .instructions
        .iter()
        .find(|i| i.name.as_str() == "vaultTransactionCreate")
        .expect("vaultTransactionCreate instruction");
    assert_eq!(ix.arguments[0].name.as_str(), "discriminator");
}

#[test]
fn converted_roots_serialize_as_codama_json() {
    // The converted tree must round-trip through the Codama JSON wire format.
    for idl in [SVMGOV, SQUADS] {
        let root = root_node_from_anchor(idl).unwrap();
        let json = serde_json::to_string(&root).unwrap();
        let reparsed: codama_nodes::RootNode = serde_json::from_str(&json).unwrap();
        assert_eq!(root, reparsed);
    }
}

#[test]
fn node_names_are_reachable() {
    // Sanity: converted nodes participate in the Node union like hand-written ones.
    let root = root_node_from_anchor(SVMGOV).unwrap();
    let node: Node = root.program.instructions[0].clone().into();
    assert!(matches!(node, Node::Instruction(_)));
    let _ = root.program.instructions[0].name();
}
