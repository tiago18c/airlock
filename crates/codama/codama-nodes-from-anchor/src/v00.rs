//! Converter for the legacy Anchor IDL spec (pre-0.30 Anchor and Shank).

use crate::discriminators::{
    anchor_account_discriminator, anchor_event_discriminator, anchor_instruction_discriminator,
};
use crate::helpers::{arr, docs, flag, opt_str, req_str};
use crate::{AnchorIdlError, Result};
use codama_nodes::{
    AccountNode, ArrayTypeNode, BooleanTypeNode, BooleanValueNode, BytesTypeNode, BytesValueNode,
    ConstantDiscriminatorNode, ConstantPdaSeedNode, ConstantValueNode, DefaultValueStrategy,
    DefinedTypeLinkNode, DefinedTypeNode, Docs, EnumEmptyVariantTypeNode,
    EnumStructVariantTypeNode, EnumTupleVariantTypeNode, EnumTypeNode, EnumVariantTypeNode,
    ErrorNode, EventNode, FieldDiscriminatorNode, FixedCountNode, FixedSizeTypeNode,
    HiddenPrefixTypeNode, InstructionAccountNode, InstructionArgumentNode, InstructionNode,
    IsSigner, MapTypeNode, NestedTypeNode, NumberFormat, NumberTypeNode, NumberValueNode,
    OptionTypeNode, OptionalAccountStrategy, PdaLinkNode, PdaNode, PdaSeedNode, PrefixedCountNode,
    ProgramNode, ProgramOrigin, PublicKeyTypeNode, RemainderCountNode, SetTypeNode,
    SizePrefixTypeNode, StringTypeNode, StringValueNode, StructFieldTypeNode, StructTypeNode,
    TupleTypeNode, TypeNode, VariablePdaSeedNode, U32, U8,
};
use serde_json::Value;

