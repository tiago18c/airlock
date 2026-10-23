//! Converter for the new Anchor IDL spec (0.30+, `metadata.spec == "0.1.0"`).

use crate::helpers::{arr, byte_array, docs, flag, opt_str, req_str};
use crate::{AnchorIdlError, Result};
use codama_nodes::{
    AccountNode, AccountValueNode, ArgumentValueNode, ArrayTypeNode, BooleanTypeNode,
    BooleanValueNode, BytesTypeNode, BytesValueNode, CamelCaseString, ConstantDiscriminatorNode,
    ConstantNode, ConstantPdaSeedNode, ConstantValueNode, DefaultValueStrategy,
    DefinedTypeLinkNode, DefinedTypeNode, Docs, EnumEmptyVariantTypeNode,
    EnumStructVariantTypeNode, EnumTupleVariantTypeNode, EnumTypeNode, EnumVariantTypeNode,
    ErrorNode, EventNode, FieldDiscriminatorNode, FixedCountNode, FixedSizeTypeNode,
    HiddenPrefixTypeNode, InstructionAccountNode, InstructionArgumentNode, InstructionNode,
    IsSigner, NestedTypeNode, NumberFormat, NumberTypeNode, NumberValueNode, OptionTypeNode,
    PdaNode, PdaSeedNode, PdaSeedValueNode, PdaValueNode, PrefixedCountNode, ProgramNode,
    ProgramOrigin, PublicKeyTypeNode, PublicKeyValueNode, SizePrefixTypeNode, StringTypeNode,
    StringValueNode, StructFieldTypeNode, StructTypeNode, TupleTypeNode, TypeNode, ValueNode, U32,
    U8,
};
use serde_json::Value;
use std::collections::HashMap;

/// Generic-type resolution context: the generic typedefs available in the IDL
/// plus any `const`/`type` arguments bound at the current use site.
#[derive(Clone, Default)]
pub struct Generics<'a> {
    pub const_args: HashMap<String, &'a Value>,
    pub type_args: HashMap<String, &'a Value>,
    pub types: HashMap<String, &'a Value>,
}

