//! The fields an instruction needs, read from its IDL.

use super::*;

/// Describes the form for `instruction`, including any PDA address that can be
/// derived from `input`.
pub fn idl_instruction_form(
    client: &ProgramClient,
    instruction: &str,
    input: &IdlBuildInput,
) -> Result<IdlForm, String> {
    let node = client.instruction_node(instruction).ok_or_else(|| {
        format!(
            "No instruction `{instruction}` on this program. Available: {}.",
            client.instruction_names().join(", ")
        )
    })?;
    let mut filled = filled_accounts(input);
    // A fixed address the user has not replaced is a seed like any typed one,
    // so the associated-token address shows before the token program is picked.
    for account in &node.accounts {
        if let Some(InstructionInputValueNode::PublicKeyValue(value)) =
            account.default_value.as_ref().as_ref()
        {
            if let Ok(address) = Pubkey::from_str(&value.public_key) {
                filled.entry(account.name.to_string()).or_insert(address);
            }
        }
    }
    let args = argument_text(input);
    Ok(IdlForm {
        program_id: client.program_address(),
        instruction: instruction.to_string(),
        accounts: node
            .accounts
            .iter()
            .map(|account| {
                account_field(
                    client.root(),
                    &client.program_address(),
                    account,
                    &filled,
                    &args,
                )
            })
            .collect(),
        arguments: node.arguments.iter().filter_map(argument_field).collect(),
    })
}

pub(super) fn account_field(
    root: &RootNode,
    program_id: &Pubkey,
    account: &InstructionAccountNode,
    filled: &HashMap<String, Pubkey>,
    args: &HashMap<String, String>,
) -> IdlAccountField {
    let signer = !matches!(account.is_signer, codama_nodes::IsSigner::False);
    let optional = account.is_optional.unwrap_or(false);
    let mut field = IdlAccountField {
        name: account.name.to_string(),
        signer,
        optional,
        source: IdlAccountSource::Picker,
        seeds: Vec::new(),
        address: None,
        reason: String::new(),
    };
    let Some(default) = account.default_value.as_ref().as_ref() else {
        return field;
    };
    match default {
        InstructionInputValueNode::PayerValue(_) | InstructionInputValueNode::IdentityValue(_) => {
            field
        }
        InstructionInputValueNode::PublicKeyValue(node) => {
            field.source = IdlAccountSource::Derived;
            field.seeds = vec![node.public_key.clone()];
            field.address = Pubkey::from_str(&node.public_key).ok();
            field
        }
        InstructionInputValueNode::ProgramIdValue(_)
        | InstructionInputValueNode::ProgramLink(_) => {
            field.source = IdlAccountSource::Derived;
            field.seeds = vec!["program id".to_string()];
            field.address = Some(*program_id);
            field
        }
        InstructionInputValueNode::AccountValue(node) => {
            field.source = IdlAccountSource::Derived;
            field.seeds = vec![format!("account {}", node.name.as_str())];
            field.address = filled.get(node.name.as_str()).copied();
            field
        }
        InstructionInputValueNode::ArgumentValue(node) => {
            field.source = IdlAccountSource::Derived;
            field.seeds = vec![format!("argument {}", node.name.as_str())];
            field.address = args
                .get(node.name.as_str())
                .and_then(|text| Pubkey::from_str(text.trim()).ok());
            field
        }
        InstructionInputValueNode::PdaValue(node) => {
            apply_pda(&mut field, root, program_id, node, filled, args);
            field
        }
        InstructionInputValueNode::AccountFieldValue(_) => {
            field.source = IdlAccountSource::Fill;
            field.reason = "needs account data".to_string();
            field
        }
        InstructionInputValueNode::ResolverValue(node) => {
            field.source = IdlAccountSource::Fill;
            field.reason = format!("resolver `{}` is not available offline", node.name.as_str());
            field
        }
        other => {
            field.source = IdlAccountSource::Fill;
            field.reason = format!("{} cannot be derived offline", other.kind());
            field
        }
    }
}

pub(super) fn argument_field(argument: &InstructionArgumentNode) -> Option<IdlArgField> {
    if argument.default_value_strategy == Some(DefaultValueStrategy::Omitted) {
        return None;
    }
    Some(IdlArgField {
        name: argument.name.to_string(),
        kind: classify(&argument.r#type),
        blank_uses_default: argument.default_value_strategy == Some(DefaultValueStrategy::Optional),
    })
}

pub(super) fn classify(ty: &TypeNode) -> IdlArgKind {
    match unwrap_type(ty) {
        TypeNode::Number(node) => integer_kind(node.format),
        TypeNode::Amount(node) => integer_kind(node.number.get_nested_type_node().format),
        TypeNode::SolAmount(node) => integer_kind(node.number.get_nested_type_node().format),
        TypeNode::PublicKey(_) => IdlArgKind::Pubkey,
        TypeNode::String(_) => IdlArgKind::String,
        TypeNode::Boolean(_) => IdlArgKind::Bool,
        TypeNode::Bytes(_) => IdlArgKind::Bytes,
        TypeNode::Option(node) => IdlArgKind::Option(Box::new(classify(&node.item))),
        TypeNode::RemainderOption(node) => IdlArgKind::Option(Box::new(classify(&node.item))),
        TypeNode::ZeroableOption(node) => IdlArgKind::Option(Box::new(classify(&node.item))),
        TypeNode::Enum(node) => match unit_variants(node) {
            Some(variants) => IdlArgKind::Enum { variants },
            None => IdlArgKind::Raw {
                note: "enum with fields".to_string(),
            },
        },
        other => IdlArgKind::Raw {
            note: other.kind().to_string(),
        },
    }
}

pub(super) fn unwrap_type(ty: &TypeNode) -> &TypeNode {
    match ty {
        TypeNode::SizePrefix(node) => unwrap_type(&node.r#type),
        TypeNode::FixedSize(node) => unwrap_type(&node.r#type),
        TypeNode::HiddenPrefix(node) => unwrap_type(&node.r#type),
        TypeNode::HiddenSuffix(node) => unwrap_type(&node.r#type),
        TypeNode::PreOffset(node) => unwrap_type(&node.r#type),
        TypeNode::PostOffset(node) => unwrap_type(&node.r#type),
        TypeNode::Sentinel(node) => unwrap_type(&node.r#type),
        other => other,
    }
}

pub(super) fn integer_kind(format: NumberFormat) -> IdlArgKind {
    IdlArgKind::Integer {
        signed: matches!(
            format,
            NumberFormat::I8
                | NumberFormat::I16
                | NumberFormat::I32
                | NumberFormat::I64
                | NumberFormat::I128
        ),
        float: matches!(format, NumberFormat::F32 | NumberFormat::F64),
    }
}

pub(super) fn unit_variants(node: &codama_nodes::EnumTypeNode) -> Option<Vec<String>> {
    let mut names = Vec::with_capacity(node.variants.len());
    for variant in &node.variants {
        match variant {
            codama_nodes::EnumVariantTypeNode::Empty(empty) => names.push(empty.name.to_string()),
            _ => return None,
        }
    }
    Some(names)
}
