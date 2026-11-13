//! Scaled fields: the unit each core field is typed in, exact reading, a
//! scale taken from another argument, and the text a built instruction opens
//! with on Build. Folded fields: what Build puts under "Filled in for you".

use super::*;
use crate::edit::idl_draft_for;
use crate::fixtures::{pk, token_transfer_checked, transfer};
use crate::idl::builtin::BUILTIN_IDLS;
use crate::idl::draft::IdlDraft;
use codama_nodes::{
    AmountNumberDisplayNode, AmountTypeNode, FieldDiscriminatorNode, NumberDisplayNode,
    NumberTypeNode, NumberValueNode, OptionTypeNode, ProgramNode, SolAmountTypeNode,
    StringValueNode, U16, U64, U8,
};

fn core_client(name: &str) -> ProgramClient {
    let idl = BUILTIN_IDLS
        .iter()
        .find(|idl| idl.name == name)
        .unwrap_or_else(|| panic!("no core IDL {name}"));
    if idl.json.contains("\"rootNode\"") {
        return ProgramClient::from_json(idl.json).unwrap();
    }
    let root = codama_nodes_from_anchor::root_node_from_anchor(idl.json).unwrap();
    ProgramClient::from_root(root).unwrap()
}

fn scale_of(client: &ProgramClient, instruction: &str, argument: &str) -> IdlArgScale {
    let form = idl_instruction_form(client, instruction, &IdlBuildInput::default()).unwrap();
    form.arguments
        .into_iter()
        .find(|field| field.name == argument)
        .unwrap_or_else(|| panic!("no argument {instruction}.{argument}"))
        .scale
}

fn fixed(decimals: u8, unit: &str) -> IdlArgScale {
    IdlArgScale::Fixed {
        decimals,
        unit: Some(unit.to_string()),
    }
}

#[test]
fn core_fields_are_typed_in_the_unit_their_display_names() {
    let system = core_client("system");
    assert_eq!(scale_of(&system, "transferSol", "amount"), fixed(9, "SOL"));
    assert_eq!(
        scale_of(&system, "createAccount", "lamports"),
        fixed(9, "SOL")
    );
    assert_eq!(
        scale_of(&system, "createAccount", "space"),
        fixed(0, "bytes")
    );
    // The owner is an address, never scaled.
    assert_eq!(
        scale_of(&system, "createAccount", "programAddress"),
        IdlArgScale::None
    );

    for name in ["token", "token2022"] {
        let token = core_client(name);
        assert_eq!(
            scale_of(&token, "transferChecked", "amount"),
            IdlArgScale::FromArgument {
                argument: "decimals".to_string(),
                unit: None,
            },
            "{name}"
        );
        // Its decimals live in the mint account: base units in this form.
        assert_eq!(
            scale_of(&token, "mintTo", "amount"),
            IdlArgScale::None,
            "{name}"
        );
        assert_eq!(
            scale_of(&token, "transfer", "amount"),
            fixed(0, "base units"),
            "{name}"
        );
        assert_eq!(
            scale_of(&token, "transferChecked", "decimals"),
            IdlArgScale::None
        );
    }

    let token_2022 = core_client("token2022");
    assert_eq!(
        scale_of(
            &token_2022,
            "initializeTransferFeeConfig",
            "transferFeeBasisPoints"
        ),
        fixed(2, "%")
    );
}

/// A program of one instruction, `set`, whose arguments are `fields`.
fn sample(fields: Vec<InstructionArgumentNode>) -> ProgramClient {
    let mut discriminator = InstructionArgumentNode::new("discriminator", NumberTypeNode::le(U8));
    discriminator.default_value_strategy = Some(DefaultValueStrategy::Omitted);
    discriminator.default_value = Box::new(Some(NumberValueNode::new(7u8).into()));
    let mut arguments = vec![discriminator];
    arguments.extend(fields);
    let set = InstructionNode {
        name: "set".into(),
        accounts: vec![InstructionAccountNode::new("owner", false, true)],
        arguments,
        discriminators: vec![FieldDiscriminatorNode::new("discriminator", 0).into()],
        ..Default::default()
    };
    let program = ProgramNode::new("sample", pk(30).to_string()).add_instruction(set);
    ProgramClient::from_root(RootNode::new(program)).unwrap()
}

