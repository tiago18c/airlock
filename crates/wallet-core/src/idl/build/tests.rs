use super::*;
use codama_nodes::{
    AccountBumpValueNode, AccountFieldValueNode, AccountValueNode, FieldDiscriminatorNode,
    InstructionArgumentNode, InstructionNode, NumberTypeNode, NumberValueNode, PdaNode,
    PdaSeedValueNode, PdaValueNode, PdaValuePda, ProgramNode, PublicKeyTypeNode, RootNode,
    VariablePdaSeedNode, U64, U8,
};

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn disc(value: u8) -> InstructionArgumentNode {
    let mut argument = InstructionArgumentNode::new("discriminator", NumberTypeNode::le(U8));
    argument.default_value_strategy = Some(DefaultValueStrategy::Omitted);
    argument.default_value = Box::new(Some(NumberValueNode::new(value).into()));
    argument
}

fn with_disc(node: InstructionNode, argument: &str) -> InstructionNode {
    let mut node = node;
    node.discriminators = vec![FieldDiscriminatorNode::new(argument, 0).into()];
    node
}

fn sample(program: &Pubkey) -> RootNode {
    let transfer = with_disc(
        InstructionNode {
            name: "transfer".into(),
            accounts: vec![
                InstructionAccountNode::new("source", true, true),
                InstructionAccountNode::new("destination", true, false),
            ],
            arguments: vec![
                disc(1),
                InstructionArgumentNode::new("amount", NumberTypeNode::le(U64)),
            ],
            ..Default::default()
        },
        "discriminator",
    );
    let echo = with_disc(
        InstructionNode {
            name: "echo".into(),
            accounts: vec![
                InstructionAccountNode::new("source", true, true),
                InstructionAccountNode::new("destination", true, false),
            ],
            arguments: vec![
                disc(1),
                InstructionArgumentNode::new("amount", NumberTypeNode::le(U64)),
            ],
            ..Default::default()
        },
        "discriminator",
    );

    let pda = PdaNode::new(
        "vaultPda",
        vec![VariablePdaSeedNode::new("authority", PublicKeyTypeNode::new()).into()],
    );
    let pda_value = PdaValueNode::new(
        PdaValuePda::Pda(pda),
        vec![PdaSeedValueNode::new(
            "authority",
            AccountValueNode::new("authority"),
        )],
    );
    let mut vault = InstructionAccountNode::new("vault", false, false);
    vault.default_value = Box::new(Some(pda_value.into()));
    let mut bump = InstructionArgumentNode::new("bump", NumberTypeNode::le(U8));
    bump.default_value_strategy = Some(DefaultValueStrategy::Omitted);
    bump.default_value = Box::new(Some(AccountBumpValueNode::new("vault").into()));
    let lock = with_disc(
        InstructionNode {
            name: "lock".into(),
            accounts: vec![InstructionAccountNode::new("authority", false, true), vault],
            arguments: vec![disc(2), bump],
            ..Default::default()
        },
        "discriminator",
    );

    let mut broken_pda = PdaNode::new(
        "brokenPda",
        vec![VariablePdaSeedNode::new("authority", PublicKeyTypeNode::new()).into()],
    );
    broken_pda.program_id = Some("not-a-pubkey".into());
    let broken_value = PdaValueNode::new(
        PdaValuePda::Pda(broken_pda),
        vec![PdaSeedValueNode::new(
            "authority",
            AccountValueNode::new("authority"),
        )],
    );
    let mut broken_vault = InstructionAccountNode::new("vault", false, false);
    broken_vault.default_value = Box::new(Some(broken_value.into()));
    let broken = with_disc(
        InstructionNode {
            name: "broken".into(),
            accounts: vec![
                InstructionAccountNode::new("authority", false, true),
                broken_vault,
            ],
            arguments: vec![disc(3)],
            ..Default::default()
        },
        "discriminator",
    );

    let mut mint = InstructionAccountNode::new("mint", false, false);
    mint.default_value = Box::new(Some(InstructionInputValueNode::AccountFieldValue(
        AccountFieldValueNode {
            account: "mint".into(),
            ..Default::default()
        },
    )));
    let peek = with_disc(
        InstructionNode {
            name: "peek".into(),
            accounts: vec![InstructionAccountNode::new("payer", true, true), mint],
            arguments: vec![disc(4)],
            ..Default::default()
        },
        "discriminator",
    );

    let program_node = ProgramNode::new("builder", program.to_string())
        .add_instruction(transfer)
        .add_instruction(echo)
        .add_instruction(lock)
        .add_instruction(broken)
        .add_instruction(peek);
    RootNode::new(program_node)
}

