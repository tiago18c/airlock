//! The unit an integer argument is typed in, read from the IDL's amount
//! display: the same `amountNumberDisplayNode` Review scales an amount by.
//!
//! A scale is fixed (`transferSol.amount`: 9 decimals, SOL), taken from
//! another argument of the same instruction (`transferChecked.amount` from
//! `decimals`), or none. A scale that needs an account's data, such as
//! `mintTo.amount` from its mint, is none here: that field is typed in base
//! units.

use super::*;
use crate::units::{decimal_to_base, format_base_digits, DecimalError};
use codama_nodes::{
    AmountNumberDisplayNode, InjectableNumberValueNode, InjectableStringValueNode,
    InjectedValueNode, InstructionNode, NumberDisplayNode, RegisteredContextualValueNode,
    RegisteredValueNode, ValueNode,
};
use std::collections::HashSet;

/// The scale of an argument of `instruction` with type `ty`.
pub(super) fn argument_scale(instruction: &InstructionNode, ty: &TypeNode) -> IdlArgScale {
    match unwrap_type(ty) {
        TypeNode::Option(node) => argument_scale(instruction, &node.item),
        TypeNode::RemainderOption(node) => argument_scale(instruction, &node.item),
        TypeNode::ZeroableOption(node) => argument_scale(instruction, &node.item),
        TypeNode::Number(node) => number_scale(instruction, node, None),
        TypeNode::Amount(node) => {
            let fallback = u8::try_from(node.decimals)
                .ok()
                .map(|decimals| IdlArgScale::Fixed {
                    decimals,
                    unit: node.unit.clone().filter(|unit| !unit.is_empty()),
                });
            number_scale(instruction, node.number.get_nested_type_node(), fallback)
        }
        TypeNode::SolAmount(node) => number_scale(
            instruction,
            node.number.get_nested_type_node(),
            Some(IdlArgScale::Fixed {
                decimals: 9,
                unit: Some("SOL".to_string()),
            }),
        ),
        _ => IdlArgScale::None,
    }
}

/// A number's own display wins over what its amount type implies, as on
/// Review. A float is never scaled.
fn number_scale(
    instruction: &InstructionNode,
    number: &codama_nodes::NumberTypeNode,
    implied: Option<IdlArgScale>,
) -> IdlArgScale {
    if matches!(number.format, NumberFormat::F32 | NumberFormat::F64) {
        return IdlArgScale::None;
    }
    match number.display.as_ref().as_ref() {
        Some(NumberDisplayNode::Amount(amount)) => amount_scale(instruction, amount),
        _ => implied.unwrap_or_default(),
    }
}

