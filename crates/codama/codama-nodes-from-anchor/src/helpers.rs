//! JSON access helpers and case conversions shared by both converters.

use crate::{AnchorIdlError, Result};
use serde_json::Value;

/// A string field, or an error naming it.
pub fn req_str<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| AnchorIdlError::MissingField(field.to_string()))
}

/// An optional string field.
pub fn opt_str<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

/// An optional array field, defaulting to empty.
pub fn arr<'a>(value: &'a Value, field: &str) -> &'a [Value] {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// An optional bool field, defaulting to false.
pub fn flag(value: &Value, field: &str) -> bool {
    value.get(field).and_then(Value::as_bool).unwrap_or(false)
}

/// Doc lines from an optional `docs` array of strings.
pub fn docs(value: &Value) -> Vec<String> {
    arr(value, "docs")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// A `discriminator: [u8, ...]` field as bytes.
pub fn byte_array(value: &Value) -> Option<Vec<u8>> {
    value
        .as_array()?
        .iter()
        .map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect()
}

/// `snake_case`, mirroring the TS `snakeCase` used for discriminator preimages.
pub fn snake_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    let mut prev_lower = false;
    for c in input.chars() {
        if c == '-' || c == ' ' || c == '_' {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            prev_lower = false;
        } else if c.is_uppercase() {
            if prev_lower && !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            out.extend(c.to_lowercase());
            prev_lower = false;
        } else {
            out.push(c);
            prev_lower = c.is_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

/// `PascalCase`, mirroring the TS `pascalCase` used for discriminator preimages.
pub fn pascal_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut new_word = true;
    let mut prev_lower = false;
    for c in input.chars() {
        if c == '-' || c == ' ' || c == '_' {
            new_word = true;
            prev_lower = false;
        } else if new_word {
            out.extend(c.to_uppercase());
            new_word = false;
            prev_lower = c.is_lowercase();
        } else if c.is_uppercase() {
            out.push(c);
            prev_lower = false;
        } else {
            out.push(c);
            prev_lower = true;
        }
    }
    let _ = prev_lower;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_conversions() {
        assert_eq!(snake_case("addConfigLines"), "add_config_lines");
        assert_eq!(snake_case("add_config_lines"), "add_config_lines");
        assert_eq!(pascal_case("stakeEntry"), "StakeEntry");
        assert_eq!(pascal_case("stake_entry"), "StakeEntry");
        assert_eq!(pascal_case("StakeEntry"), "StakeEntry");
    }
}