fn displayed(format: codama_nodes::NumberFormat, decimals: u8, unit: &str) -> NumberTypeNode {
    let mut number = NumberTypeNode::le(format);
    number.display = Box::new(Some(NumberDisplayNode::Amount(
        AmountNumberDisplayNode::new(NumberValueNode::new(decimals), StringValueNode::new(unit)),
    )));
    number
}

fn build(client: &ProgramClient, arguments: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    let input = IdlBuildInput {
        accounts: vec![("owner".into(), Some(pk(31)))],
        arguments: arguments
            .iter()
            .map(|(name, text)| (name.to_string(), text.to_string()))
            .collect(),
    };
    build_idl_instruction(client, "set", &input)
        .map(|built| built.data)
        .map_err(|err| err.to_string())
}

#[test]
fn amount_types_imply_their_scale() {
    let client = sample(vec![
        InstructionArgumentNode::new("lamports", SolAmountTypeNode::new(NumberTypeNode::le(U64))),
        InstructionArgumentNode::new(
            "price",
            AmountTypeNode::new(NumberTypeNode::le(U64), 6, Some("USDC".to_string())),
        ),
        // A display on the number wins, as on Review.
        InstructionArgumentNode::new(
            "fee",
            AmountTypeNode::new(displayed(U16, 2, "%"), 6, Some("USDC".to_string())),
        ),
    ]);
    assert_eq!(scale_of(&client, "set", "lamports"), fixed(9, "SOL"));
    assert_eq!(scale_of(&client, "set", "price"), fixed(6, "USDC"));
    assert_eq!(scale_of(&client, "set", "fee"), fixed(2, "%"));
    let data = build(
        &client,
        &[("lamports", "0.5"), ("price", "2.25"), ("fee", "1.5")],
    )
    .unwrap();
    let mut expected = vec![7u8];
    expected.extend_from_slice(&500_000_000u64.to_le_bytes());
    expected.extend_from_slice(&2_250_000u64.to_le_bytes());
    expected.extend_from_slice(&150u16.to_le_bytes());
    assert_eq!(data, expected);
}

#[test]
fn a_scaled_amount_is_read_exactly() {
    let system = core_client("system");
    let send = |amount: &str| {
        let mut draft = IdlDraft::for_instruction(
            system.program_address(),
            "system",
            &system,
            Some("transferSol"),
        );
        draft.set_account("source", Some(pk(1)));
        draft.set_account("destination", Some(pk(2)));
        draft.set_argument("amount", amount.to_string());
        draft.build(&system).map_err(|err| err.to_string())
    };
    assert_eq!(send("1.5").unwrap(), transfer(pk(1), pk(2), 1_500_000_000));
    assert_eq!(send("0.000000001").unwrap(), transfer(pk(1), pk(2), 1));
    assert_eq!(send(" 2 ").unwrap(), transfer(pk(1), pk(2), 2_000_000_000));
    assert_eq!(
        send("18446744073.709551615").unwrap(),
        transfer(pk(1), pk(2), u64::MAX)
    );

    let refused = |amount: &str, says: &str| {
        let err = send(amount).unwrap_err();
        assert!(err.contains(says), "{amount}: {err}");
    };
    refused("0.0000000001", "more than 9 decimal places");
    refused("-1", "zero or more");
    refused("18446744073.709551616", "too large");
    refused("1e9", "Enter a number");
    refused("1,5", "Enter a number");
    refused("", "Enter a number.");
}

#[test]
fn a_field_too_small_for_the_value_refuses_it() {
    let client = sample(vec![
        InstructionArgumentNode::new("basisPoints", displayed(U16, 2, "%")),
        InstructionArgumentNode::new("count", NumberTypeNode::le(U8)),
    ]);
    let err = build(&client, &[("basisPoints", "655.36"), ("count", "1")]).unwrap_err();
    assert!(err.contains("too large"), "{err}");
    let err = build(&client, &[("basisPoints", "1"), ("count", "256")]).unwrap_err();
    assert!(err.contains("too large"), "{err}");
    let err = build(&client, &[("basisPoints", "1"), ("count", "-1")]).unwrap_err();
    assert!(err.contains("zero or more"), "{err}");
    let data = build(&client, &[("basisPoints", "655.35"), ("count", "255")]).unwrap();
    assert_eq!(data, vec![7, 0xff, 0xff, 0xff]);
}

