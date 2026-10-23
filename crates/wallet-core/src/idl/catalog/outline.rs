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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlNamed {
    pub name: String,
    pub detail: String,
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
            .map(|node| IdlNamed {
                name: text_at(node, "name"),
                detail: type_text(node.get("type").unwrap_or(node)),
            })
            .collect(),
        pdas: array_at(&program, "pdas").iter().map(pda_outline).collect(),
        events: array_at(&program, "events")
            .iter()
            .map(|node| IdlNamed {
                name: text_at(node, "name"),
                detail: String::new(),
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
        arguments: arguments
            .iter()
            .map(|arg| IdlArgOutline {
                name: text_at(arg, "name"),
                ty: type_text(arg.get("type").unwrap_or(&serde_json::Value::Null)),
            })
            .collect(),
    }
}

pub(super) fn account_outline(node: &serde_json::Value) -> IdlNamed {
    let fields: Vec<String> = node
        .get("data")
        .map(|data| array_at(data, "fields"))
        .unwrap_or_default()
        .iter()
        .map(|field| text_at(field, "name"))
        .filter(|name| !name.is_empty())
        .collect();
    let mut detail = fields.join(", ");
    if let Some(size) = node.get("size").and_then(serde_json::Value::as_u64) {
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
    }
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
            if endian.is_empty() {
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
