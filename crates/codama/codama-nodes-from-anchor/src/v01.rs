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
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

/// How deep a converted type may nest, generic templates expanded. A file is
/// held to about this depth by the reader's own limit on its text; a generic
/// template expands in place, so without this a few lines could nest without
/// end.
const MAX_TYPE_DEPTH: usize = 32;

/// How many times generic templates may be expanded in one IDL. A template
/// that uses another twice doubles the tree at each level.
const MAX_GENERIC_EXPANSIONS: usize = 4096;

/// How many JSON values the arguments of generic types may hold in one IDL,
/// once the generics of the scope they are written in are filled in. An
/// argument that repeats its own parameter doubles at each level.
const MAX_GENERIC_ARGUMENT_NODES: usize = 1 << 18;

/// How many types generic templates may expand to in one IDL, every field
/// and item counted. The number of expansions is bounded, but not what one
/// makes: a template of a thousand plain fields, used four thousand times,
/// is a file of a few hundred kilobytes that asks for more than a gigabyte.
const MAX_GENERIC_EXPANDED_NODES: usize = 1 << 17;

/// What one conversion has spent of the limits above, shared by every scope.
#[derive(Default)]
struct Spent {
    depth: Cell<usize>,
    expansions: Cell<usize>,
    argument_nodes: Cell<usize>,
    expanded_nodes: Cell<usize>,
}

/// One level of type nesting, given back when the level is left.
struct Level<'s>(&'s Cell<usize>);

impl Drop for Level<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

/// Generic-type resolution context: the generic typedefs available in the IDL
/// plus any `const`/`type` arguments bound at the current use site.
///
/// A bound type argument holds no generic of its own: the generics of the
/// scope it was written in are filled in when it is bound, so it means the
/// same wherever the template uses it. Left as written, `Price<T>` inside
/// `Prices<T>` would bind `T` to "the type `T`" and never resolve.
#[derive(Clone, Default)]
pub struct Generics<'a> {
    const_args: HashMap<String, u64>,
    type_args: HashMap<String, Value>,
    types: Rc<HashMap<String, &'a Value>>,
    spent: Rc<Spent>,
    /// The template this scope expands, when it expands one: what is built
    /// in it is counted against [`MAX_GENERIC_EXPANDED_NODES`].
    template: Option<Rc<str>>,
}

impl<'a> Generics<'a> {
    fn with_templates(types: HashMap<String, &'a Value>) -> Self {
        Self {
            types: Rc::new(types),
            ..Self::default()
        }
    }

    /// The scope of a template being expanded: its own arguments only.
    fn inside_a_template(&self, name: &str) -> Result<Self> {
        let expansions = self.spent.expansions.get() + 1;
        if expansions > MAX_GENERIC_EXPANSIONS {
            return Err(AnchorIdlError::GenericExpansionLimit(name.to_string()));
        }
        self.spent.expansions.set(expansions);
        Ok(Self {
            template: Some(name.into()),
            ..self.without_arguments()
        })
    }

    /// One more type built. Inside a template it counts against what the
    /// templates of one IDL may expand to.
    fn built(&self) -> Result<()> {
        let Some(template) = &self.template else {
            return Ok(());
        };
        let nodes = self.spent.expanded_nodes.get() + 1;
        if nodes > MAX_GENERIC_EXPANDED_NODES {
            return Err(AnchorIdlError::GenericExpansionLimit(template.to_string()));
        }
        self.spent.expanded_nodes.set(nodes);
        Ok(())
    }

    /// This scope with nothing bound, for a value that holds no generic.
    fn without_arguments(&self) -> Self {
        Self {
            const_args: HashMap::new(),
            type_args: HashMap::new(),
            types: self.types.clone(),
            spent: self.spent.clone(),
            template: self.template.clone(),
        }
    }