#[test]
fn an_optional_scaled_amount_reads_blank_as_none() {
    let client = sample(vec![InstructionArgumentNode::new(
        "limit",
        OptionTypeNode::new(displayed(U64, 6, "USDC")),
    )]);
    assert_eq!(scale_of(&client, "set", "limit"), fixed(6, "USDC"));
    assert_eq!(build(&client, &[("limit", "")]).unwrap(), vec![7, 0]);
    let mut expected = vec![7, 1];
    expected.extend_from_slice(&2_500_000u64.to_le_bytes());
    assert_eq!(build(&client, &[("limit", "2.5")]).unwrap(), expected);
    let err = build(&client, &[("limit", "0.0000001")]).unwrap_err();
    assert!(err.contains("more than 6 decimal places"), "{err}");

    // Opened on Build, it shows what was typed.
    let built = build_idl_instruction(
        &client,
        "set",
        &IdlBuildInput {
            accounts: vec![("owner".into(), Some(pk(31)))],
            arguments: vec![("limit".into(), "2.5".into())],
        },
    )
    .unwrap();
    let draft = idl_draft_for(&client, "sample", &built).unwrap();
    assert_eq!(draft.argument("limit"), "2.5");
}

#[test]
fn a_scale_from_another_argument_asks_for_it_first() {
    for name in ["token", "token2022"] {
        let token = core_client(name);
        let program = token.program_address();
        let send = |amount: &str, decimals: &str| {
            let mut draft =
                IdlDraft::for_instruction(program, name, &token, Some("transferChecked"));
            draft.set_account("source", Some(pk(1)));
            draft.set_account("mint", Some(pk(2)));
            draft.set_account("destination", Some(pk(3)));
            draft.set_account("authority", Some(pk(4)));
            draft.set_argument("amount", amount.to_string());
            draft.set_argument("decimals", decimals.to_string());
            draft.build(&token).map_err(|err| err.to_string())
        };
        let err = send("1.5", "").unwrap_err();
        assert!(err.contains("Enter decimals first"), "{err}");
        let err = send("1.5", "six").unwrap_err();
        assert!(err.contains("Enter decimals"), "{err}");
        let err = send("1.5", "256").unwrap_err();
        assert!(err.contains("Enter decimals"), "{err}");
        // A blank amount says so before anything else.
        assert_eq!(send("", "").unwrap_err(), "Enter a number.");
        assert_eq!(
            send("1.5", "6").unwrap(),
            token_transfer_checked(program, pk(1), pk(2), pk(3), pk(4), 1_500_000, 6)
        );
        let err = send("1.5", "0").unwrap_err();
        assert!(err.contains("whole number"), "{err}");
        assert_eq!(
            send("15", "0").unwrap(),
            token_transfer_checked(program, pk(1), pk(2), pk(3), pk(4), 15, 0)
        );
    }
}