pub fn program_node_from_anchor(idl: &Value) -> Result<ProgramNode> {
    let metadata = idl
        .get("metadata")
        .ok_or_else(|| AnchorIdlError::MissingField("metadata".into()))?;
    let name = req_str(metadata, "name")?;
    let version = opt_str(metadata, "version").unwrap_or("");
    let address = opt_str(idl, "address").unwrap_or("");

    // Split typedefs into concrete types and generic templates (monomorphized
    // at each use site rather than emitted as defined types).
    let mut generics = Generics::default();
    let mut concrete_types: Vec<&Value> = Vec::new();
    for t in arr(idl, "types") {
        let type_name = req_str(t, "name")?;
        if t.get("generics")
            .map(|g| !g.as_array().map(Vec::is_empty).unwrap_or(true))
            == Some(true)
        {
            generics.types.insert(type_name.to_string(), t);
        } else {
            concrete_types.push(t);
        }
    }

    // Types that back accounts or events become account/event data, not
    // standalone defined types.
    let account_names: Vec<&str> = arr(idl, "accounts")
        .iter()
        .filter_map(|a| opt_str(a, "name"))
        .collect();
    let event_names: Vec<&str> = arr(idl, "events")
        .iter()
        .filter_map(|e| opt_str(e, "name"))
        .collect();

    let mut program = ProgramNode::new(name, address).set_version(version);
    program.origin = Some(ProgramOrigin::Anchor);

    for t in &concrete_types {
        let type_name = req_str(t, "name")?;
        if account_names.contains(&type_name) || event_names.contains(&type_name) {
            continue;
        }
        program
            .defined_types
            .push(defined_type_node_from_anchor(t, &generics)?);
    }
    for account in arr(idl, "accounts") {
        program.accounts.push(account_node_from_anchor(
            account,
            &concrete_types,
            &generics,
        )?);
    }
    for event in arr(idl, "events") {
        program
            .events
            .push(event_node_from_anchor(event, &concrete_types, &generics)?);
    }
    for instruction in arr(idl, "instructions") {
        program
            .instructions
            .push(instruction_node_from_anchor(instruction, &generics)?);
    }
    for error in arr(idl, "errors") {
        program.errors.push(error_node_from_anchor(error)?);
    }
    for constant in arr(idl, "constants") {
        program
            .constants
            .push(constant_node_from_anchor(constant, &generics)?);
    }
    Ok(program)
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

pub fn instruction_node_from_anchor(idl: &Value, generics: &Generics) -> Result<InstructionNode> {
    let name = req_str(idl, "name")?;

    let mut arguments = Vec::new();
    if let Some(disc) = idl.get("discriminator").and_then(byte_array) {
        arguments.push(discriminator_argument(&disc));
    }
    for arg in arr(idl, "args") {
        arguments.push(instruction_argument_from_anchor(arg, generics)?);
    }

    let accounts = instruction_account_nodes(arr(idl, "accounts"), &arguments, generics)?;

    let discriminators = if idl.get("discriminator").is_some() {
        vec![FieldDiscriminatorNode::new("discriminator", 0).into()]
    } else {
        vec![]
    };

    Ok(InstructionNode {
        name: name.into(),
        docs: docs(idl).into(),
        accounts,
        arguments,
        discriminators,
        ..InstructionNode::default()
    })
}

/// The prepended, omitted `discriminator` argument carrying the explicit
/// discriminator bytes as a base16 default value.
fn discriminator_argument(bytes: &[u8]) -> InstructionArgumentNode {
    InstructionArgumentNode {
        name: "discriminator".into(),
        default_value_strategy: Some(DefaultValueStrategy::Omitted),
        docs: Docs::default(),
        r#type: Box::new(FixedSizeTypeNode::new(BytesTypeNode::new(), bytes.len()).into()),
        default_value: Box::new(Some(BytesValueNode::base16(hex::encode(bytes)).into())),
        display: None,
    }
}

fn instruction_argument_from_anchor(
    idl: &Value,
    generics: &Generics,
) -> Result<InstructionArgumentNode> {
    let mut node = InstructionArgumentNode::new(
        req_str(idl, "name")?,
        type_node_from_anchor(
            idl.get("type")
                .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
            generics,
        )?,
    );
    node.docs = docs(idl).into();
    Ok(node)
}

/// Flattens (possibly grouped) instruction accounts. Group names become `_`
/// prefixes only when the flattened leaf names would otherwise collide,
/// mirroring the TS converter.
fn instruction_account_nodes(
    items: &[Value],
    arguments: &[InstructionArgumentNode],
    generics: &Generics,
) -> Result<Vec<InstructionAccountNode>> {
    let mut leaf_names = Vec::new();
    collect_leaf_names(items, &mut leaf_names)?;
    let normalized: Vec<CamelCaseString> = leaf_names.iter().map(|n| n.as_str().into()).collect();
    let has_duplicates =
        (1..normalized.len()).any(|i| normalized[i..].contains(&normalized[i - 1]));

    let mut out = Vec::new();
    flatten_accounts(items, arguments, generics, "", has_duplicates, &mut out)?;
    Ok(out)
}

fn collect_leaf_names(items: &[Value], out: &mut Vec<String>) -> Result<()> {
    for item in items {
        match item.get("accounts").and_then(Value::as_array) {
            Some(nested) => collect_leaf_names(nested, out)?,
            None => out.push(req_str(item, "name")?.to_string()),
        }
    }
    Ok(())
}

fn flatten_accounts(
    items: &[Value],
    arguments: &[InstructionArgumentNode],
    generics: &Generics,
    prefix: &str,
    use_prefix: bool,
    out: &mut Vec<InstructionAccountNode>,
) -> Result<()> {
    for item in items {
        let name = req_str(item, "name")?;
        match item.get("accounts").and_then(Value::as_array) {
            Some(nested) => {
                let sub_prefix = if use_prefix {
                    join_prefix(prefix, name)
                } else {
                    String::new()
                };
                flatten_accounts(nested, arguments, generics, &sub_prefix, use_prefix, out)?;
            }
            None => out.push(instruction_account_from_anchor(
                item, arguments, generics, prefix,
            )?),
        }
    }
    Ok(())
}

fn join_prefix(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}_{name}")
    }
}

fn instruction_account_from_anchor(
    idl: &Value,
    arguments: &[InstructionArgumentNode],
    generics: &Generics,
    prefix: &str,
) -> Result<InstructionAccountNode> {
    let name = join_prefix(prefix, req_str(idl, "name")?);
    let mut node = InstructionAccountNode::new(
        name,
        flag(idl, "writable"),
        IsSigner::from(flag(idl, "signer")),
    );
    node.is_optional = Some(flag(idl, "optional"));
    node.docs = docs(idl).into();

    if let Some(address) = opt_str(idl, "address") {
        node.default_value = Box::new(Some(PublicKeyValueNode::new(address).into()));
    } else if let Some(pda) = idl.get("pda") {
        if let Some(default) = pda_default_value(pda, &node.name, arguments, generics, prefix)? {
            node.default_value = Box::new(Some(default.into()));
        }
    }
    Ok(node)
}

