//! Reading what was typed into an argument field.

use super::*;

/// The value `text` stands for in a field of `kind`. `decimals` is the
/// field's scale ([`IdlArgScale`]): an integer typed with up to that many
/// decimal places, encoded in base units. `None` reads an integer as typed.
pub(super) fn parse_argument(
    kind: &IdlArgKind,
    text: &str,
    blank_uses_default: bool,
    decimals: Option<u8>,
) -> Result<Option<Value>, String> {
    let text = text.trim();
    if text.is_empty() && blank_uses_default {
        return Ok(None);
    }
    match kind {
        IdlArgKind::Option(_) if text.is_empty() => Ok(Some(Value::Option(None))),
        IdlArgKind::Option(inner) => {
            let value = parse_argument(inner, text, false, decimals)?
                .ok_or_else(|| "Enter a value.".to_string())?;
            Ok(Some(Value::Option(Some(Box::new(value)))))
        }
        IdlArgKind::Integer {
            signed,
            float,
            bits,
        } => {
            if text.is_empty() {
                return Err("Enter a number.".into());
            }
            match decimals {
                Some(decimals) if !*float => {
                    let digits = scaled_digits(text, decimals)?;
                    parse_integer(*signed, false, *bits, &digits, text).map(Some)
                }
                _ => parse_integer(*signed, *float, *bits, text, text).map(Some),
            }
        }
        IdlArgKind::Pubkey => {
            if text.is_empty() {
                return Err("Enter an address.".into());
            }
            Pubkey::from_str(text)
                .map(|pk| Some(Value::PublicKey(pk.to_bytes())))
                .map_err(|_| format!("`{text}` is not an address."))
        }
        IdlArgKind::String => {
            if text.is_empty() {
                return Err("Enter a value.".into());
            }
            Ok(Some(Value::String(text.to_string())))
        }
        IdlArgKind::Bool => match text {
            "true" => Ok(Some(Value::Bool(true))),
            "false" => Ok(Some(Value::Bool(false))),
            "" => Err("Enter true or false.".into()),
            _ => Err(format!("`{text}` is not true or false.")),
        },
        IdlArgKind::Enum { variants } => {
            if text.is_empty() {
                return Err("Choose a variant.".into());
            }
            if !variants.iter().any(|variant| variant == text) {
                return Err(format!("`{text}` is not a variant of this enum."));
            }
            Ok(Some(Value::unit_enum(text)))
        }
        IdlArgKind::Bytes => {
            if text.is_empty() {
                return Err("Enter hex bytes.".into());
            }
            hex::decode(text.trim_start_matches("0x"))
                .map(|bytes| Some(Value::Bytes(bytes)))
                .map_err(|_| format!("`{text}` is not hex."))
        }
        IdlArgKind::List(item) => parse_list(item, text).map(|items| Some(Value::Array(items))),
        IdlArgKind::FixedBytes { len } => fixed_bytes(text, *len).map(|bytes| {
            Some(Value::Array(
                bytes
                    .into_iter()
                    .map(|byte| Value::from(u128::from(byte)))
                    .collect(),
            ))
        }),
        IdlArgKind::Variants { variants } if is_empty_variant(variants, text) => {
            Ok(Some(Value::unit_enum(data_variant(text))))
        }
        IdlArgKind::Struct { .. } | IdlArgKind::Variants { .. } => {
            if text.is_empty() {
                return Err(json_shape(kind));
            }
            let json =
                serde_json::from_str::<serde_json::Value>(text).map_err(|_| json_shape(kind))?;
            json_value(kind, &json).map(Some)
        }
        IdlArgKind::Raw { note } => Err(format!(
            "This field ({note}) has to be filled by hand in a later slice."
        )),
    }
}

/// A list typed as a JSON array: `["--features", "mainnet"]`. Blank is the
/// empty list. Each item is read as a field of the item's kind would read
/// its text, so an integer may be written as a number or a string.
fn parse_list(item: &IdlArgKind, text: &str) -> Result<Vec<Value>, String> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let shape = "Enter a list as JSON, such as [\"a\", \"b\"], or leave it blank for none.";
    let serde_json::Value::Array(items) =
        serde_json::from_str::<serde_json::Value>(text).map_err(|_| shape.to_string())?
    else {
        return Err(shape.to_string());
    };
    items
        .iter()
        .enumerate()
        .map(|(at, value)| {
            let text = match (item, value) {
                (IdlArgKind::String, serde_json::Value::String(text)) => {
                    return Ok(Value::String(text.clone()));
                }
                (IdlArgKind::Pubkey, serde_json::Value::String(text)) => text.clone(),
                (IdlArgKind::Integer { .. }, serde_json::Value::String(text)) => text.clone(),
                (IdlArgKind::Integer { .. }, serde_json::Value::Number(number)) => {
                    number.to_string()
                }
                (IdlArgKind::Bool, serde_json::Value::Bool(flag)) => flag.to_string(),
                (IdlArgKind::Struct { .. } | IdlArgKind::Variants { .. }, value) => {
                    return json_value(item, value)
                        .map_err(|err| format!("Item {}: {err}", at + 1));
                }
                _ => {
                    return Err(format!(
                        "Item {} of the list is not {}.",
                        at + 1,
                        item_words(item)
                    ))
                }
            };
            parse_argument(item, &text, false, None)?
                .ok_or_else(|| format!("Item {} of the list is empty.", at + 1))
                .map_err(|err| format!("Item {}: {err}", at + 1))
        })
        .collect()
}