/// Text a filled form could hold for `kind`: an amount with a fraction where
/// its scale allows one, and 6 for an argument another field is scaled by.
pub(super) fn sample_text(kind: &IdlArgKind, scale: &IdlArgScale, source: bool) -> Option<String> {
    Some(match kind {
        _ if source => "6".to_string(),
        IdlArgKind::Option(inner) => return sample_text(inner, scale, false),
        IdlArgKind::Integer { float: true, .. } => "1.5".to_string(),
        IdlArgKind::Integer { .. } => match scale {
            IdlArgScale::Fixed { decimals: 0, .. } | IdlArgScale::None => "15".to_string(),
            IdlArgScale::Fixed { .. } | IdlArgScale::FromArgument { .. } => "1.5".to_string(),
        },
        IdlArgKind::Pubkey => pk(50).to_string(),
        IdlArgKind::String => "x".to_string(),
        IdlArgKind::Bool => "false".to_string(),
        IdlArgKind::Enum { variants } => variants.first()?.clone(),
        IdlArgKind::Bytes => "00".to_string(),
        IdlArgKind::List(item) => match **item {
            IdlArgKind::String => r#"["a","b"]"#.to_string(),
            IdlArgKind::Bool => "[true]".to_string(),
            IdlArgKind::Pubkey => format!(r#"["{}"]"#, pk(50)),
            IdlArgKind::Struct { .. } | IdlArgKind::Variants { .. } => {
                serde_json::Value::Array(vec![sample_json(item)?]).to_string()
            }
            _ => r#"["15"]"#.to_string(),
        },
        IdlArgKind::FixedBytes { .. } => "x".to_string(),
        IdlArgKind::Variants { variants } => match variants.first()? {
            (name, IdlVariant::Empty) => name.clone(),
            _ => sample_json(kind)?.to_string(),
        },
        IdlArgKind::Struct { .. } => sample_json(kind)?.to_string(),
        IdlArgKind::Raw { .. } => return None,
    })
}

/// What a filled form could hold for `kind` inside JSON: the last variant
/// of an enum with fields, so a variant that holds values is built too.
fn sample_json(kind: &IdlArgKind) -> Option<serde_json::Value> {
    use serde_json::Value as Json;
    Some(match kind {
        IdlArgKind::Option(inner) => sample_json(inner)?,
        IdlArgKind::Integer { float: true, .. } => Json::from(1.5),
        IdlArgKind::Integer { .. } => Json::from(15),
        IdlArgKind::Pubkey => Json::String(pk(50).to_string()),
        IdlArgKind::String => Json::String("x".into()),
        IdlArgKind::Bool => Json::Bool(false),
        IdlArgKind::Enum { variants } => Json::String(variants.first()?.clone()),
        IdlArgKind::Bytes => Json::String("00".into()),
        IdlArgKind::FixedBytes { .. } => Json::String("x".into()),
        IdlArgKind::List(item) => Json::Array(vec![sample_json(item)?]),
        IdlArgKind::Struct { fields } => Json::Object(
            fields
                .iter()
                .map(|(name, kind)| Some((name.clone(), sample_json(kind)?)))
                .collect::<Option<_>>()?,
        ),
        IdlArgKind::Variants { variants } => {
            let (name, held) = variants.last()?;
            let held = match held {
                IdlVariant::Empty => return Some(Json::String(name.clone())),
                IdlVariant::Tuple(items) => {
                    let mut items = items.iter().map(sample_json).collect::<Option<Vec<_>>>()?;
                    if items.len() == 1 {
                        items.pop()?
                    } else {
                        Json::Array(items)
                    }
                }
                IdlVariant::Struct(fields) => sample_json(&IdlArgKind::Struct {
                    fields: fields.clone(),
                })?,
            };
            Json::Object([(name.clone(), held)].into_iter().collect())
        }
        IdlArgKind::Raw { .. } => return None,
    })
}

/// Builds `instruction` with every field filled, opens it as Build would,
/// and builds it again. The number of scaled fields checked.
fn round_trip(client: &ProgramClient, program: &str, instruction: &str) -> Result<usize, String> {
    let form = idl_instruction_form(client, instruction, &IdlBuildInput::default())?;
    let sources: Vec<&str> = form
        .arguments
        .iter()
        .filter_map(|field| match &field.scale {
            IdlArgScale::FromArgument { argument, .. } => Some(argument.as_str()),
            _ => None,
        })
        .collect();
    let mut draft =
        IdlDraft::for_instruction(client.program_address(), program, client, Some(instruction));
    for (i, account) in form.accounts.iter().enumerate() {
        if account.source != IdlAccountSource::Derived {
            draft.set_account(&account.name, Some(pk(100 + i as u8)));
        }
    }
    for field in &form.arguments {
        let source = sources.contains(&field.name.as_str());
        let text = sample_text(&field.kind, &field.scale, source)
            .ok_or_else(|| format!("{} has no widget", field.name))?;
        draft.set_argument(&field.name, text);
    }
    let mut built = draft.build(client);
    for _ in 0..4 {
        match &built {
            Err(IdlBuildFailure::FillAddress { account, .. }) => {
                draft.set_account(&account.clone(), Some(pk(90)));
                built = draft.build(client);
            }
            _ => break,
        }
    }
    let built = built.map_err(|err| format!("does not build: {err}"))?;
    let opened = idl_draft_for(client, program, &built).ok_or("does not open on Build")?;
    let mut checked = 0;
    for field in &form.arguments {
        if field.scale == IdlArgScale::None {
            continue;
        }
        let (typed, shown) = (draft.argument(&field.name), opened.argument(&field.name));
        if typed != shown {
            return Err(format!(
                "{} typed `{typed}`, opens as `{shown}`",
                field.name
            ));
        }
        checked += 1;
    }
    let again = opened
        .clone()
        .build(client)
        .map_err(|err| format!("opened, does not build: {err}"))?;
    if again != built {
        return Err("opened, builds other bytes".to_string());
    }
    Ok(checked)
}

#[test]
fn every_scaled_field_of_the_core_idls_opens_as_typed_and_builds_the_same() {
    let mut checked = 0;
    let mut failures = Vec::new();
    let mut skipped = Vec::new();
    for idl in &BUILTIN_IDLS {
        let client = core_client(idl.name);
        for instruction in client.instruction_names() {
            let form =
                idl_instruction_form(&client, instruction, &IdlBuildInput::default()).unwrap();
            if form
                .arguments
                .iter()
                .all(|field| field.scale == IdlArgScale::None)
            {
                continue;
            }
            // A field Build has no widget for, such as a confidential
            // transfer's proof data, keeps the whole instruction off Build.
            if form.arguments.iter().any(|field| raw_inside(&field.kind)) {
                skipped.push(format!("{}.{instruction}", idl.name));
                continue;
            }
            match round_trip(&client, idl.name, instruction) {
                Ok(count) => checked += count,
                Err(err) => failures.push(format!("{}.{instruction}: {err}", idl.name)),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(checked >= 40, "only {checked} scaled fields checked");
    assert!(skipped.len() <= 4, "skipped {skipped:?}");
}

fn raw_inside(kind: &IdlArgKind) -> bool {
    match kind {
        IdlArgKind::Raw { .. } => true,
        IdlArgKind::Option(inner) => raw_inside(inner),
        _ => false,
    }
}

#[test]
fn derived_accounts_fold_once_filled_and_until_changed() {
    use crate::ids::ASSOCIATED_TOKEN_PROGRAM_ID;
    let store = crate::fixtures::shipped_catalog(crate::IdlNetwork::Devnet);
    let client = store.program_client(&ASSOCIATED_TOKEN_PROGRAM_ID).unwrap();
    let mut draft = IdlDraft::for_instruction(
        ASSOCIATED_TOKEN_PROGRAM_ID,
        "associatedToken",
        &client,
        Some("createAssociatedTokenIdempotent"),
    );
    let folded = |draft: &IdlDraft| -> Vec<String> {
        idl_instruction_form(&client, &draft.instruction, &draft.input())
            .unwrap()
            .accounts
            .into_iter()
            .filter(|account| draft.filled_by_idl(account))
            .map(|account| account.name)
            .collect()
    };
    // The associated account waits for its owner and mint.
    assert!(!folded(&draft).contains(&"ata".to_string()));
    assert!(folded(&draft).contains(&"tokenProgram".to_string()));

    draft.set_account("payer", Some(pk(1)));
    draft.set_account("owner", Some(pk(2)));
    draft.set_account("mint", Some(pk(3)));
    let filled = folded(&draft);
    assert!(filled.contains(&"ata".to_string()), "{filled:?}");
    assert!(!filled.contains(&"payer".to_string()), "{filled:?}");

    // Changed by hand, it is a field again.
    draft.set_manual("tokenProgram", true);
    assert!(!folded(&draft).contains(&"tokenProgram".to_string()));
}

#[test]
fn an_argument_folds_while_it_is_left_to_its_default() {
    let system = core_client("system");
    let mut draft = IdlDraft::for_instruction(
        system.program_address(),
        "system",
        &system,
        Some("createAccountAllowPrefund"),
    );
    let form = idl_instruction_form(&system, &draft.instruction, &draft.input()).unwrap();
    let lamports = form
        .arguments
        .iter()
        .find(|field| field.name == "lamports")
        .unwrap();
    let space = form
        .arguments
        .iter()
        .find(|field| field.name == "space")
        .unwrap();
    assert!(lamports.blank_uses_default);
    assert_eq!(lamports.scale, fixed(9, "SOL"));
    assert!(draft.left_to_default(lamports));
    assert!(!draft.left_to_default(space), "space has no default");
    draft.set_argument("lamports", "1.5".into());
    assert!(!draft.left_to_default(lamports));
    draft.set_argument("lamports", "  ".into());
    assert!(draft.left_to_default(lamports));
}
