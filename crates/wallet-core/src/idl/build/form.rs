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
    let arguments: Vec<IdlArgField> = node
        .arguments
        .iter()
        .flat_map(|argument| argument_fields(&client.root().program, node, argument))
        .collect();
    // Seeds read an argument as the program does: in base units.
    let args = base_unit_text(&arguments, &argument_text(input));
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
        arguments,
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

/// Between a struct argument's name and one of its fields: `params.gitUrl`.
pub const STRUCT_FIELD_SEPARATOR: char = '.';

/// The rows of one argument: one, or one per field when its type is a
/// struct, directly or through a link to the program's own types, and it
/// has no default. A field is typed as an argument of its type would be,
/// in base units; a struct inside a struct is a field nobody can fill.
pub(super) fn argument_fields(
    program: &ProgramNode,
    instruction: &InstructionNode,
    argument: &InstructionArgumentNode,
) -> Vec<IdlArgField> {
    if argument.default_value_strategy == Some(DefaultValueStrategy::Omitted) {
        return Vec::new();
    }
    match struct_fields(program, &argument.r#type, 0) {
        Some(fields) if !fields.is_empty() && argument.default_value.is_none() => fields
            .iter()
            .map(|field| IdlArgField {
                name: format!(
                    "{}{STRUCT_FIELD_SEPARATOR}{}",
                    argument.name.as_str(),
                    field.name.as_str()
                ),
                kind: classify_in(program, &field.r#type),
                blank_uses_default: false,
                scale: IdlArgScale::None,
                remainder_option: false,
            })
            .collect(),
        _ => argument_field(program, instruction, argument)
            .into_iter()
            .collect(),
    }
}

/// The fields of `ty` when it is a struct, following links as
/// [`classify_in`] does.
fn struct_fields<'a>(
    program: &'a ProgramNode,
    ty: &'a TypeNode,
    links: usize,
) -> Option<&'a [codama_nodes::StructFieldTypeNode]> {
    match unwrap_type(ty) {
        TypeNode::Struct(node) => Some(&node.fields),
        TypeNode::Link(link) if links < LINK_DEPTH => {
            struct_fields(program, linked_type(program, link)?, links + 1)
        }
        _ => None,
    }
}