fn client(program: &Pubkey) -> ProgramClient {
    ProgramClient::from_root(sample(program)).unwrap()
}

#[test]
fn fixture_file_round_trips_transfer() {
    let client =
        ProgramClient::from_json(include_str!("../../../tests/fixtures/builder-program.json"))
            .unwrap();
    let input = IdlBuildInput {
        accounts: vec![
            ("source".into(), Some(pk(2))),
            ("destination".into(), Some(pk(3))),
        ],
        arguments: vec![("amount".into(), "1000".into())],
    };
    let ix = build_idl_instruction(&client, "transfer", &input).unwrap();
    assert_eq!(ix.program_id, pk(1));
    assert_eq!(ix.data[0], 1);
}

#[test]
fn transfer_form_hides_omitted_discriminator_and_round_trips() {
    let program = pk(1);
    let client = client(&program);
    let form = idl_instruction_form(&client, "transfer", &IdlBuildInput::default()).unwrap();
    assert_eq!(
        form.arguments
            .iter()
            .map(|arg| arg.name.as_str())
            .collect::<Vec<_>>(),
        vec!["amount"]
    );
    assert!(form
        .accounts
        .iter()
        .all(|account| account.source == IdlAccountSource::Picker));

    let input = IdlBuildInput {
        accounts: vec![
            ("source".into(), Some(pk(2))),
            ("destination".into(), Some(pk(3))),
        ],
        arguments: vec![("amount".into(), "1000".into())],
    };
    let ix = build_idl_instruction(&client, "transfer", &input).unwrap();
    assert_eq!(ix.program_id, program);
    assert_eq!(ix.accounts[0].pubkey, pk(2));
    assert_eq!(ix.accounts[1].pubkey, pk(3));
    let mut expected = vec![1u8];
    expected.extend_from_slice(&1000u64.to_le_bytes());
    assert_eq!(ix.data, expected);
}

#[test]
fn lock_derives_the_pda_and_omits_the_bump_field() {
    let program = pk(9);
    let client = client(&program);
    let authority = pk(7);
    let input = IdlBuildInput {
        accounts: vec![("authority".into(), Some(authority))],
        arguments: vec![],
    };
    let form = idl_instruction_form(&client, "lock", &input).unwrap();
    assert!(form.arguments.is_empty());
    let vault = form
        .accounts
        .iter()
        .find(|account| account.name == "vault")
        .unwrap();
    assert_eq!(vault.source, IdlAccountSource::Derived);
    assert_eq!(vault.seeds, vec!["authority".to_string()]);
    let (expected, bump) = Pubkey::find_program_address(&[authority.as_ref()], &program);
    assert_eq!(vault.address, Some(expected));

    let ix = build_idl_instruction(&client, "lock", &input).unwrap();
    assert_eq!(ix.accounts[1].pubkey, expected);
    assert!(!ix.accounts[1].is_signer && !ix.accounts[1].is_writable);
    assert_eq!(ix.data, vec![2, bump]);
}

#[test]
fn same_bytes_that_parse_as_another_instruction_are_refused() {
    let client = client(&pk(1));
    let input = IdlBuildInput {
        accounts: vec![
            ("source".into(), Some(pk(2))),
            ("destination".into(), Some(pk(3))),
        ],
        arguments: vec![("amount".into(), "1000".into())],
    };
    let err = build_idl_instruction(&client, "echo", &input).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("transfer"), "{message}");
    assert!(message.contains("Refusing"), "{message}");
}

#[test]
fn undervivable_pda_asks_for_the_address_and_accepts_it() {
    let program = pk(4);
    let client = client(&program);
    let open = IdlBuildInput {
        accounts: vec![("authority".into(), Some(pk(8)))],
        arguments: vec![],
    };
    let form = idl_instruction_form(&client, "broken", &open).unwrap();
    let vault = form
        .accounts
        .iter()
        .find(|account| account.name == "vault")
        .unwrap();
    assert_eq!(vault.source, IdlAccountSource::Fill);
    assert!(vault.reason.contains("program id"), "{}", vault.reason);

    let err = build_idl_instruction(&client, "broken", &open).unwrap_err();
    assert!(matches!(
        err,
        IdlBuildFailure::FillAddress { ref account, .. } if account == "vault"
    ));

    let filled = IdlBuildInput {
        accounts: vec![
            ("authority".into(), Some(pk(8))),
            ("vault".into(), Some(pk(5))),
        ],
        arguments: vec![],
    };
    let ix = build_idl_instruction(&client, "broken", &filled).unwrap();
    assert_eq!(ix.accounts[1].pubkey, pk(5));
    assert_eq!(ix.program_id, program);
}

