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

/// OtterSec's `update` takes one struct argument, `params`, holding a list of
/// strings. Build asks for each field of the struct, reads the list as JSON,
/// encodes the struct as the program reads it, and an instruction read back
/// opens onto the same fields.
#[test]
fn a_struct_argument_is_one_field_per_member_and_a_list_reads_as_json() {
    let catalog = crate::fixtures::shipped_catalog(crate::IdlNetwork::Mainnet);
    let program = Pubkey::from_str("verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC").unwrap();
    let client = catalog.program_client(&program).unwrap();
    let mut input = IdlBuildInput {
        accounts: vec![
            ("authority".to_string(), Some(pk(1))),
            ("programAddress".to_string(), Some(pk(2))),
        ],
        arguments: vec![
            ("params.version".to_string(), "0.4.11".to_string()),
            (
                "params.gitUrl".to_string(),
                "https://example.com/p".to_string(),
            ),
            ("params.commit".to_string(), "abc".to_string()),
            (
                "params.args".to_string(),
                r#"["--library-name", "p"]"#.to_string(),
            ),
            ("params.deploySlot".to_string(), "77".to_string()),
        ],
    };
    let form = idl_instruction_form(&client, "update", &input).unwrap();
    let fields: Vec<(&str, &IdlArgKind)> = form
        .arguments
        .iter()
        .map(|field| (field.name.as_str(), &field.kind))
        .collect();
    assert_eq!(
        fields,
        [
            ("params.version", &IdlArgKind::String),
            ("params.gitUrl", &IdlArgKind::String),
            ("params.commit", &IdlArgKind::String),
            (
                "params.args",
                &IdlArgKind::List(Box::new(IdlArgKind::String))
            ),
            (
                "params.deploySlot",
                &IdlArgKind::Integer {
                    signed: false,
                    float: false,
                    bits: 64
                }
            ),
        ]
    );
    assert!(form.arguments[3].may_be_blank());

    let built = build_idl_instruction(&client, "update", &input).unwrap();
    // Borsh: each string and the list with a u32 length, the slot as a u64.
    let mut want = hex::decode("dbc858b09e3ffd7f").unwrap();
    for text in ["0.4.11", "https://example.com/p", "abc"] {
        want.extend_from_slice(&(text.len() as u32).to_le_bytes());
        want.extend_from_slice(text.as_bytes());
    }
    want.extend_from_slice(&2u32.to_le_bytes());
    for text in ["--library-name", "p"] {
        want.extend_from_slice(&(text.len() as u32).to_le_bytes());
        want.extend_from_slice(text.as_bytes());
    }
    want.extend_from_slice(&77u64.to_le_bytes());
    assert_eq!(built.data, want);

    let opened = crate::edit::idl_draft_for(&client, "otterVerify", &built).unwrap();
    assert_eq!(opened.argument("params.args"), r#"["--library-name","p"]"#);
    assert_eq!(opened.argument("params.deploySlot"), "77");

    // Blank is the empty list; anything but a JSON list of strings is refused.
    input.arguments[3].1 = String::new();
    let built = build_idl_instruction(&client, "update", &input).unwrap();
    let at = want.len() - 8 - (4 + 14) - (4 + 1) - 4;
    assert_eq!(&built.data[at..at + 4], &0u32.to_le_bytes());
    let opened = crate::edit::idl_draft_for(&client, "otterVerify", &built).unwrap();
    assert_eq!(opened.argument("params.args"), "");
    for bad in ["--library-name p", "[1]", r#"{"a": "b"}"#] {
        input.arguments[3].1 = bad.to_string();
        assert!(
            build_idl_instruction(&client, "update", &input).is_err(),
            "{bad}"
        );
    }
}

/// Token Metadata's `createV1` holds a struct with a list of structs
/// (creators), an optional struct (the collection) and an enum some of whose
/// variants hold a value (the print supply). Each is typed as JSON, or by a
/// variant's name, encodes as the program reads it, and an instruction read
/// back opens onto the same text.
#[test]
fn structs_in_lists_and_options_and_enums_with_fields_read_as_json() {
    let catalog = crate::fixtures::shipped_catalog(crate::IdlNetwork::Mainnet);
    let program = Pubkey::from_str("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s").unwrap();
    let client = catalog.program_client(&program).unwrap();
    let creators = format!(r#"[{{"address":"{}","verified":true,"share":100}}]"#, pk(4));
    let collection = format!(r#"{{"verified":false,"key":"{}"}}"#, pk(5));
    let mut input = IdlBuildInput {
        accounts: [
            "metadata",
            "masterEdition",
            "mint",
            "authority",
            "payer",
            "updateAuthority",
            "systemProgram",
            "sysvarInstructions",
            "splTokenProgram",
        ]
        .iter()
        .enumerate()
        .map(|(at, name)| (name.to_string(), Some(pk(10 + at as u8))))
        .collect(),
        arguments: vec![
            ("assetData.name".to_string(), "Ape".to_string()),
            ("assetData.symbol".to_string(), "APE".to_string()),
            (
                "assetData.uri".to_string(),
                "https://e.x/a.json".to_string(),
            ),
            (
                "assetData.sellerFeeBasisPoints".to_string(),
                "500".to_string(),
            ),
            ("assetData.creators".to_string(), creators.clone()),
            (
                "assetData.primarySaleHappened".to_string(),
                "false".to_string(),
            ),
            ("assetData.isMutable".to_string(), "true".to_string()),
            (
                "assetData.tokenStandard".to_string(),
                "nonFungible".to_string(),
            ),
            ("assetData.collection".to_string(), collection.clone()),
            ("assetData.uses".to_string(), String::new()),
            ("assetData.collectionDetails".to_string(), String::new()),
            ("assetData.ruleSet".to_string(), String::new()),
            ("decimals".to_string(), String::new()),
            ("printSupply".to_string(), "zero".to_string()),
        ],
    };
    let form = idl_instruction_form(&client, "createV1", &input).unwrap();
    let kind = |name: &str| {
        form.arguments
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.kind.clone())
            .unwrap()
    };
    let byte = IdlArgKind::Integer {
        signed: false,
        float: false,
        bits: 8,
    };
    assert_eq!(
        kind("assetData.creators"),
        IdlArgKind::Option(Box::new(IdlArgKind::List(Box::new(IdlArgKind::Struct {
            fields: vec![
                ("address".to_string(), IdlArgKind::Pubkey),
                ("verified".to_string(), IdlArgKind::Bool),
                ("share".to_string(), byte.clone()),
            ]
        }))))
    );
    assert_eq!(
        kind("printSupply"),
        IdlArgKind::Option(Box::new(IdlArgKind::Variants {
            variants: vec![
                ("zero".to_string(), IdlVariant::Empty),
                (
                    "limited".to_string(),
                    IdlVariant::Tuple(vec![IdlArgKind::Integer {
                        signed: false,
                        float: false,
                        bits: 64
                    }])
                ),
                ("unlimited".to_string(), IdlVariant::Empty),
            ]
        }))
    );

    let built = build_idl_instruction(&client, "createV1", &input).unwrap();
    let mut want = vec![42, 0];
    for text in ["Ape", "APE", "https://e.x/a.json"] {
        want.extend_from_slice(&(text.len() as u32).to_le_bytes());
        want.extend_from_slice(text.as_bytes());
    }
    want.extend_from_slice(&500u16.to_le_bytes());
    want.extend_from_slice(&[1, 1, 0, 0, 0]);
    want.extend_from_slice(pk(4).as_ref());
    want.extend_from_slice(&[1, 100]);
    want.extend_from_slice(&[0, 1, 0]); // not sold, mutable, non-fungible
    want.extend_from_slice(&[1, 0]);
    want.extend_from_slice(pk(5).as_ref());
    want.extend_from_slice(&[0, 0, 0]); // no uses, details or rule set
    want.extend_from_slice(&[0, 1, 0]); // no decimals, print supply zero
    assert_eq!(built.data, want);

    let opened = crate::edit::idl_draft_for(&client, "tokenMetadata", &built).unwrap();
    assert_eq!(opened.argument("assetData.creators"), creators);
    assert_eq!(opened.argument("assetData.collection"), collection);
    assert_eq!(opened.argument("printSupply"), "zero");

    // A variant that holds a value, and what is refused.
    let at = input.arguments.len() - 1;
    input.arguments[at].1 = r#"{"limited": 7}"#.to_string();
    let built = build_idl_instruction(&client, "createV1", &input).unwrap();
    assert_eq!(
        &built.data[built.data.len() - 9..],
        &[1, 7, 0, 0, 0, 0, 0, 0, 0][..]
    );
    let opened = crate::edit::idl_draft_for(&client, "tokenMetadata", &built).unwrap();
    assert_eq!(opened.argument("printSupply"), r#"{"limited":7}"#);
    for bad in ["limited", r#"{"limited": [7]}"#, r#"{"other": 1}"#, "{}"] {
        input.arguments[at].1 = bad.to_string();
        assert!(
            build_idl_instruction(&client, "createV1", &input).is_err(),
            "{bad}"
        );
    }
    input.arguments[at].1 = "zero".to_string();
    for bad in [
        format!(r#"[{{"address":"{}","verified":true}}]"#, pk(4)),
        format!(
            r#"[{{"address":"{}","verified":true,"share":100,"x":1}}]"#,
            pk(4)
        ),
        r#"[{"address":"nope","verified":true,"share":100}]"#.to_string(),
        format!(r#"{{"address":"{}","verified":true,"share":100}}"#, pk(4)),
    ] {
        input.arguments[4].1 = bad.clone();
        assert!(
            build_idl_instruction(&client, "createV1", &input).is_err(),
            "{bad}"
        );
    }
}

/// Streamflow keeps a stream's name in `[u8; 64]`: it is typed as text and
/// kept as its bytes followed by zeros, or as hex after `0x`, and blank is
/// all zeros.
#[test]
fn a_fixed_byte_array_reads_as_text_or_hex() {
    assert_eq!(fixed_bytes("Payroll", 8).unwrap(), b"Payroll\0".to_vec());
    assert_eq!(fixed_bytes("0x00ff", 3).unwrap(), vec![0, 255, 0]);
    assert_eq!(fixed_bytes("", 2).unwrap(), vec![0, 0]);
    assert!(fixed_bytes("too long", 4).is_err());
    assert!(fixed_bytes("0xzz", 4).is_err());

    let catalog = crate::fixtures::shipped_catalog(crate::IdlNetwork::Mainnet);
    let program = Pubkey::from_str("strmRqUCoQUgGUan5YhzUZa6KqdzwX5L6FpUxfmKg5m").unwrap();
    let client = catalog.program_client(&program).unwrap();
    let form = idl_instruction_form(&client, "create", &IdlBuildInput::default()).unwrap();
    let name = form
        .arguments
        .iter()
        .find(|field| field.name == "streamName")
        .unwrap();
    assert_eq!(name.kind, IdlArgKind::FixedBytes { len: 64 });
    assert!(name.may_be_blank());
    for (typed, opens) in [("Payroll", "Payroll"), ("0x0001", "0x0001"), ("", "")] {
        let mut draft = crate::idl::draft::IdlDraft::for_instruction(
            program,
            "streamflow",
            &client,
            Some("create"),
        );
        for (at, account) in form.accounts.iter().enumerate() {
            if account.source != IdlAccountSource::Derived {
                draft.set_account(&account.name, Some(pk(20 + at as u8)));
            }
        }
        for field in &form.arguments {
            let text = match field.name.as_str() {
                "streamName" => typed.to_string(),
                _ => match &field.kind {
                    IdlArgKind::Bool => "false".to_string(),
                    IdlArgKind::Option(_) => String::new(),
                    _ => "1".to_string(),
                },
            };
            draft.set_argument(&field.name, text);
        }
        let built = draft.build(&client).unwrap();
        let opened = crate::edit::idl_draft_for(&client, "streamflow", &built).unwrap();
        assert_eq!(opened.argument("streamName"), opens, "{typed}");
    }
}