fn amount_scale(instruction: &InstructionNode, amount: &AmountNumberDisplayNode) -> IdlArgScale {
    let unit = match amount.unit.as_ref().as_ref() {
        None => None,
        Some(InjectableStringValueNode::String(node)) => Some(node.string.clone()),
        Some(InjectableStringValueNode::Injected(injected)) => {
            match resolve_injected(instruction, injected) {
                Some(Node::Value(RegisteredValueNode::String(node))) => Some(node.string),
                _ => None,
            }
        }
    }
    .filter(|unit| !unit.is_empty());
    let terminal = match amount.decimals.as_ref().as_ref() {
        None => return IdlArgScale::Fixed { decimals: 0, unit },
        Some(InjectableNumberValueNode::Number(node)) => {
            return match number_u8(&node.number) {
                Some(decimals) => IdlArgScale::Fixed { decimals, unit },
                None => IdlArgScale::None,
            };
        }
        Some(InjectableNumberValueNode::Injected(injected)) => {
            resolve_injected(instruction, injected)
        }
    };
    match terminal {
        Some(Node::Value(RegisteredValueNode::Number(node))) => match number_u8(&node.number) {
            Some(decimals) => IdlArgScale::Fixed { decimals, unit },
            None => IdlArgScale::None,
        },
        Some(Node::ContextualValue(RegisteredContextualValueNode::Argument(node))) => {
            let name = node.name.as_str();
            // Only an argument the form asks for, as an integer.
            let asked = instruction.arguments.iter().any(|argument| {
                argument.name.as_str() == name
                    && argument.default_value_strategy != Some(DefaultValueStrategy::Omitted)
                    && matches!(
                        classify(&argument.r#type),
                        IdlArgKind::Integer { float: false, .. }
                    )
            });
            if asked {
                IdlArgScale::FromArgument {
                    argument: name.to_string(),
                    unit,
                }
            } else {
                IdlArgScale::None
            }
        }
        // Account data, or anything else: base units.
        _ => IdlArgScale::None,
    }
}

/// What an injected key stands for: the node the instruction provides for
/// it, followed through further keys, or its fallback value.
fn resolve_injected(instruction: &InstructionNode, injected: &InjectedValueNode) -> Option<Node> {
    let mut seen = HashSet::new();
    let mut current = injected.clone();
    loop {
        if !seen.insert(current.key.to_string()) {
            return None;
        }
        let provided = instruction
            .provides
            .iter()
            .find(|provided| provided.name.as_str() == current.key.as_str())
            .map(|provided| (*provided.node).clone());
        let next = match provided {
            Some(node) => node,
            None => match current.fallback.as_ref().as_ref()? {
                ValueNode::Number(node) => return Some((*node).into()),
                ValueNode::String(node) => return Some(node.clone().into()),
                ValueNode::Injected(node) => node.clone().into(),
                _ => return None,
            },
        };
        match next {
            Node::Value(RegisteredValueNode::Injected(node)) => current = node,
            other => return Some(other),
        }
    }
}

fn number_u8(number: &Number) -> Option<u8> {
    match number {
        Number::UnsignedInteger(value) => u8::try_from(*value).ok(),
        Number::SignedInteger(value) => u8::try_from(*value).ok(),
        Number::Float(_) => None,
    }
}

/// The decimal places `field` is typed with, given the text of every
/// argument. `None` for a field in base units. A scale from another
/// argument asks for that argument first when it is blank or not a number
/// of decimals.
pub(crate) fn field_decimals(
    field: &IdlArgField,
    text: &HashMap<String, String>,
) -> Result<Option<u8>, String> {
    match &field.scale {
        IdlArgScale::None => Ok(None),
        IdlArgScale::Fixed { decimals, .. } => Ok(Some(*decimals)),
        IdlArgScale::FromArgument { argument, .. } => {
            let source = text.get(argument).map_or("", |text| text.trim());
            if source.is_empty() {
                return Err(format!(
                    "Enter {argument} first: {} is counted in the units it sets.",
                    field.name
                ));
            }
            source.parse::<u8>().map(Some).map_err(|_| {
                format!(
                    "Enter {argument} as a whole number from 0 to 255 first: {} is counted in the units it sets.",
                    field.name
                )
            })
        }
    }
}

/// The value of `field`, read from its text in `text`.
pub(crate) fn parse_field(
    field: &IdlArgField,
    text: &HashMap<String, String>,
) -> Result<Option<Value>, String> {
    let raw = text.get(&field.name).map_or("", String::as_str);
    let decimals = if raw.trim().is_empty() {
        None
    } else {
        field_decimals(field, text)?
    };
    parse_argument(&field.kind, raw, field.blank_uses_default, decimals)
}

/// The argument text with each scaled field written in base units, for the
/// seeds of a derived address, which read an argument as the program does.
/// A scaled field that does not read yet is left blank, so its address waits.
pub(super) fn base_unit_text(
    fields: &[IdlArgField],
    text: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut out = text.clone();
    for field in fields {
        if field.scale == IdlArgScale::None {
            continue;
        }
        let base = match parse_field(field, text) {
            Ok(Some(value)) => integer_text(&value).unwrap_or_default(),
            _ => String::new(),
        };
        out.insert(field.name.clone(), base);
    }
    out
}

fn integer_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(NumberValue::Unsigned(n)) => Some(n.to_string()),
        Value::Number(NumberValue::Signed(n)) => Some(n.to_string()),
        Value::Option(Some(inner)) => integer_text(inner),
        Value::Option(None) => Some(String::new()),
        _ => None,
    }
}

/// An integer typed with `decimals` decimal places, as the digits of its
/// value in base units.
pub(super) fn scaled_digits(text: &str, decimals: u8) -> Result<String, String> {
    decimal_to_base(text, decimals).map_err(|err| match err {
        DecimalError::NotANumber => format!("Enter a number, not `{text}`."),
        DecimalError::TooManyPlaces if decimals == 0 => {
            format!("Enter a whole number, not `{text}`.")
        }
        DecimalError::TooManyPlaces if decimals == 1 => {
            format!("`{text}` has more than 1 decimal place.")
        }
        DecimalError::TooManyPlaces => {
            format!("`{text}` has more than {decimals} decimal places.")
        }
    })
}

/// The text a scaled field shows for `value`, in base units: what
/// [`scaled_digits`] reads back to the same integer.
pub(crate) fn scaled_text(value: &NumberValue, decimals: u8) -> Option<String> {
    match value {
        NumberValue::Unsigned(n) => Some(format_base_digits(&n.to_string(), decimals)),
        NumberValue::Signed(n) => {
            let body = format_base_digits(&n.unsigned_abs().to_string(), decimals);
            Some(if *n < 0 { format!("-{body}") } else { body })
        }
        NumberValue::Float(_) => None,
    }
}