pub(super) fn argument_field(
    program: &ProgramNode,
    instruction: &InstructionNode,
    argument: &InstructionArgumentNode,
) -> Option<IdlArgField> {
    if argument.default_value_strategy == Some(DefaultValueStrategy::Omitted) {
        return None;
    }
    let kind = classify_in(program, &argument.r#type);
    let scale = if integer_inside(&kind) {
        argument_scale(instruction, &argument.r#type)
    } else {
        IdlArgScale::None
    };
    Some(IdlArgField {
        name: argument.name.to_string(),
        kind,
        blank_uses_default: argument.default_value_strategy == Some(DefaultValueStrategy::Optional),
        scale,
        remainder_option: matches!(unwrap_type(&argument.r#type), TypeNode::RemainderOption(_)),
    })
}

/// An integer, or an option of one.
fn integer_inside(kind: &IdlArgKind) -> bool {
    match kind {
        IdlArgKind::Integer { float, .. } => !float,
        IdlArgKind::Option(inner) => integer_inside(inner),
        _ => false,
    }
}

/// How many links a field's type is followed through to the program's own
/// defined types: a link, and a link that link names.
const LINK_DEPTH: usize = 2;

/// The field a type is typed into, without following links: a link is a
/// field nobody can fill. Seeds and scales read types this way.
pub(super) fn classify(ty: &TypeNode) -> IdlArgKind {
    classify_at(None, ty, 0)
}

/// The field a type is typed into, following a link to a defined type of
/// `program` up to [`LINK_DEPTH`] links deep. Only an enum whose variants
/// hold nothing is read through a link, as a choice; anything else behind
/// one, a link to another program, a type the program does not define, or
/// a link past the depth stays a field nobody can fill. A type that links
/// to itself ends there too: IDLs are hostile input.
pub(super) fn classify_in(program: &ProgramNode, ty: &TypeNode) -> IdlArgKind {
    classify_at(Some(program), ty, 0)
}

fn classify_at(program: Option<&ProgramNode>, ty: &TypeNode, links: usize) -> IdlArgKind {
    match unwrap_type(ty) {
        TypeNode::Number(node) => integer_kind(node.format),
        TypeNode::Amount(node) => integer_kind(node.number.get_nested_type_node().format),
        TypeNode::SolAmount(node) => integer_kind(node.number.get_nested_type_node().format),
        TypeNode::PublicKey(_) => IdlArgKind::Pubkey,
        TypeNode::String(_) => IdlArgKind::String,
        TypeNode::Boolean(_) => IdlArgKind::Bool,
        TypeNode::Bytes(_) => IdlArgKind::Bytes,
        TypeNode::Option(node) => {
            IdlArgKind::Option(Box::new(classify_at(program, &node.item, links)))
        }
        TypeNode::RemainderOption(node) => {
            IdlArgKind::Option(Box::new(classify_at(program, &node.item, links)))
        }
        TypeNode::ZeroableOption(node) => {
            IdlArgKind::Option(Box::new(classify_at(program, &node.item, links)))
        }
        TypeNode::Array(node) => {
            let item = classify_at(program, &node.item, links);
            let listable = matches!(
                item,
                IdlArgKind::String
                    | IdlArgKind::Pubkey
                    | IdlArgKind::Bool
                    | IdlArgKind::Integer { float: false, .. }
                    | IdlArgKind::Struct { .. }
                    | IdlArgKind::Variants { .. }
            );
            let byte = item
                == IdlArgKind::Integer {
                    signed: false,
                    float: false,
                    bits: 8,
                };
            match node.count.as_ref() {
                codama_nodes::CountNode::Prefixed(_) | codama_nodes::CountNode::Remainder(_)
                    if listable =>
                {
                    IdlArgKind::List(Box::new(item))
                }
                codama_nodes::CountNode::Fixed(count)
                    if byte && count.value <= MAX_FIXED_BYTES as u64 =>
                {
                    IdlArgKind::FixedBytes {
                        len: count.value as usize,
                    }
                }
                _ => IdlArgKind::Raw {
                    note: "array".to_string(),
                },
            }
        }
        TypeNode::Enum(node) => match unit_variants(node) {
            Some(variants) => IdlArgKind::Enum { variants },
            None => data_variants(program, node, links).unwrap_or_else(|| IdlArgKind::Raw {
                note: "enum with fields".to_string(),
            }),
        },
        TypeNode::Struct(node) => match struct_kinds(program, &node.fields, links) {
            Some(fields) if !fields.is_empty() => IdlArgKind::Struct { fields },
            _ => IdlArgKind::Raw {
                note: "struct".to_string(),
            },
        },
        TypeNode::Link(link) => program
            .filter(|_| links < LINK_DEPTH)
            .and_then(|program| linked_type(program, link))
            .map(|linked| classify_at(program, linked, links + 1))
            .filter(|kind| {
                matches!(
                    kind,
                    IdlArgKind::Enum { .. }
                        | IdlArgKind::Struct { .. }
                        | IdlArgKind::Variants { .. }
                )
            })
            .unwrap_or_else(|| IdlArgKind::Raw {
                note: link.kind().to_string(),
            }),
        other => IdlArgKind::Raw {
            note: other.kind().to_string(),
        },
    }
}

/// The most bytes a fixed byte array is typed as text for. Larger arrays,
/// such as a whole account's worth, stay fields nobody can fill.
pub(super) const MAX_FIXED_BYTES: usize = 1024;

/// The kind of each field, or `None` when one is a field nobody can fill.
fn struct_kinds(
    program: Option<&ProgramNode>,
    fields: &[codama_nodes::StructFieldTypeNode],
    links: usize,
) -> Option<Vec<(String, IdlArgKind)>> {
    fields
        .iter()
        .map(|field| {
            let kind = classify_at(program, &field.r#type, links);
            (!raw_inside(&kind)).then(|| (field.name.to_string(), kind))
        })
        .collect()
}

/// An enum whose variants hold fields, each of a kind the form can type.
fn data_variants(
    program: Option<&ProgramNode>,
    node: &codama_nodes::EnumTypeNode,
    links: usize,
) -> Option<IdlArgKind> {
    let mut variants = Vec::with_capacity(node.variants.len());
    for variant in &node.variants {
        let (name, held) = match variant {
            codama_nodes::EnumVariantTypeNode::Empty(empty) => {
                (empty.name.to_string(), IdlVariant::Empty)
            }
            codama_nodes::EnumVariantTypeNode::Struct(node) => {
                let fields = struct_fields_of(program, &node.r#struct, links)?;
                (node.name.to_string(), IdlVariant::Struct(fields))
            }
            codama_nodes::EnumVariantTypeNode::Tuple(node) => {
                let items = node
                    .tuple
                    .get_nested_type_node()
                    .items
                    .iter()
                    .map(|item| {
                        let kind = classify_at(program, item, links);
                        (!raw_inside(&kind)).then_some(kind)
                    })
                    .collect::<Option<Vec<_>>>()?;
                (node.name.to_string(), IdlVariant::Tuple(items))
            }
        };
        variants.push((name, held));
    }
    Some(IdlArgKind::Variants { variants })
}

fn struct_fields_of(
    program: Option<&ProgramNode>,
    node: &codama_nodes::NestedTypeNode<codama_nodes::StructTypeNode>,
    links: usize,
) -> Option<Vec<(String, IdlArgKind)>> {
    struct_kinds(program, &node.get_nested_type_node().fields, links)
}

/// Whether a field nobody can fill is inside `kind`.
pub(crate) fn raw_inside(kind: &IdlArgKind) -> bool {
    match kind {
        IdlArgKind::Raw { .. } => true,
        IdlArgKind::Option(inner) | IdlArgKind::List(inner) => raw_inside(inner),
        IdlArgKind::Struct { fields } => fields.iter().any(|(_, kind)| raw_inside(kind)),
        IdlArgKind::Variants { variants } => variants.iter().any(|(_, held)| match held {
            IdlVariant::Empty => false,
            IdlVariant::Tuple(items) => items.iter().any(raw_inside),
            IdlVariant::Struct(fields) => fields.iter().any(|(_, kind)| raw_inside(kind)),
        }),
        _ => false,
    }
}

/// The type `link` names among `program`'s own defined types.
fn linked_type<'a>(program: &'a ProgramNode, link: &DefinedTypeLinkNode) -> Option<&'a TypeNode> {
    if let Some(other) = &link.program {
        if other.name != program.name {
            return None;
        }
    }
    program
        .defined_types
        .iter()
        .find(|defined| defined.name == link.name)
        .map(|defined| defined.r#type.as_ref())
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
    let bits = match format {
        NumberFormat::U8 | NumberFormat::I8 => 8,
        NumberFormat::U16 | NumberFormat::I16 | NumberFormat::ShortU16 => 16,
        NumberFormat::U32 | NumberFormat::I32 | NumberFormat::F32 => 32,
        NumberFormat::U64 | NumberFormat::I64 | NumberFormat::F64 => 64,
        NumberFormat::U128 | NumberFormat::I128 => 128,
    };
    IdlArgKind::Integer {
        bits,
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