#[test]
fn account_field_default_stays_manual() {
    let client = client(&pk(1));
    let form = idl_instruction_form(&client, "peek", &IdlBuildInput::default()).unwrap();
    let mint = form
        .accounts
        .iter()
        .find(|account| account.name == "mint")
        .unwrap();
    assert_eq!(mint.source, IdlAccountSource::Fill);
    assert!(mint.reason.contains("account data"), "{}", mint.reason);

    let err = build_idl_instruction(&client, "peek", &IdlBuildInput::default()).unwrap_err();
    assert!(matches!(err, IdlBuildFailure::Message(ref message) if message.contains("payer")));

    let input = IdlBuildInput {
        accounts: vec![("payer".into(), Some(pk(2)))],
        arguments: vec![],
    };
    let err = build_idl_instruction(&client, "peek", &input).unwrap_err();
    assert!(matches!(
        err,
        IdlBuildFailure::FillAddress { ref account, .. } if account == "mint"
    ));

    let input = IdlBuildInput {
        accounts: vec![("payer".into(), Some(pk(2))), ("mint".into(), Some(pk(3)))],
        arguments: vec![],
    };
    let ix = build_idl_instruction(&client, "peek", &input).unwrap();
    assert_eq!(ix.data, vec![4]);
    assert_eq!(ix.accounts[1].pubkey, pk(3));
}

/// A seed of each literal kind and one typed into an argument. The form shows
/// the address the instruction is then built with.
#[test]
fn every_seed_kind_derives_the_address_the_build_uses() {
    use codama_nodes::{
        ArgumentValueNode, BooleanTypeNode, BooleanValueNode, ConstantPdaSeedNode, StringTypeNode,
        StringValueNode, U16,
    };

    let program = pk(6);
    let pda = PdaNode::new(
        "slotPda",
        vec![
            ConstantPdaSeedNode::new(StringTypeNode::utf8(), StringValueNode::new("slot")).into(),
            VariablePdaSeedNode::new("index", NumberTypeNode::le(U64)).into(),
            VariablePdaSeedNode::new("width", NumberTypeNode::be(U16)).into(),
            VariablePdaSeedNode::new("open", BooleanTypeNode::default()).into(),
        ],
    );
    let value = PdaValueNode::new(
        PdaValuePda::Pda(pda),
        vec![
            PdaSeedValueNode::new("index", ArgumentValueNode::new("index")),
            PdaSeedValueNode::new("width", NumberValueNode::new(258u16)),
            PdaSeedValueNode::new("open", BooleanValueNode::new(true)),
        ],
    );
    let mut slot = InstructionAccountNode::new("slot", false, false);
    slot.default_value = Box::new(Some(value.into()));
    let open = with_disc(
        InstructionNode {
            name: "open".into(),
            accounts: vec![slot],
            arguments: vec![
                disc(5),
                InstructionArgumentNode::new("index", NumberTypeNode::le(U64)),
            ],
            ..Default::default()
        },
        "discriminator",
    );
    let root = RootNode::new(ProgramNode::new("seeds", program.to_string()).add_instruction(open));
    let client = ProgramClient::from_root(root).unwrap();

    let blank = idl_instruction_form(&client, "open", &IdlBuildInput::default()).unwrap();
    assert_eq!(blank.accounts[0].source, IdlAccountSource::Derived);
    assert_eq!(blank.accounts[0].address, None);

    let input = IdlBuildInput {
        accounts: vec![],
        arguments: vec![("index".into(), "42".into())],
    };
    let form = idl_instruction_form(&client, "open", &input).unwrap();
    let (expected, _) = Pubkey::find_program_address(
        &[b"slot", &42u64.to_le_bytes(), &258u16.to_be_bytes(), &[1]],
        &program,
    );
    assert_eq!(form.accounts[0].address, Some(expected));
    let ix = build_idl_instruction(&client, "open", &input).unwrap();
    assert_eq!(ix.accounts[0].pubkey, expected);
}