/// The name the encoder knows a variant of an enum with fields by: the
/// IDL's name with its first letter in capitals.
pub(crate) fn data_variant(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Whether `text` names a variant of `variants` that holds nothing.
fn is_empty_variant(variants: &[(String, IdlVariant)], text: &str) -> bool {
    variants
        .iter()
        .any(|(name, held)| name == text && *held == IdlVariant::Empty)
}

/// What a struct or an enum with fields is typed as, for a message.
fn json_shape(kind: &IdlArgKind) -> String {
    match kind {
        IdlArgKind::Struct { fields } => format!(
            "Enter a JSON object with {}.",
            fields
                .iter()
                .map(|(name, _)| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        IdlArgKind::Variants { variants } => {
            let names: Vec<&str> = variants.iter().map(|(name, _)| name.as_str()).collect();
            let holding = variants
                .iter()
                .find(|(_, held)| *held != IdlVariant::Empty)
                .map_or("", |(name, _)| name.as_str());
            format!(
                "Enter one of {}; one that holds values as JSON, such as {{\"{holding}\": …}}.",
                names.join(", ")
            )
        }
        _ => "Enter a value as JSON.".to_string(),
    }
}

/// The bytes of a fixed byte array typed as `text`: its UTF-8, or the hex
/// after `0x`, followed by zeros up to `len`. Blank is all zeros.
pub(crate) fn fixed_bytes(text: &str, len: usize) -> Result<Vec<u8>, String> {
    let mut bytes = match text.strip_prefix("0x") {
        Some(digits) => {
            hex::decode(digits).map_err(|_| format!("`{text}` is not hex after 0x."))?
        }
        None => text.as_bytes().to_vec(),
    };
    if bytes.len() > len {
        return Err(format!(
            "This field holds {len} bytes, and this is {}.",
            bytes.len()
        ));
    }
    bytes.resize(len, 0);
    Ok(bytes)
}

/// The value `json` stands for in a field of `kind`, inside a JSON object
/// or list: text and addresses are strings, a number a number or a string,
/// an option `null` or its value.
fn json_value(kind: &IdlArgKind, json: &serde_json::Value) -> Result<Value, String> {
    use serde_json::Value as Json;
    let text = match (kind, json) {
        (IdlArgKind::Option(_), Json::Null) => return Ok(Value::Option(None)),
        (IdlArgKind::Option(inner), json) => {
            return json_value(inner, json).map(|value| Value::Option(Some(Box::new(value))));
        }
        (IdlArgKind::String, Json::String(text)) => return Ok(Value::String(text.clone())),
        (IdlArgKind::List(item), Json::Array(items)) => {
            return items
                .iter()
                .enumerate()
                .map(|(at, value)| {
                    json_value(item, value).map_err(|err| format!("item {}: {err}", at + 1))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array);
        }
        (IdlArgKind::Struct { fields }, Json::Object(object)) => {
            return json_struct(fields, object).map(Value::struct_from);
        }
        (IdlArgKind::Variants { variants }, Json::String(name)) => {
            return if is_empty_variant(variants, name) {
                Ok(Value::unit_enum(data_variant(name)))
            } else {
                Err(format!("`{name}` is not a variant that holds nothing."))
            };
        }
        (IdlArgKind::Variants { variants }, Json::Object(object)) if object.len() == 1 => {
            let (name, held_json) = object.iter().next().expect("one key");
            let Some((_, held)) = variants.iter().find(|(variant, _)| variant == name) else {
                return Err(format!("`{name}` is not a variant of this enum."));
            };
            let fields = match held {
                IdlVariant::Empty => Vec::new(),
                IdlVariant::Struct(fields) => match held_json {
                    Json::Object(object) => json_struct(fields, object)?,
                    _ => return Err(format!("`{name}` holds a JSON object.")),
                },
                IdlVariant::Tuple(items) => {
                    let values = match (items.as_slice(), held_json) {
                        ([one], Json::Array(_)) if !matches!(one, IdlArgKind::List(_)) => {
                            return Err(format!("`{name}` holds one value, not a list."));
                        }
                        ([one], json) => vec![json_value(one, json)?],
                        (items, Json::Array(values)) if values.len() == items.len() => items
                            .iter()
                            .zip(values)
                            .map(|(item, value)| json_value(item, value))
                            .collect::<Result<Vec<_>, _>>()?,
                        (items, _) => {
                            return Err(format!(
                                "`{name}` holds a list of {} values.",
                                items.len()
                            ));
                        }
                    };
                    vec![("fields".to_string(), Value::Tuple(values))]
                }
            };
            return Ok(Value::Enum {
                variant: data_variant(name),
                fields: Box::new(Value::struct_from(fields)),
            });
        }
        (IdlArgKind::Pubkey, Json::String(text)) => text.clone(),
        (IdlArgKind::Integer { .. }, Json::String(text)) => text.clone(),
        (IdlArgKind::Integer { .. }, Json::Number(number)) => number.to_string(),
        (IdlArgKind::Bool, Json::Bool(flag)) => flag.to_string(),
        (IdlArgKind::Enum { .. }, Json::String(text)) => text.clone(),
        (IdlArgKind::Bytes | IdlArgKind::FixedBytes { .. }, Json::String(text)) => text.clone(),
        (kind, _) => return Err(format!("this is not {}.", item_words(kind))),
    };
    parse_argument(kind, &text, false, None)?.ok_or_else(|| "it is empty.".to_string())
}

/// A struct's fields from a JSON object, in the order the struct declares
/// them. A key that is no field, and a field with no key, are refused.
fn json_struct(
    fields: &[(String, IdlArgKind)],
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<(String, Value)>, String> {
    if let Some(extra) = object
        .keys()
        .find(|key| !fields.iter().any(|(name, _)| name == *key))
    {
        return Err(format!("there is no field \"{extra}\"."));
    }
    let mut out = Vec::with_capacity(fields.len());
    for (name, field) in fields {
        let value = object
            .get(name)
            .ok_or_else(|| format!("\"{name}\" is missing."))?;
        let value = json_value(field, value).map_err(|err| format!("{name}: {err}"))?;
        out.push((name.clone(), value));
    }
    Ok(out)
}

fn item_words(item: &IdlArgKind) -> &'static str {
    match item {
        IdlArgKind::String => "text in quotes",
        IdlArgKind::Pubkey => "an address in quotes",
        IdlArgKind::Bool => "true or false",
        IdlArgKind::Integer { .. } => "a whole number",
        IdlArgKind::Struct { .. } => "a JSON object",
        IdlArgKind::List(_) => "a JSON list",
        IdlArgKind::Enum { .. } => "a variant's name in quotes",
        IdlArgKind::Variants { .. } => "a variant",
        IdlArgKind::Bytes | IdlArgKind::FixedBytes { .. } => "bytes in quotes",
        IdlArgKind::Option(_) => "a value or null",
        _ => "a value this list holds",
    }
}

/// The integer in `digits`, refused when a field of `bits` bits cannot hold
/// it: the encoder would otherwise keep only its low bytes. `shown` is what
/// was typed, for the message.
pub(super) fn parse_integer(
    signed: bool,
    float: bool,
    bits: u8,
    digits: &str,
    shown: &str,
) -> Result<Value, String> {
    if float {
        return digits
            .parse::<f64>()
            .map(Value::from)
            .map_err(|_| format!("Enter a number, not `{shown}`."));
    }
    let bits = u32::from(bits.clamp(1, 128));
    if signed {
        let value = digits
            .parse::<i128>()
            .map_err(|_| too_large_or_not(digits, shown))?;
        let max = i128::MAX >> (128 - bits);
        let min = -max - 1;
        if value > max {
            return Err(format!("`{shown}` is too large for this field."));
        }
        if value < min {
            return Err(format!("`{shown}` is too small for this field."));
        }
        return Ok(Value::from(value));
    }
    if let Some(rest) = digits.strip_prefix('-') {
        if rest.bytes().all(|b| b.is_ascii_digit()) && !rest.is_empty() {
            return Err(format!("Enter zero or more, not `{shown}`."));
        }
    }
    let value = digits
        .parse::<u128>()
        .map_err(|_| too_large_or_not(digits, shown))?;
    let max = u128::MAX >> (128 - bits);
    if value > max {
        return Err(format!("`{shown}` is too large for this field."));
    }
    Ok(Value::from(value))
}

/// Digits too many for any integer are too large; anything else is not an
/// integer.
fn too_large_or_not(digits: &str, shown: &str) -> String {
    let body = digits.strip_prefix('-').unwrap_or(digits);
    if !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit()) {
        if digits.starts_with('-') {
            format!("`{shown}` is too small for this field.")
        } else {
            format!("`{shown}` is too large for this field.")
        }
    } else {
        format!("Enter an integer, not `{shown}`.")
    }
}