// ---------------------------------------------------------------------------
// PDA seeds (inline on instruction accounts)
// ---------------------------------------------------------------------------

/// Builds an inline `pdaValueNode` default for an account with a `pda` field.
/// Returns `None` (gracefully skipping the default) when a seed uses a nested
/// path (`a.b`), which cannot be represented.
fn pda_default_value(
    pda: &Value,
    account_name: &CamelCaseString,
    arguments: &[InstructionArgumentNode],
    generics: &Generics,
    prefix: &str,
) -> Result<Option<PdaValueNode>> {
    let seeds = arr(pda, "seeds");
    let nested_path = |seed: &Value| {
        opt_str(seed, "path")
            .map(|p| p.contains('.'))
            .unwrap_or(false)
    };
    if seeds.iter().any(nested_path) || pda.get("program").map(nested_path).unwrap_or(false) {
        return Ok(None);
    }

    let mut definitions = Vec::with_capacity(seeds.len());
    let mut values = Vec::new();
    for seed in seeds {
        let (definition, value) = pda_seed_from_anchor(seed, arguments, generics, prefix)?;
        definitions.push(definition);
        values.extend(value);
    }

    let mut pda_node = PdaNode::new(account_name.clone(), definitions);
    let mut program_value: Option<PdaSeedValueNode> = None;
    if let Some(program) = pda.get("program") {
        let (definition, value) = pda_seed_from_anchor(program, arguments, generics, prefix)?;
        match &definition {
            // A constant program seed holds the program address bytes.
            PdaSeedNode::Constant(seed)
                if matches!(
                    seed.value.as_ref(),
                    codama_nodes::ConstantPdaSeedValue::Bytes(bytes)
                        if bytes.encoding == codama_nodes::BytesEncoding::Base58
                ) =>
            {
                if let codama_nodes::ConstantPdaSeedValue::Bytes(bytes) = seed.value.as_ref() {
                    pda_node.program_id = Some(bytes.data.clone());
                }
            }
            _ => program_value = value,
        }
    }

    Ok(Some(match program_value {
        Some(value) => match value.value.as_ref() {
            codama_nodes::PdaSeedValueValue::Account(account) => {
                PdaValueNode::new_with_program_id(pda_node, values, account.clone())
            }
            codama_nodes::PdaSeedValueValue::Argument(argument) => {
                PdaValueNode::new_with_program_id(pda_node, values, argument.clone())
            }
            _ => PdaValueNode::new(pda_node, values),
        },
        None => PdaValueNode::new(pda_node, values),
    }))
}

/// Converts one v01 seed into its definition node and (for variable seeds) the
/// `pdaSeedValueNode` binding it to an account or argument.
fn pda_seed_from_anchor(
    seed: &Value,
    arguments: &[InstructionArgumentNode],
    _generics: &Generics,
    prefix: &str,
) -> Result<(PdaSeedNode, Option<PdaSeedValueNode>)> {
    match opt_str(seed, "kind") {
        Some("const") => {
            let bytes = seed
                .get("value")
                .and_then(byte_array)
                .ok_or_else(|| AnchorIdlError::MissingField("value".into()))?;
            let definition = ConstantPdaSeedNode::new(
                BytesTypeNode::new(),
                BytesValueNode::base58(bs58::encode(&bytes).into_string()),
            );
            Ok((definition.into(), None))
        }
        Some("account") => {
            let path = req_str(seed, "path")?;
            let name = join_prefix(prefix, path.split('.').next().unwrap_or(path));
            let definition = VariableSeed::public_key(&name);
            let value = PdaSeedValueNode::new(name.as_str(), AccountValueNode::new(name.as_str()));
            Ok((definition, Some(value)))
        }
        Some("arg") => {
            let path = req_str(seed, "path")?;
            let arg_name: CamelCaseString = path.split('.').next().unwrap_or(path).into();
            let argument = arguments
                .iter()
                .find(|a| a.name == arg_name)
                .ok_or_else(|| AnchorIdlError::ArgumentMissing(arg_name.to_string()))?;
            // Anchor seeds borsh strings without their length prefix.
            let seed_type = match argument.r#type.as_ref() {
                TypeNode::SizePrefix(sp) if matches!(sp.r#type.as_ref(), TypeNode::String(_)) => {
                    sp.r#type.as_ref().clone()
                }
                other => other.clone(),
            };
            let definition = codama_nodes::VariablePdaSeedNode::new(arg_name.clone(), seed_type);
            let value =
                PdaSeedValueNode::new(arg_name.clone(), ArgumentValueNode::new(arg_name.clone()));
            Ok((definition.into(), Some(value)))
        }
        other => Err(AnchorIdlError::SeedKindUnimplemented(
            other.unwrap_or("<missing>").to_string(),
        )),
    }
}

