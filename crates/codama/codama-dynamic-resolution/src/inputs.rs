use codama_dynamic_codecs::{NumberValue, Value};
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;

/// User-supplied account addresses, keyed by instruction-account name.
pub type AccountsInput = HashMap<String, Pubkey>;

/// User-supplied instruction arguments, keyed by argument name.
pub type ArgumentsInput = HashMap<String, Value>;

/// A custom resolver function provided by the caller.
///
/// Given the currently known accounts and arguments, it returns an address.
/// Counterpart of the TypeScript `ResolverFn` (collapsed to synchronous).
pub type ResolverFn = Box<dyn Fn(&AccountsInput, &ArgumentsInput) -> CodamaResult<Pubkey>>;

/// Converts a runtime [`Value`] into a [`Pubkey`].
///
/// Accepts a `PublicKey` value, a base58 `String`, or 32 raw `Bytes`.
pub fn value_to_pubkey(value: &Value) -> CodamaResult<Pubkey> {
    match value {
        Value::PublicKey(bytes) => Ok(Pubkey::new_from_array(*bytes)),
        Value::String(s) => {
            Pubkey::from_str(s).map_err(|e| CodamaError::UnexpectedAddressType(e.to_string()))
        }
        Value::Bytes(b) => {
            let arr: [u8; 32] = b.as_slice().try_into().map_err(|_| {
                CodamaError::UnexpectedAddressType(format!(
                    "expected 32 bytes for a public key, got {}",
                    b.len()
                ))
            })?;
            Ok(Pubkey::new_from_array(arr))
        }
        other => Err(CodamaError::UnexpectedAddressType(format!("{other:?}"))),
    }
}

/// Whether a [`Value`] is considered truthy when used as an unparameterized
/// conditional condition (no explicit expected value).
pub(crate) fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(NumberValue::Unsigned(n)) => *n != 0,
        Value::Number(NumberValue::Signed(n)) => *n != 0,
        Value::Number(NumberValue::Float(n)) => *n != 0.0,
        Value::Option(opt) => opt.is_some(),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
