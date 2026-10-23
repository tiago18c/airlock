//! Encoding instruction arguments and resolving argument defaults.
//!
//! Rust counterpart of the TypeScript `dynamic-instructions/arguments`
//! sub-package. Unlike TypeScript — which receives loosely-typed JavaScript
//! input and runs it through superstruct validators plus a "codec input
//! transformer" — the Rust port takes a strongly-typed [`Value`] directly, so
//! that validation and normalisation step is unnecessary: the codec consumes
//! the `Value` as-is and surfaces any mismatch as an encode error.

use codama_dynamic_codecs::{get_node_codec, get_value_with, Value};
use codama_dynamic_resolution::{AccountsInput, ArgumentsInput};
use codama_nodes::{
    DefaultValueStrategy, InstructionArgumentNode, InstructionInputValueNode, InstructionNode,
    RootNode, TypeNode, ValueNode,
};
use codama_visitors_core::NodePath;
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use std::collections::HashMap;

/// A custom resolver that produces an argument [`Value`] from the currently
/// known accounts and arguments.
///
/// Counterpart of the TypeScript `ResolverFn` when used for argument defaults
/// (collapsed to synchronous, and typed to return a `Value` rather than the
/// `Pubkey` an account resolver returns).
pub type ArgumentResolverFn = Box<dyn Fn(&AccountsInput, &ArgumentsInput) -> CodamaResult<Value>>;

/// Whether the argument uses the `omitted` default-value strategy.
pub(crate) fn is_omitted(node: &InstructionArgumentNode) -> bool {
    node.default_value_strategy == Some(DefaultValueStrategy::Omitted)
}

/// Whether the argument's type is an option-like node and no value was provided
/// — in which case it is encoded as `none`.
fn is_optional(node: &InstructionArgumentNode, input: Option<&Value>) -> bool {
    matches!(
        node.r#type.as_ref(),
        TypeNode::Option(_) | TypeNode::ZeroableOption(_) | TypeNode::RemainderOption(_)
    ) && input.is_none()
}

/// Resolves argument defaults from user-provided custom resolvers.
///
/// For each argument (including `extra_arguments`) that is missing, is not
/// omitted, and whose default value is a [`ResolverValueNode`], invokes the
/// matching resolver and records the produced value. Arguments with no matching
/// resolver are left untouched so the encoding step can handle them (optional →
/// `none`, required → error).
///
/// [`ResolverValueNode`]: codama_nodes::ResolverValueNode
pub fn resolve_argument_defaults(
    ix: &InstructionNode,
    arguments: &ArgumentsInput,
    accounts: &AccountsInput,
    resolvers: &HashMap<String, ArgumentResolverFn>,
) -> CodamaResult<ArgumentsInput> {
    let mut resolved = arguments.clone();

    for arg in ix.arguments.iter().chain(ix.extra_arguments.iter()) {
        if resolved.contains_key(arg.name.as_str()) {
            continue;
        }
        if is_omitted(arg) {
            continue;
        }
        let resolver_name = match arg.default_value.as_ref().as_ref() {
            Some(InstructionInputValueNode::ResolverValue(r)) => r.name.as_str(),
            _ => continue,
        };
        // No resolver provided — let the encoding step decide (optional → none,
        // required → error), matching the TypeScript behaviour.
        let Some(resolver_fn) = resolvers.get(resolver_name) else {
            continue;
        };
        let value = resolver_fn(accounts, &resolved)
            .map_err(|_| CodamaError::FailedToExecuteResolver(resolver_name.to_string()))?;
        resolved.insert(arg.name.to_string(), value);
    }

    Ok(resolved)
}

/// Encodes all of an instruction's arguments into a single byte buffer.
///
/// Iterates over `ix.arguments` (not `extra_arguments`, which are never
/// serialised), encoding each according to its category: omitted arguments use
/// their default value, option-typed arguments with no input encode as `none`,
/// and required arguments encode the provided [`Value`].
pub fn encode_instruction_arguments(
    root: &RootNode,
    ix: &InstructionNode,
    arguments: &ArgumentsInput,
    bumps: &HashMap<String, u8>,
    injected: &HashMap<String, Value>,
) -> CodamaResult<Vec<u8>> {
    let mut out = Vec::new();

    for arg in &ix.arguments {
        let path = NodePath::new()
            .extended(root.clone())
            .extended(root.program.clone())
            .extended(ix.clone())
            .extended(arg.clone());
        let codec = get_node_codec(&path)?;
        let input = arguments.get(arg.name.as_str());

        let chunk = if is_omitted(arg) {
            encode_omitted_argument(ix, arg, codec.as_ref(), bumps, injected)?
        } else if is_optional(arg, input) {
            encode_value(arg, codec.as_ref(), &Value::Option(None))?
        } else {
            let value = input.ok_or_else(|| CodamaError::ArgumentMissing(arg.name.to_string()))?;
            encode_value(arg, codec.as_ref(), value)?
        };
        out.extend_from_slice(&chunk);
    }

    Ok(out)
}

/// Encodes an omitted argument from its default value node.
fn encode_omitted_argument(
    ix: &InstructionNode,
    arg: &InstructionArgumentNode,
    codec: &dyn codama_dynamic_codecs::Codec,
    bumps: &HashMap<String, u8>,
    injected: &HashMap<String, Value>,
) -> CodamaResult<Vec<u8>> {
    let default_value =
        arg.default_value
            .as_ref()
            .clone()
            .ok_or_else(|| CodamaError::DefaultValueMissing {
                argument: arg.name.to_string(),
                instruction: ix.name.to_string(),
            })?;
    let value = match default_value {
        InstructionInputValueNode::AccountBumpValue(node) => {
            let bump = bumps.get(node.name.as_str()).copied().ok_or_else(|| {
                CodamaError::FailedToDerivePda(format!(
                    "bump for account `{}` is not available",
                    node.name.as_str()
                ))
            })?;
            Value::from(bump)
        }
        InstructionInputValueNode::AccountFieldValue(node) => {
            return Err(CodamaError::UnexpectedAddressType(format!(
                "account field `{}` requires fetched account data",
                node.account.as_str()
            )));
        }
        other => {
            let value_node = ValueNode::try_from(other)?;
            get_value_with(&value_node, injected)?
        }
    };
    encode_value(arg, codec, &value)
}

/// Encodes a [`Value`] with the given codec, wrapping any failure as a
/// [`CodamaError::FailedToEncodeArgument`].
fn encode_value(
    arg: &InstructionArgumentNode,
    codec: &dyn codama_dynamic_codecs::Codec,
    value: &Value,
) -> CodamaResult<Vec<u8>> {
    codec
        .encode_to_vec(value)
        .map_err(|e| CodamaError::FailedToEncodeArgument {
            argument: arg.name.to_string(),
            message: e.to_string(),
        })
}