/// Small helper for the common `variablePdaSeedNode(name, publicKeyTypeNode())`.
struct VariableSeed;
impl VariableSeed {
    fn public_key(name: &str) -> PdaSeedNode {
        codama_nodes::VariablePdaSeedNode::new(name, PublicKeyTypeNode::new()).into()
    }
}

// ---------------------------------------------------------------------------
// Accounts, events, defined types, errors, constants
// ---------------------------------------------------------------------------

pub fn account_node_from_anchor(
    idl: &Value,
    types: &[&Value],
    generics: &Generics,
) -> Result<AccountNode> {
    let name = req_str(idl, "name")?;
    let type_def = types
        .iter()
        .find(|t| opt_str(t, "name") == Some(name))
        .ok_or_else(|| AnchorIdlError::AccountTypeMissing(name.to_string()))?;
    let data = *defined_type_node_from_anchor(type_def, generics)?.r#type;
    let TypeNode::Struct(mut data) = data else {
        return Err(AnchorIdlError::AccountDataNotAStruct(name.to_string()));
    };

    let mut node = AccountNode::new(name, NestedTypeNode::Value(StructTypeNode::new(vec![])));
    if let Some(disc) = idl.get("discriminator").and_then(byte_array) {
        data.fields.insert(0, discriminator_field(&disc));
        node.discriminators = vec![FieldDiscriminatorNode::new("discriminator", 0).into()];
    }
    node.data = NestedTypeNode::Value(data);
    Ok(node)
}

fn discriminator_field(bytes: &[u8]) -> StructFieldTypeNode {
    StructFieldTypeNode {
        name: "discriminator".into(),
        default_value_strategy: Some(DefaultValueStrategy::Omitted),
        docs: Docs::default(),
        r#type: Box::new(FixedSizeTypeNode::new(BytesTypeNode::new(), bytes.len()).into()),
        default_value: Box::new(Some(BytesValueNode::base16(hex::encode(bytes)).into())),
        display: None,
    }
}

pub fn event_node_from_anchor(
    idl: &Value,
    types: &[&Value],
    generics: &Generics,
) -> Result<EventNode> {
    let name = req_str(idl, "name")?;
    let type_def = types
        .iter()
        .find(|t| opt_str(t, "name") == Some(name))
        .ok_or_else(|| AnchorIdlError::EventTypeMissing(name.to_string()))?;
    let data = *defined_type_node_from_anchor(type_def, generics)?.r#type;

    let mut node = EventNode::new(name, data.clone());
    if let Some(disc) = idl.get("discriminator").and_then(byte_array) {
        let constant = ConstantValueNode::new(
            FixedSizeTypeNode::new(BytesTypeNode::new(), disc.len()),
            BytesValueNode::base16(hex::encode(&disc)),
        );
        node.data =
            Box::new(HiddenPrefixTypeNode::<TypeNode>::new(data, vec![constant.clone()]).into());
        node.discriminators = vec![ConstantDiscriminatorNode::new(constant, 0).into()];
    }
    Ok(node)
}

pub fn defined_type_node_from_anchor(idl: &Value, generics: &Generics) -> Result<DefinedTypeNode> {
    let mut node = DefinedTypeNode::new(
        opt_str(idl, "name").unwrap_or(""),
        type_node_from_anchor(idl.get("type").unwrap_or(&Value::Null), generics)?,
    );
    node.docs = docs(idl).into();
    Ok(node)
}

pub fn error_node_from_anchor(idl: &Value) -> Result<ErrorNode> {
    let name = req_str(idl, "name")?;
    let message = opt_str(idl, "msg").unwrap_or("");
    let code = idl.get("code").and_then(Value::as_u64).unwrap_or(0) as u32;
    let mut node = ErrorNode::new(name, code, message);
    node.docs = vec![format!("{name}: {message}")].into();
    Ok(node)
}

