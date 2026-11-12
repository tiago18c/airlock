//! An instruction read back onto its IDL's form.

use crate::idl::build::{
    data_variant, idl_instruction_form, scaled_text, IdlAccountSource, IdlArgKind, IdlArgScale,
    IdlVariant, STRUCT_FIELD_SEPARATOR,
};
use crate::idl::draft::IdlDraft;
use codama_dynamic_client::{NumberValue, ProgramClient, Value};
use codama_nodes::Node;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

/// The form `client` builds `instruction` from, filled in so that it builds
/// those same bytes and accounts. `None` when the IDL does not read it, or
/// when the form cannot hold it: an argument the form has no field for,
/// accounts beyond the ones it names, signer flags it would set otherwise.
pub fn idl_draft_for(
    client: &ProgramClient,
    program_name: &str,
    instruction: &Instruction,
) -> Option<IdlDraft> {
    let parsed = client.parse_instruction(instruction).ok()??;
    let name = match parsed.path.last()? {
        Node::Instruction(node) => node.name.to_string(),
        _ => return None,
    };
    let mut draft =
        IdlDraft::for_instruction(instruction.program_id, program_name, client, Some(&name));
    for account in &parsed.accounts {
        draft.set_account(&account.name, Some(account.meta.pubkey));
    }
    let form = idl_instruction_form(client, &name, &draft.input()).ok()?;
    let Value::Struct(fields) = &parsed.data else {
        return None;
    };
    for argument in &form.arguments {
        let value = match argument.name.split_once(STRUCT_FIELD_SEPARATOR) {
            Some((parent, child)) => match fields.get(parent)? {
                Value::Struct(inner) => inner.get(child)?,
                _ => return None,
            },
            None => fields.get(&argument.name)?,
        };
        // A scaled field holds the amount in its unit, as it is typed.
        let decimals = match &argument.scale {
            IdlArgScale::None => None,
            IdlArgScale::Fixed { decimals, .. } => Some(*decimals),
            IdlArgScale::FromArgument { argument, .. } => match fields.get(argument)? {
                Value::Number(NumberValue::Unsigned(n)) => Some(u8::try_from(*n).ok()?),
                Value::Number(NumberValue::Signed(n)) => Some(u8::try_from(*n).ok()?),
                _ => return None,
            },
        };
        draft.set_argument(
            &argument.name,
            argument_text(&argument.kind, value, decimals)?,
        );
    }

    // A derived account that is what the IDL derives stays derived, so it
    // follows its seeds when they are edited. One that is not is filled in.
    let full = draft.clone();
    let form = idl_instruction_form(client, &name, &draft.input()).ok()?;
    for field in &form.accounts {
        if field.source != IdlAccountSource::Derived {
            continue;
        }
        if field.address.is_some() && field.address == full.account(&field.name) {
            draft.set_account(&field.name, None);
        } else if full.account(&field.name).is_some() {
            draft.manual.push(field.name.clone());
        }
    }
    if builds_the_same(client, &draft, instruction) {
        return Some(draft);
    }
    // Every account as it came.
    let mut full = full;
    full.manual = form
        .accounts
        .iter()
        .filter(|field| field.source == IdlAccountSource::Derived)
        .filter(|field| full.account(&field.name).is_some())
        .map(|field| field.name.clone())
        .collect();
    builds_the_same(client, &full, instruction).then_some(full)
}

fn builds_the_same(client: &ProgramClient, draft: &IdlDraft, instruction: &Instruction) -> bool {
    let mut draft = draft.clone();
    draft.build(client).is_ok_and(|built| built == *instruction)
}

/// The text an argument field holds for `value`, as `parse_argument` reads it.
/// `decimals` is the field's scale: the integer is written in its unit.
fn argument_text(kind: &IdlArgKind, value: &Value, decimals: Option<u8>) -> Option<String> {
    match (kind, value) {
        (IdlArgKind::Option(_), Value::Option(None)) => Some(String::new()),
        (IdlArgKind::Option(inner), Value::Option(Some(value))) => {
            let text = argument_text(inner, value, decimals)?;
            // Blank reads as none.
            (!text.trim().is_empty()).then_some(text)
        }
        (IdlArgKind::Integer { float: false, .. }, Value::Number(number)) if decimals.is_some() => {
            scaled_text(number, decimals?)
        }
        (IdlArgKind::Integer { .. }, Value::Number(number)) => Some(match number {
            NumberValue::Unsigned(n) => n.to_string(),
            NumberValue::Signed(n) => n.to_string(),
            NumberValue::Float(n) => n.to_string(),
        }),
        (IdlArgKind::Pubkey, Value::PublicKey(bytes)) => {
            Some(Pubkey::new_from_array(*bytes).to_string())
        }
        (IdlArgKind::String, Value::String(text)) => Some(text.clone()),
        (IdlArgKind::Bool, Value::Bool(flag)) => Some(flag.to_string()),
        (IdlArgKind::Enum { variants }, Value::Enum { variant, .. }) => {
            variants.contains(variant).then(|| variant.clone())
        }
        (IdlArgKind::Bytes, Value::Bytes(bytes)) => Some(hex::encode(bytes)),
        (IdlArgKind::List(_), Value::Array(items)) if items.is_empty() => Some(String::new()),
        (IdlArgKind::List(item), Value::Array(items)) => {
            let items = items
                .iter()
                .map(|value| json_of(item, value))
                .collect::<Option<Vec<_>>>()?;
            Some(Json::List(items).to_string())
        }
        (IdlArgKind::FixedBytes { len }, Value::Array(items)) if items.len() == *len => {
            let bytes = items
                .iter()
                .map(|item| match item {
                    Value::Number(NumberValue::Unsigned(n)) => u8::try_from(*n).ok(),
                    _ => None,
                })
                .collect::<Option<Vec<u8>>>()?;
            Some(fixed_bytes_text(&bytes))
        }
        (IdlArgKind::Variants { variants }, Value::Enum { variant, fields }) if matches!(fields.as_ref(), Value::Struct(map) if map.is_empty()) => {
            variants
                .iter()
                .find(|(name, held)| *held == IdlVariant::Empty && data_variant(name) == *variant)
                .map(|(name, _)| name.clone())
        }
        (IdlArgKind::Struct { .. } | IdlArgKind::Variants { .. }, value) => {
            Some(json_of(kind, value)?.to_string())
        }
        _ => None,
    }
}

