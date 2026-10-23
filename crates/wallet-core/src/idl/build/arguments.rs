//! Reading what was typed into an argument field.

use super::*;

pub(super) fn parse_argument(
    kind: &IdlArgKind,
    text: &str,
    blank_uses_default: bool,
) -> Result<Option<Value>, String> {
    let text = text.trim();
    if text.is_empty() && blank_uses_default {
        return Ok(None);
    }
    match kind {
        IdlArgKind::Option(inner) if text.is_empty() => Ok(Some(Value::Option(None))),
        IdlArgKind::Option(inner) => {
            let value =
                parse_argument(inner, text, false)?.ok_or_else(|| "Enter a value.".to_string())?;
            Ok(Some(Value::Option(Some(Box::new(value)))))
        }
        IdlArgKind::Integer { signed, float } => {
            if text.is_empty() {
                return Err("Enter a number.".into());
            }
            parse_integer(*signed, *float, text).map(Some)
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
        IdlArgKind::Raw { note } => Err(format!(
            "This field ({note}) has to be filled by hand in a later slice."
        )),
    }
}

pub(super) fn parse_integer(signed: bool, float: bool, text: &str) -> Result<Value, String> {
    if float {
        return text
            .parse::<f64>()
            .map(Value::from)
            .map_err(|_| format!("Enter a number, not `{text}`."));
    }
    if signed {
        return text
            .parse::<i128>()
            .map(Value::from)
            .map_err(|_| format!("Enter an integer, not `{text}`."));
    }
    text.parse::<u128>()
        .map(Value::from)
        .map_err(|_| format!("Enter an integer, not `{text}`."))
}