pub fn constant_node_from_anchor(idl: &Value, generics: &Generics) -> Result<ConstantNode> {
    let name = req_str(idl, "name")?;
    let declared: TypeNode = match idl.get("type") {
        // Constants use a bare (non size-prefixed) bytes type.
        Some(Value::String(s)) if s == "bytes" => BytesTypeNode::new().into(),
        Some(t) => type_node_from_anchor(t, generics)?,
        None => StringTypeNode::utf8().into(),
    };
    let raw = opt_str(idl, "value").unwrap_or("");
    let (r#type, value) = parse_constant_value(declared, raw);
    Ok(ConstantNode::new(name, r#type, value))
}

/// Parses a v01 constant's string value against its declared type, falling
/// back to a UTF-8 string constant when the value does not match.
fn parse_constant_value(declared: TypeNode, raw: &str) -> (TypeNode, ValueNode) {
    let string_fallback = || {
        (
            TypeNode::from(StringTypeNode::utf8()),
            ValueNode::from(StringValueNode::new(raw)),
        )
    };
    match &declared {
        TypeNode::Bytes(_) => match serde_json::from_str::<Vec<u8>>(raw) {
            Ok(bytes) => (declared, BytesValueNode::base16(hex::encode(bytes)).into()),
            Err(_) => string_fallback(),
        },
        TypeNode::Number(n) => {
            let is_float = matches!(n.format, NumberFormat::F32 | NumberFormat::F64);
            let valid = if is_float {
                raw.parse::<f64>().is_ok()
            } else {
                raw.parse::<i128>().is_ok()
            };
            match (valid, raw.parse::<f64>()) {
                (true, Ok(number)) => (declared, NumberValueNode::new(number).into()),
                _ => string_fallback(),
            }
        }
        TypeNode::Boolean(_) => match raw {
            "true" => (declared, BooleanValueNode::new(true).into()),
            "false" => (declared, BooleanValueNode::new(false).into()),
            _ => string_fallback(),
        },
        TypeNode::PublicKey(_) => (declared, PublicKeyValueNode::new(raw).into()),
        _ => (declared, StringValueNode::new(raw).into()),
    }
}

// ---------------------------------------------------------------------------
// Type mapping
// ---------------------------------------------------------------------------

pub fn type_node_from_anchor(idl_type: &Value, generics: &Generics) -> Result<TypeNode> {
    match idl_type {
        Value::String(leaf) => leaf_type_node(leaf),
        Value::Object(_) => object_type_node(idl_type, generics),
        other => Err(AnchorIdlError::UnrecognizedType(other.to_string())),
    }
}

fn leaf_type_node(leaf: &str) -> Result<TypeNode> {
    Ok(match leaf {
        "bool" => BooleanTypeNode::new(NumberTypeNode::le(U8)).into(),
        "pubkey" => PublicKeyTypeNode::new().into(),
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

fn object_type_node(idl_type: &Value, generics: &Generics) -> Result<TypeNode> {
    if let Some(array) = idl_type.get("array").and_then(Value::as_array) {
        if array.len() == 2 {
            let item = type_node_from_anchor(&array[0], generics)?;
            let count = match &array[1] {
                Value::Number(n) => FixedCountNode::new(n.as_u64().unwrap_or(0)),
                len => FixedCountNode::new(resolve_const_generic(len, generics)?),
            };
            return Ok(ArrayTypeNode::new(item, count).into());
        }
    }
    if let Some(item) = idl_type.get("vec") {
        let item = type_node_from_anchor(item, generics)?;
        return Ok(
            ArrayTypeNode::new(item, PrefixedCountNode::new(NumberTypeNode::le(U32))).into(),
        );
    }
    if let Some(defined) = idl_type.get("defined") {
        return defined_type_link(defined, generics);
    }
    if let Some(generic) = idl_type.get("generic").and_then(Value::as_str) {
        let bound = generics
            .type_args
            .get(generic)
            .ok_or_else(|| AnchorIdlError::GenericTypeMissing(generic.to_string()))?;
        let inner = bound.get("type").unwrap_or(bound);
        return type_node_from_anchor(inner, generics);
    }
    if let Some(item) = idl_type.get("option") {
        return Ok(OptionTypeNode::new(type_node_from_anchor(item, generics)?).into());
    }
    if let Some(item) = idl_type.get("coption") {
        let mut node = OptionTypeNode::fixed(type_node_from_anchor(item, generics)?);
        node.prefix = NumberTypeNode::le(U32).into();
        return Ok(node.into());
    }
    match opt_str(idl_type, "kind") {
        Some("struct") => struct_or_tuple_type_node(arr(idl_type, "fields"), generics),
        Some("enum") => enum_type_node(arr(idl_type, "variants"), generics),
        Some("alias") => type_node_from_anchor(
            idl_type
                .get("value")
                .ok_or_else(|| AnchorIdlError::MissingField("value".into()))?,
            generics,
        ),
        _ => Err(AnchorIdlError::UnrecognizedType(idl_type.to_string())),
    }
}

/// A const generic used as an array length: `{"generic": "N"}`.
fn resolve_const_generic(len: &Value, generics: &Generics) -> Result<u64> {
    let name =
        opt_str(len, "generic").ok_or_else(|| AnchorIdlError::UnrecognizedType(len.to_string()))?;
    let bound = generics
        .const_args
        .get(name)
        .ok_or_else(|| AnchorIdlError::GenericTypeMissing(name.to_string()))?;
    opt_str(bound, "value")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| AnchorIdlError::GenericTypeMissing(name.to_string()))
}

/// `{"defined": {"name": ..., "generics": [...]?}}` — a link node for plain
/// types, or an inlined monomorphization for generic ones.
fn defined_type_link(defined: &Value, generics: &Generics) -> Result<TypeNode> {
    // v01 uses object form; tolerate a bare string for robustness.
    let (name, args) = match defined {
        Value::String(s) => (s.as_str(), &[][..]),
        _ => (req_str(defined, "name")?, arr(defined, "generics")),
    };
    if args.is_empty() && !generics.types.contains_key(name) {
        return Ok(DefinedTypeLinkNode::new(name).into());
    }

    // Monomorphize: bind the use-site args to the template's declarations.
    let template = generics
        .types
        .get(name)
        .copied()
        .ok_or_else(|| AnchorIdlError::GenericTypeMissing(name.to_string()))?;
    let declarations = arr(template, "generics");
    let mut bound = generics.clone();
    for (declaration, arg) in declarations.iter().zip(args) {
        let generic_name = req_str(declaration, "name")?.to_string();
        if opt_str(declaration, "kind") == Some("const") {
            bound.const_args.insert(generic_name, arg);
        } else {
            bound.type_args.insert(generic_name, arg);
        }
    }
    type_node_from_anchor(
        template
            .get("type")
            .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
        &bound,
    )
}

fn struct_or_tuple_type_node(fields: &[Value], generics: &Generics) -> Result<TypeNode> {
    // Named fields → struct; bare types → tuple.
    let is_tuple = fields
        .first()
        .map(|f| f.get("name").is_none())
        .unwrap_or(false);
    if is_tuple {
        let items = fields
            .iter()
            .map(|f| type_node_from_anchor(f, generics))
            .collect::<Result<Vec<_>>>()?;
        return Ok(TupleTypeNode::new(items).into());
    }
    let fields = fields
        .iter()
        .map(|f| struct_field_from_anchor(f, generics))
        .collect::<Result<Vec<_>>>()?;
    Ok(StructTypeNode::new(fields).into())
}

fn struct_field_from_anchor(field: &Value, generics: &Generics) -> Result<StructFieldTypeNode> {
    let mut node = StructFieldTypeNode::new(
        req_str(field, "name")?,
        type_node_from_anchor(
            field
                .get("type")
                .ok_or_else(|| AnchorIdlError::MissingField("type".into()))?,
            generics,
        )?,
    );
    node.docs = docs(field).into();
    Ok(node)
}

fn enum_type_node(variants: &[Value], generics: &Generics) -> Result<TypeNode> {
    let variants = variants
        .iter()
        .map(|v| enum_variant_from_anchor(v, generics))
        .collect::<Result<Vec<_>>>()?;
    Ok(EnumTypeNode::new(variants).into())
}

fn enum_variant_from_anchor(variant: &Value, generics: &Generics) -> Result<EnumVariantTypeNode> {
    let name = req_str(variant, "name")?;
    let fields = arr(variant, "fields");
    if fields.is_empty() {
        return Ok(EnumEmptyVariantTypeNode::new(name).into());
    }
    match struct_or_tuple_type_node(fields, generics)? {
        TypeNode::Struct(fields) => Ok(EnumStructVariantTypeNode::new(name, fields).into()),
        TypeNode::Tuple(items) => Ok(EnumTupleVariantTypeNode::new(name, items).into()),
        _ => unreachable!("struct_or_tuple_type_node returns struct or tuple"),
    }
}