/// A fixed byte array as it is typed: its text when it is UTF-8 with no
/// control character and only zeros after it, otherwise hex after `0x`.
/// Either reads back to the same bytes.
fn fixed_bytes_text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |at| at + 1);
    match std::str::from_utf8(&bytes[..end]) {
        Ok(text)
            if !text.starts_with("0x")
                && !text.chars().any(char::is_control)
                && text.trim() == text =>
        {
            text.to_string()
        }
        _ => format!("0x{}", hex::encode(&bytes[..end])),
    }
}

/// `value` as the JSON a field of `kind` reads inside a list or an object:
/// text, addresses, bytes and big numbers as strings, small numbers as
/// numbers, an option as `null` or its value.
fn json_of(kind: &IdlArgKind, value: &Value) -> Option<Json> {
    Some(match (kind, value) {
        (IdlArgKind::Option(_), Value::Option(None)) => Json::Plain(serde_json::Value::Null),
        (IdlArgKind::Option(inner), Value::Option(Some(value))) => json_of(inner, value)?,
        (IdlArgKind::Bool, Value::Bool(flag)) => Json::Plain(serde_json::Value::Bool(*flag)),
        (IdlArgKind::Integer { float: false, .. }, Value::Number(NumberValue::Unsigned(n))) => {
            match u64::try_from(*n) {
                Ok(n) if n <= (1 << 53) => Json::Plain(serde_json::Value::from(n)),
                _ => Json::Plain(serde_json::Value::String(n.to_string())),
            }
        }
        (IdlArgKind::Integer { float: false, .. }, Value::Number(NumberValue::Signed(n))) => {
            match i64::try_from(*n) {
                Ok(n) if n.unsigned_abs() <= (1 << 53) => Json::Plain(serde_json::Value::from(n)),
                _ => Json::Plain(serde_json::Value::String(n.to_string())),
            }
        }
        (IdlArgKind::List(item), Value::Array(items)) => Json::List(
            items
                .iter()
                .map(|value| json_of(item, value))
                .collect::<Option<_>>()?,
        ),
        (IdlArgKind::Struct { fields }, Value::Struct(map)) => {
            Json::Object(json_fields(fields, |name| map.get(name))?)
        }
        (IdlArgKind::Variants { variants }, Value::Enum { variant, fields }) => {
            let (variant, held) = variants
                .iter()
                .find(|(name, _)| data_variant(name) == *variant)?;
            let Value::Struct(map) = fields.as_ref() else {
                return None;
            };
            let held = match held {
                IdlVariant::Empty => {
                    return Some(Json::Plain(serde_json::Value::String(variant.clone())))
                }
                IdlVariant::Struct(fields) => {
                    Json::Object(json_fields(fields, |name| map.get(name))?)
                }
                IdlVariant::Tuple(items) => {
                    let values = match map.get("fields")? {
                        Value::Tuple(values) | Value::Array(values) => values,
                        _ => return None,
                    };
                    if values.len() != items.len() {
                        return None;
                    }
                    let mut out = items
                        .iter()
                        .zip(values)
                        .map(|(item, value)| json_of(item, value))
                        .collect::<Option<Vec<_>>>()?;
                    if out.len() == 1 {
                        out.pop()?
                    } else {
                        Json::List(out)
                    }
                }
            };
            Json::Object(vec![(variant.clone(), held)])
        }
        (kind, value) => Json::Plain(serde_json::Value::String(argument_text(kind, value, None)?)),
    })
}

fn json_fields<'a>(
    fields: &[(String, IdlArgKind)],
    get: impl Fn(&str) -> Option<&'a Value>,
) -> Option<Vec<(String, Json)>> {
    fields
        .iter()
        .map(|(name, kind)| Some((name.clone(), json_of(kind, get(name)?)?)))
        .collect()
}

/// JSON written with an object's keys in the order its struct declares them.
enum Json {
    Plain(serde_json::Value),
    List(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl std::fmt::Display for Json {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(value) => write!(f, "{value}"),
            Self::List(items) => {
                f.write_str("[")?;
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str("]")
            }
            Self::Object(fields) => {
                f.write_str("{")?;
                for (at, (name, value)) in fields.iter().enumerate() {
                    if at > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{}:{value}", serde_json::Value::String(name.clone()))?;
                }
                f.write_str("}")
            }
        }
    }
}
