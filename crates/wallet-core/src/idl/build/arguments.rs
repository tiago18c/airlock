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

fn item_words(item: &IdlArgKind) -> &'static str {
    match item {
        IdlArgKind::String => "text in quotes",
        IdlArgKind::Pubkey => "an address in quotes",
        IdlArgKind::Bool => "true or false",
        IdlArgKind::Integer { .. } => "a whole number",
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