    /// One level deeper into a type, or an error past the limit.
    fn descend(&self) -> Result<Level<'_>> {
        let depth = self.spent.depth.get() + 1;
        if depth > MAX_TYPE_DEPTH {
            return Err(AnchorIdlError::TypeTooDeep(MAX_TYPE_DEPTH));
        }
        self.spent.depth.set(depth);
        Ok(Level(&self.spent.depth))
    }

    /// `value`, a type as a template or a use site writes it, with every
    /// generic this scope binds filled in. `name` is the template it is an
    /// argument of, for the error.
    fn filled_in(&self, value: &Value, name: &str) -> Result<Value> {
        let nodes = self.spent.argument_nodes.get() + 1;
        if nodes > MAX_GENERIC_ARGUMENT_NODES {
            return Err(AnchorIdlError::GenericExpansionLimit(name.to_string()));
        }
        self.spent.argument_nodes.set(nodes);
        Ok(match value {
            Value::Object(map) => {
                if let (1, Some(generic)) = (map.len(), map.get("generic").and_then(Value::as_str))
                {
                    if let Some(bound) = self.type_args.get(generic) {
                        return Ok(bound.clone());
                    }
                    // A const generic, as the length of an array.
                    if let Some(bound) = self.const_args.get(generic) {
                        return Ok(Value::from(*bound));
                    }
                }
                let mut filled = serde_json::Map::with_capacity(map.len());
                for (key, inner) in map {
                    filled.insert(key.clone(), self.filled_in(inner, name)?);
                }
                // A const argument that passes on a const generic of this scope.
                if filled.get("kind").and_then(Value::as_str) == Some("const") {
                    let passed_on = filled
                        .get("value")
                        .and_then(Value::as_str)
                        .and_then(|value| self.const_args.get(value));
                    if let Some(bound) = passed_on {
                        filled.insert("value".into(), Value::from(bound.to_string()));
                    }
                }
                Value::Object(filled)
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.filled_in(item, name))
                    .collect::<Result<_>>()?,
            ),
            leaf => leaf.clone(),
        })
    }
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
    let mut templates = HashMap::new();
    let mut concrete_types: Vec<&Value> = Vec::new();
    for t in arr(idl, "types") {
        let type_name = req_str(t, "name")?;
        if t.get("generics")
            .map(|g| !g.as_array().map(Vec::is_empty).unwrap_or(true))
            == Some(true)
        {
            templates.insert(type_name.to_string(), t);
        } else {
            concrete_types.push(t);
        }
    }
    let generics = Generics::with_templates(templates);

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
    generics.built()?;
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
    let _level = generics.descend()?;
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
        // It holds no generic, so no argument of this scope is its to read.
        return type_node_from_anchor(bound, &generics.without_arguments());
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
    generics
        .const_args
        .get(name)
        .copied()
        .ok_or_else(|| AnchorIdlError::GenericTypeMissing(name.to_string()))
}

/// The value of a const argument at a use site: a number, or a const generic
/// of the scope the use site is written in, passed on.
fn const_argument(arg: &Value, generics: &Generics) -> Option<u64> {
    let written = opt_str(arg, "value").or_else(|| {
        arg.get("type")
            .and_then(|passed_on| opt_str(passed_on, "generic"))
    })?;
    written
        .parse()
        .ok()
        .or_else(|| generics.const_args.get(written).copied())
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
    let mut bound = generics.inside_a_template(name)?;
    for (declaration, arg) in declarations.iter().zip(args) {
        let generic_name = req_str(declaration, "name")?.to_string();
        if opt_str(declaration, "kind") == Some("const") {
            let value = const_argument(arg, generics)
                .ok_or_else(|| AnchorIdlError::GenericTypeMissing(generic_name.clone()))?;
            bound.const_args.insert(generic_name, value);
        } else {
            // Read in the scope the use site is written in, not the template's.
            let written = arg.get("type").unwrap_or(arg);
            bound
                .type_args
                .insert(generic_name, generics.filled_in(written, name)?);
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
