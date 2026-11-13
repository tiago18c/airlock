//! An IDL as a tree to browse: instructions, accounts, types, errors.

use super::*;

/// Pretty JSON when the body is JSON. Otherwise the file text as stored.
pub fn pretty_idl(bytes: &[u8]) -> String {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(value) => serde_json::to_string_pretty(&value)
            .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned()),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlOutline {
    pub name: String,
    pub version: String,
    pub instructions: Vec<IdlIxOutline>,
    pub accounts: Vec<IdlNamed>,
    pub types: Vec<IdlNamed>,
    pub pdas: Vec<IdlPdaOutline>,
    pub events: Vec<IdlNamed>,
    pub errors: Vec<IdlErrorOutline>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlIxOutline {
    pub name: String,
    pub docs: String,
    pub accounts: Vec<IdlAccountUse>,
    pub arguments: Vec<IdlArgOutline>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlAccountUse {
    pub name: String,
    pub writable: bool,
    pub signer: String,
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlArgOutline {
    pub name: String,
    pub ty: String,
    /// The IDL gives it a value the caller cannot change: a default with
    /// the `omitted` strategy, as a discriminator has.
    pub fixed: bool,
    /// That value in words, when it is a plain number, boolean, string or
    /// bytes.
    pub fixed_value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlNamed {
    pub name: String,
    pub detail: String,
    /// What it holds, each with its type: an account's or a struct's
    /// fields, or an enum's variants (the type of one that carries data).
    pub fields: Vec<IdlFieldOutline>,
    /// Whether `fields` are an enum's variants rather than fields.
    pub variants: bool,
    /// Its size in bytes, when the IDL gives one.
    pub size: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlFieldOutline {
    pub name: String,
    pub ty: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlPdaOutline {
    pub name: String,
    pub seeds: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlErrorOutline {
    pub name: String,
    pub code: u64,
    pub message: String,
}

/// Walk the Codama tree for a stored IDL. `program_id` is the catalog key,
/// which legacy Anchor files need when they do not name an address.
pub fn outline_idl(bytes: &[u8], program_id: Pubkey) -> Result<IdlOutline, IdlError> {
    let (client, _, _) = open_client(bytes, Some(program_id))?;
    outline_client(&client)
}

impl<S: VaultStore, B: IdlBodies> IdlStore<S, B> {
    /// The outline of one stored program, a companion included.
    pub fn outline(&self, program_id: &Pubkey) -> Result<IdlOutline, IdlError> {
        if self.host_of(program_id).is_none() {
            let body = self
                .body(program_id)
                .ok_or_else(|| IdlError::Invalid("The IDL file is missing.".into()))?;
            return outline_idl(body, *program_id);
        }
        let client = self
            .program_client(program_id)
            .ok_or_else(|| IdlError::Invalid("The IDL file is missing.".into()))?;
        outline_client(&client)
    }
}

/// The name and message the IDL of `program_id` gives its error `code`.
pub fn program_error_words(
    catalog: &(impl IdlCatalog + ?Sized),
    program_id: &Pubkey,
    code: u32,
) -> Option<(String, String)> {
    let client = catalog.program_client(program_id)?;
    let error = client
        .root()
        .program
        .errors
        .iter()
        .find(|error| error.code == code)?;
    Some((error.name.to_string(), error.message.clone()))
}

fn outline_client(client: &ProgramClient) -> Result<IdlOutline, IdlError> {
    let value =
        serde_json::to_value(client.root()).map_err(|err| IdlError::Invalid(err.to_string()))?;
    let program = value
        .get("program")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(IdlOutline {
        name: text_at(&program, "name"),
        version: text_at(&program, "version"),
        instructions: array_at(&program, "instructions")
            .iter()
            .map(instruction_outline)
            .collect(),
        accounts: array_at(&program, "accounts")
            .iter()
            .map(account_outline)
            .collect(),
        types: array_at(&program, "definedTypes")
            .iter()
            .map(type_outline)
            .collect(),
        pdas: array_at(&program, "pdas").iter().map(pda_outline).collect(),
        events: array_at(&program, "events")
            .iter()
            .map(|node| IdlNamed {
                name: text_at(node, "name"),
                detail: String::new(),
                fields: Vec::new(),
                variants: false,
                size: None,
            })
            .collect(),
        errors: array_at(&program, "errors")
            .iter()
            .map(|node| IdlErrorOutline {
                name: text_at(node, "name"),
                code: node
                    .get("code")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                message: text_at(node, "message"),
            })
            .collect(),
    })
}

pub(super) fn instruction_outline(node: &serde_json::Value) -> IdlIxOutline {
    let mut arguments = array_at(node, "arguments");
    arguments.extend(array_at(node, "extraArguments"));
    IdlIxOutline {
        name: text_at(node, "name"),
        docs: docs_text(node.get("docs")),
        accounts: array_at(node, "accounts")
            .iter()
            .map(|account| IdlAccountUse {
                name: text_at(account, "name"),
                writable: account
                    .get("isWritable")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                signer: signer_text(account.get("isSigner")),
                optional: account
                    .get("isOptional")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
            .collect(),
        arguments: arguments.iter().map(argument_outline).collect(),
    }
}

pub(super) fn argument_outline(arg: &serde_json::Value) -> IdlArgOutline {
    let default = arg.get("defaultValue").filter(|value| !value.is_null());
    let fixed = default.is_some() && text_at(arg, "defaultValueStrategy") == "omitted";
    IdlArgOutline {
        name: text_at(arg, "name"),
        ty: type_text(arg.get("type").unwrap_or(&serde_json::Value::Null)),
        fixed,
        fixed_value: default.filter(|_| fixed).and_then(value_text),
    }
}

/// A plain value in words: a number, a boolean, a string, or bytes (base16
/// as `0x…`). `None` for anything else, such as a value read from another
/// account or argument.
pub(super) fn value_text(value: &serde_json::Value) -> Option<String> {
    match value.get("kind").and_then(serde_json::Value::as_str)? {
        "numberValueNode" => value.get("number").map(serde_json::Value::to_string),
        "booleanValueNode" => value
            .get("boolean")
            .and_then(serde_json::Value::as_bool)
            .map(|on| on.to_string()),
        "stringValueNode" => value
            .get("string")
            .and_then(serde_json::Value::as_str)
            .map(|text| format!("{text:?}")),
        "bytesValueNode" => {
            let data = value.get("data").and_then(serde_json::Value::as_str)?;
            match text_at(value, "encoding").as_str() {
                "base16" => Some(format!("0x{data}")),
                "" => Some(data.to_string()),
                encoding => Some(format!("{data} ({encoding})")),
            }
        }
        _ => None,
    }
}

pub(super) fn account_outline(node: &serde_json::Value) -> IdlNamed {
    let fields = node.get("data").map(struct_fields).unwrap_or_default();
    let size = node.get("size").and_then(serde_json::Value::as_u64);
    let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
    let mut detail = names.join(", ");
    if let Some(size) = size {
        let size = format!("{size} bytes");
        detail = if detail.is_empty() {
            size
        } else {
            format!("{detail} · {size}")
        };
    }
    IdlNamed {
        name: text_at(node, "name"),
        detail,
        fields,
        variants: false,
        size,
    }
}

/// A defined type: what kind it is in a word, then a struct's fields or an
/// enum's variants.
pub(super) fn type_outline(node: &serde_json::Value) -> IdlNamed {
    let ty = node.get("type").unwrap_or(node);
    let variants = text_at(ty, "kind") == "enumTypeNode";
    let fields = if variants {
        array_at(ty, "variants")
            .iter()
            .map(|variant| IdlFieldOutline {
                name: text_at(variant, "name"),
                ty: match text_at(variant, "kind").as_str() {
                    "enumStructVariantTypeNode" => "struct".into(),
                    "enumTupleVariantTypeNode" => "tuple".into(),
                    _ => String::new(),
                },
            })
            .filter(|variant| !variant.name.is_empty())
            .collect()
    } else {
        struct_fields(ty)
    };
    IdlNamed {
        name: text_at(node, "name"),
        detail: type_text(ty),
        fields,
        variants,
        size: None,
    }
}

/// The named fields of a struct type node, each with its type.
fn struct_fields(node: &serde_json::Value) -> Vec<IdlFieldOutline> {
    array_at(node, "fields")
        .iter()
        .map(|field| IdlFieldOutline {
            name: text_at(field, "name"),
            ty: type_text(field.get("type").unwrap_or(&serde_json::Value::Null)),
        })
        .filter(|field| !field.name.is_empty())
        .collect()
}

pub(super) fn pda_outline(node: &serde_json::Value) -> IdlPdaOutline {
    IdlPdaOutline {
        name: text_at(node, "name"),
        seeds: array_at(node, "seeds")
            .iter()
            .map(|seed| {
                let name = text_at(seed, "name");
                if !name.is_empty() {
                    name
                } else {
                    text_at(seed, "kind")
                }
            })
            .collect(),
    }
}

pub(super) fn array_at(node: &serde_json::Value, key: &str) -> Vec<serde_json::Value> {
    node.get(key)
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn text_at(node: &serde_json::Value, key: &str) -> String {
    node.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

pub(super) fn docs_text(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::Array(lines)) => lines
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        Some(serde_json::Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

pub(super) fn signer_text(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::Bool(true)) => "signer".into(),
        Some(serde_json::Value::Bool(false)) => "not a signer".into(),
        Some(serde_json::Value::String(text)) => text.clone(),
        _ => "not a signer".into(),
    }
}

pub(super) fn type_text(value: &serde_json::Value) -> String {
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    match kind {
        "publicKeyTypeNode" => "pubkey".into(),
        "booleanTypeNode" => "bool".into(),
        "stringTypeNode" => "string".into(),
        "bytesTypeNode" => "bytes".into(),
        "numberTypeNode" => {
            let format = text_at(value, "format");
            let endian = text_at(value, "endian");
            // A byte has no byte order, whatever the IDL says.
            if endian.is_empty() || matches!(format.as_str(), "u8" | "i8") {
                format
            } else {
                format!("{format} {endian}")
            }
        }
        "optionTypeNode" | "zeroableOptionTypeNode" => format!(
            "option<{}>",
            type_text(value.get("item").unwrap_or(&serde_json::Value::Null))
        ),
        "arrayTypeNode" => format!(
            "array<{}>",
            type_text(value.get("item").unwrap_or(&serde_json::Value::Null))
        ),
        "definedTypeLinkNode" => {
            let name = text_at(value, "name");
            if name.is_empty() {
                "defined".into()
            } else {
                name
            }
        }
        "amountTypeNode" | "solAmountTypeNode" => "amount".into(),
        "" => String::new(),
        other => other
            .trim_end_matches("TypeNode")
            .trim_end_matches("Node")
            .to_string(),
    }
}
