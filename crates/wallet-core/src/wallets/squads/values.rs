//! Reading fields out of a decoded account.

use super::*;

pub(super) fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::struct_from(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_string(), value)),
    )
}

pub(super) fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Struct(fields) => fields.get(name),
        _ => None,
    }
}

pub(super) fn number<T: TryFrom<i128>>(value: &Value) -> Option<T> {
    match value {
        Value::Number(number) => T::try_from(number.as_i128()).ok(),
        _ => None,
    }
}

pub(super) fn key(value: &Value) -> Option<Pubkey> {
    match value {
        Value::PublicKey(bytes) => Some(Pubkey::new_from_array(*bytes)),
        _ => None,
    }
}

pub(super) fn bytes(value: &Value) -> Option<&[u8]> {
    match value {
        Value::Bytes(bytes) => Some(bytes),
        _ => None,
    }
}

pub(super) fn list(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Array(items) => Some(items),
        _ => None,
    }
}