pub fn program_node_from_anchor(idl: &Value) -> Result<ProgramNode> {
    let name = req_str(idl, "name")?;
    let version = opt_str(idl, "version").unwrap_or("");
    let metadata = idl.get("metadata");
    let address = metadata.and_then(|m| opt_str(m, "address")).unwrap_or("");
    let origin = metadata
        .and_then(|m| opt_str(m, "origin"))
        .unwrap_or("anchor");
    let is_anchor = origin == "anchor";

    let mut program = ProgramNode::new(name, address).set_version(version);
    program.origin = Some(if is_anchor {
        ProgramOrigin::Anchor
    } else {
        ProgramOrigin::Shank
    });

    for t in arr(idl, "types") {
        program
            .defined_types
            .push(defined_type_node_from_anchor(t)?);
    }
    for account in arr(idl, "accounts") {
        let (node, pda) = account_node_from_anchor(account, is_anchor)?;
        program.accounts.push(node);
        if let Some(pda) = pda {
            program.pdas.push(pda);
        }
    }
    if is_anchor {
        for event in arr(idl, "events") {
            program.events.push(event_node_from_anchor(event)?);
        }
    }
    for (index, instruction) in arr(idl, "instructions").iter().enumerate() {
        program
            .instructions
            .push(instruction_node_from_anchor(instruction, index, origin)?);
    }
    for error in arr(idl, "errors") {
        program.errors.push(error_node_from_anchor(error)?);
    }
    Ok(program)
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

pub fn instruction_node_from_anchor(
    idl: &Value,
    index: usize,
    origin: &str,
) -> Result<InstructionNode> {
    let name = req_str(idl, "name")?;

    // Three discriminator flavors: an explicit `discriminant` field, the
    // computed 8-byte Anchor hash, or the Shank 1-byte instruction index.
    let discriminator_argument = if let Some(discriminant) = idl.get("discriminant") {
        let r#type = type_node_from_anchor(
            discriminant
                .get("type")
                .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
        )?;
        let value = discriminant
            .get("value")
            .and_then(Value::as_f64)
            .ok_or_else(|| AnchorIdlError::MissingField("value".into()))?;
        Some(omitted_argument(r#type, NumberValueNode::new(value).into()))
    } else if origin == "anchor" {
        let disc = anchor_instruction_discriminator(name);
        Some(omitted_argument(
            FixedSizeTypeNode::new(BytesTypeNode::new(), disc.len()).into(),
            BytesValueNode::base16(hex::encode(disc)).into(),
        ))
    } else if origin == "shank" {
        // TS emits the index as unpadded hex; we zero-pad so the base16 value
        // stays decodable byte-for-byte.
        Some(omitted_argument(
            FixedSizeTypeNode::new(BytesTypeNode::new(), 1).into(),
            BytesValueNode::base16(format!("{index:02x}")).into(),
        ))
    } else {
        None
    };

    let mut arguments = Vec::new();
    let mut discriminators = Vec::new();
    if let Some(argument) = discriminator_argument {
        arguments.push(argument);
        discriminators.push(FieldDiscriminatorNode::new("discriminator", 0).into());
    }
    for arg in arr(idl, "args") {
        let mut node = InstructionArgumentNode::new(
            req_str(arg, "name")?,
            type_node_from_anchor(
                arg.get("type")
                    .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
            )?,
        );
        node.docs = docs(arg).into();
        arguments.push(node);
    }

    let mut accounts = Vec::new();
    flatten_accounts(arr(idl, "accounts"), &mut accounts)?;

    Ok(InstructionNode {
        name: name.into(),
        docs: docs(idl).into(),
        optional_account_strategy: Some(if flag(idl, "legacyOptionalAccountsStrategy") {
            OptionalAccountStrategy::Omitted
        } else {
            OptionalAccountStrategy::ProgramId
        }),
        accounts,
        arguments,
        discriminators,
        ..InstructionNode::default()
    })
}

fn omitted_argument(
    r#type: TypeNode,
    default_value: codama_nodes::InstructionInputValueNode,
) -> InstructionArgumentNode {
    InstructionArgumentNode {
        name: "discriminator".into(),
        default_value_strategy: Some(DefaultValueStrategy::Omitted),
        docs: Docs::default(),
        r#type: Box::new(r#type),
        default_value: Box::new(Some(default_value)),
        display: None,
    }
}

fn flatten_accounts(items: &[Value], out: &mut Vec<InstructionAccountNode>) -> Result<()> {
    for item in items {
        match item.get("accounts").and_then(Value::as_array) {
            Some(nested) => flatten_accounts(nested, out)?,
            None => out.push(instruction_account_from_anchor(item)?),
        }
    }
    Ok(())
}

fn instruction_account_from_anchor(idl: &Value) -> Result<InstructionAccountNode> {
    let is_signer = if flag(idl, "isOptionalSigner") {
        IsSigner::Either
    } else {
        IsSigner::from(flag(idl, "isSigner"))
    };
    let mut node =
        InstructionAccountNode::new(req_str(idl, "name")?, flag(idl, "isMut"), is_signer);
    node.is_optional = Some(flag(idl, "isOptional") || flag(idl, "optional"));
    if let Some(desc) = opt_str(idl, "desc") {
        node.docs = vec![desc.to_string()].into();
    }
    Ok(node)
}

// ---------------------------------------------------------------------------
// Accounts (inline struct types, optional PDA seeds)
// ---------------------------------------------------------------------------

pub fn account_node_from_anchor(
    idl: &Value,
    is_anchor: bool,
) -> Result<(AccountNode, Option<PdaNode>)> {
    let name = req_str(idl, "name")?;
    let data = type_node_from_anchor(
        idl.get("type")
            .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
    )?;
    let TypeNode::Struct(mut data) = data else {
        return Err(AnchorIdlError::AccountDataNotAStruct(name.to_string()));
    };

    let mut node = AccountNode::new(name, NestedTypeNode::Value(StructTypeNode::new(vec![])));
    node.docs = docs(idl).into();
    node.size = idl.get("size").and_then(Value::as_u64);

    if is_anchor {
        let disc = anchor_account_discriminator(name);
        data.fields.insert(
            0,
            StructFieldTypeNode {
                name: "discriminator".into(),
                default_value_strategy: Some(DefaultValueStrategy::Omitted),
                docs: Docs::default(),
                r#type: Box::new(FixedSizeTypeNode::new(BytesTypeNode::new(), disc.len()).into()),
                default_value: Box::new(Some(BytesValueNode::base16(hex::encode(disc)).into())),
                display: None,
            },
        );
        node.discriminators = vec![FieldDiscriminatorNode::new("discriminator", 0).into()];
    }
    node.data = NestedTypeNode::Value(data);

    let pda = pda_node_from_anchor(name, arr(idl, "seeds"))?;
    if pda.is_some() {
        node.pda = Some(PdaLinkNode::new(name));
    }
    Ok((node, pda))
}

/// Account-level `seeds` become a program-level [`PdaNode`]. A `programId`
/// seed drops the whole PDA: the runtime can encode a `programIdValueNode`,
/// but this converter does not emit one.
fn pda_node_from_anchor(name: &str, seeds: &[Value]) -> Result<Option<PdaNode>> {
    if seeds.is_empty() {
        return Ok(None);
    }
    let mut nodes: Vec<PdaSeedNode> = Vec::with_capacity(seeds.len());
    for seed in seeds {
        match opt_str(seed, "kind") {
            Some("constant") => {
                let r#type = match opt_str(seed, "type") {
                    Some("string") => StringTypeNode::utf8().into(),
                    Some("bytes") => BytesTypeNode::new().into(),
                    Some(other) => type_node_from_anchor(&Value::String(other.to_string()))?,
                    None => type_node_from_anchor(
                        seed.get("type")
                            .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
                    )?,
                };
                let value: codama_nodes::ValueNode = match seed.get("value") {
                    Some(Value::String(s)) => StringValueNode::new(s.as_str()).into(),
                    Some(Value::Number(n)) => {
                        NumberValueNode::new(n.as_f64().unwrap_or(0.0)).into()
                    }
                    Some(Value::Bool(b)) => BooleanValueNode::new(*b).into(),
                    _ => return Err(AnchorIdlError::MissingField("value".into())),
                };
                nodes.push(ConstantPdaSeedNode::new(r#type, value).into());
            }
            Some("variable") => {
                let r#type = type_node_from_anchor(
                    seed.get("type")
                        .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
                )?;
                let mut node = VariablePdaSeedNode::new(req_str(seed, "path")?, r#type);
                if let Some(desc) = opt_str(seed, "description") {
                    node.docs = vec![desc.to_string()].into();
                }
                nodes.push(node.into());
            }
            // Runtime encoding of `programIdValueNode` exists. This converter
            // still refuses the Anchor seed rather than emit that node.
            Some("programId") => return Ok(None),
            other => {
                return Err(AnchorIdlError::SeedKindUnimplemented(
                    other.unwrap_or("<missing>").to_string(),
                ))
            }
        }
    }
    Ok(Some(PdaNode::new(name, nodes)))
}

// ---------------------------------------------------------------------------
// Events, defined types, errors
// ---------------------------------------------------------------------------

pub fn event_node_from_anchor(idl: &Value) -> Result<EventNode> {
    let name = req_str(idl, "name")?;
    let fields = arr(idl, "fields")
        .iter()
        .map(|f| {
            Ok(StructFieldTypeNode::new(
                req_str(f, "name")?,
                type_node_from_anchor(
                    f.get("type")
                        .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
                )?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let data = StructTypeNode::new(fields);

    let disc = anchor_event_discriminator(name);
    let constant = ConstantValueNode::new(
        FixedSizeTypeNode::new(BytesTypeNode::new(), disc.len()),
        BytesValueNode::base16(hex::encode(disc)),
    );
    let mut node = EventNode::new(
        name,
        HiddenPrefixTypeNode::<TypeNode>::new(data, vec![constant.clone()]),
    );
    node.discriminators = vec![ConstantDiscriminatorNode::new(constant, 0).into()];
    Ok(node)
}

pub fn defined_type_node_from_anchor(idl: &Value) -> Result<DefinedTypeNode> {
    let mut node = DefinedTypeNode::new(
        req_str(idl, "name")?,
        type_node_from_anchor(
            idl.get("type")
                .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
        )?,
    );
    node.docs = docs(idl).into();
    Ok(node)
}

pub fn error_node_from_anchor(idl: &Value) -> Result<ErrorNode> {
    let name = req_str(idl, "name")?;
    let message = opt_str(idl, "msg").unwrap_or("");
    let code = idl.get("code").and_then(Value::as_u64).unwrap_or(0) as u32;
    let mut node = ErrorNode::new(name, code, message);
    let default_doc = if message.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {message}")
    };
    let doc_lines = docs(idl);
    node.docs = if doc_lines.is_empty() {
        vec![default_doc]
    } else {
        doc_lines
    }
    .into();
    Ok(node)
}

// ---------------------------------------------------------------------------
// Type mapping (legacy)
// ---------------------------------------------------------------------------

pub fn type_node_from_anchor(idl_type: &Value) -> Result<TypeNode> {
    match idl_type {
        Value::String(leaf) => leaf_type_node(leaf),
        Value::Object(_) => object_type_node(idl_type),
        other => Err(AnchorIdlError::UnrecognizedType(other.to_string())),
    }
}

fn leaf_type_node(leaf: &str) -> Result<TypeNode> {
    Ok(match leaf {
        "bool" => BooleanTypeNode::new(NumberTypeNode::le(U8)).into(),
        // Legacy spelling.
        "publicKey" | "pubkey" => PublicKeyTypeNode::new().into(),
        "string" => SizePrefixTypeNode::new(StringTypeNode::utf8(), NumberTypeNode::le(U32)).into(),
        "bytes" => SizePrefixTypeNode::new(BytesTypeNode::new(), NumberTypeNode::le(U32)).into(),
        other => NumberTypeNode::le(number_format(other)?).into(),
    })
}

fn number_format(name: &str) -> Result<NumberFormat> {
    Ok(match name {
        "u8" => NumberFormat::U8,
        "u16" => NumberFormat::U16,
        "u32" => NumberFormat::U32,
        "u64" => NumberFormat::U64,
        "u128" => NumberFormat::U128,
        "i8" => NumberFormat::I8,
        "i16" => NumberFormat::I16,
        "i32" => NumberFormat::I32,
        "i64" => NumberFormat::I64,
        "i128" => NumberFormat::I128,
        "f32" => NumberFormat::F32,
        "f64" => NumberFormat::F64,
        "shortU16" => NumberFormat::ShortU16,
        other => return Err(AnchorIdlError::UnrecognizedType(other.to_string())),
    })
}

/// `size` on legacy collections: a number → fixed count, `"remainder"`, or a
/// number format string → prefixed count (defaulting to `u32` when absent).
fn collection_count(size: Option<&Value>) -> Result<codama_nodes::CountNode> {
    Ok(match size {
        Some(Value::Number(n)) => FixedCountNode::new(n.as_u64().unwrap_or(0)).into(),
        Some(Value::String(s)) if s == "remainder" => RemainderCountNode::new().into(),
        Some(Value::String(format)) => {
            PrefixedCountNode::new(NumberTypeNode::le(number_format(format)?)).into()
        }
        None => PrefixedCountNode::new(NumberTypeNode::le(U32)).into(),
        Some(other) => return Err(AnchorIdlError::UnrecognizedType(other.to_string())),
    })
}

fn object_type_node(idl_type: &Value) -> Result<TypeNode> {
    if let Some(array) = idl_type.get("array").and_then(Value::as_array) {
        if array.len() == 2 {
            let item = type_node_from_anchor(&array[0])?;
            let count = array[1].as_u64().unwrap_or(0);
            return Ok(ArrayTypeNode::new(item, FixedCountNode::new(count)).into());
        }
    }
    if let Some(item) = idl_type.get("vec") {
        let item = type_node_from_anchor(item)?;
        return Ok(ArrayTypeNode::new(item, collection_count(idl_type.get("size"))?).into());
    }
    if let Some(defined) = idl_type.get("defined") {
        // Legacy string form; tolerate the v01 object form for robustness.
        let name = match defined {
            Value::String(s) => s.as_str(),
            other => req_str(other, "name")?,
        };
        return Ok(DefinedTypeLinkNode::new(name).into());
    }
    if let Some(items) = idl_type.get("tuple").and_then(Value::as_array) {
        let items = items
            .iter()
            .map(type_node_from_anchor)
            .collect::<Result<Vec<_>>>()?;
        return Ok(TupleTypeNode::new(items).into());
    }
    for (field, default_prefix, default_fixed) in [
        ("option", NumberFormat::U8, false),
        ("coption", NumberFormat::U32, true),
    ] {
        if let Some(item) = idl_type.get(field) {
            let mut node = OptionTypeNode::new(type_node_from_anchor(item)?);
            node.fixed = Some(
                idl_type
                    .get("fixed")
                    .and_then(Value::as_bool)
                    .unwrap_or(default_fixed),
            );
            let prefix = match opt_str(idl_type, "prefix") {
                Some(format) => number_format(format)?,
                None => default_prefix,
            };
            node.prefix = NumberTypeNode::le(prefix).into();
            return Ok(node.into());
        }
    }
    for map_kind in ["hashMap", "bTreeMap"] {
        if let Some(pair) = idl_type.get(map_kind).and_then(Value::as_array) {
            if pair.len() == 2 {
                return Ok(MapTypeNode::new(
                    type_node_from_anchor(&pair[0])?,
                    type_node_from_anchor(&pair[1])?,
                    collection_count(idl_type.get("size"))?,
                )
                .into());
            }
        }
    }
    for set_kind in ["hashSet", "bTreeSet"] {
        if let Some(item) = idl_type.get(set_kind) {
            return Ok(SetTypeNode::new(
                type_node_from_anchor(item)?,
                collection_count(idl_type.get("size"))?,
            )
            .into());
        }
    }
    match opt_str(idl_type, "kind") {
        Some("struct") => {
            let fields = arr(idl_type, "fields")
                .iter()
                .map(|f| {
                    let mut node = StructFieldTypeNode::new(
                        req_str(f, "name")?,
                        type_node_from_anchor(
                            f.get("type")
                                .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
                        )?,
                    );
                    node.docs = docs(f).into();
                    Ok(node)
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(StructTypeNode::new(fields).into())
        }
        Some("enum") => {
            let variants = arr(idl_type, "variants")
                .iter()
                .map(enum_variant_from_anchor)
                .collect::<Result<Vec<_>>>()?;
            let mut node = EnumTypeNode::new(variants);
            if let Some(size) = opt_str(idl_type, "size") {
                node.size = NumberTypeNode::le(number_format(size)?).into();
            }
            Ok(node.into())
        }
        Some("alias") => type_node_from_anchor(
            idl_type
                .get("value")
                .ok_or_else(|| AnchorIdlError::MissingField("value".into()))?,
        ),
        _ => Err(AnchorIdlError::UnrecognizedType(idl_type.to_string())),
    }
}

fn enum_variant_from_anchor(variant: &Value) -> Result<EnumVariantTypeNode> {
    let name = req_str(variant, "name")?;
    let fields = arr(variant, "fields");
    if fields.is_empty() {
        return Ok(EnumEmptyVariantTypeNode::new(name).into());
    }
    if fields
        .first()
        .map(|f| f.get("name").is_some())
        .unwrap_or(false)
    {
        let fields = fields
            .iter()
            .map(|f| {
                Ok(StructFieldTypeNode::new(
                    req_str(f, "name")?,
                    type_node_from_anchor(
                        f.get("type")
                            .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
                    )?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(EnumStructVariantTypeNode::new(name, StructTypeNode::new(fields)).into());
    }
    let items = fields
        .iter()
        .map(type_node_from_anchor)
        .collect::<Result<Vec<_>>>()?;
    Ok(EnumTupleVariantTypeNode::new(name, TupleTypeNode::new(items)).into())
}
