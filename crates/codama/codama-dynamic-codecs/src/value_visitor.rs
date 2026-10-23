use crate::value::{NumberValue, Value};
use codama_nodes::{
    ArrayValueNode, BooleanValueNode, BytesValueNode, ConstantValueNode, EnumValueNode,
    EnumValuePayload, InjectedValueNode, MapEntryValueNode, MapValueNode, NoneValueNode, Number,
    NumberValueNode, PublicKeyValueNode, SetValueNode, SomeValueNode, StringValueNode,
    StructValueNode, TupleValueNode, ValueNode,
};
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use codama_visitors_core::{VisitableNode, Visitor};
use indexmap::IndexMap;

/// Converts Codama value nodes into runtime [`Value`]s.
///
/// Counterpart of the TypeScript value-node visitor. It is self-contained (no
/// link resolution), which is sufficient for the value nodes the dynamic client
/// needs to evaluate (constants, defaults, PDA seeds, discriminator values).
#[derive(Default)]
pub struct ValueVisitor {
    injected: std::collections::HashMap<String, Value>,
}

fn pascal_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

impl Visitor<Value> for ValueVisitor {
    fn visit_number_value(&mut self, node: &NumberValueNode) -> CodamaResult<Value> {
        let n = match node.number {
            Number::UnsignedInteger(u) => NumberValue::Unsigned(u as u128),
            Number::SignedInteger(i) => NumberValue::Signed(i as i128),
            Number::Float(f) => NumberValue::Float(f),
        };
        Ok(Value::Number(n))
    }
    fn visit_boolean_value(&mut self, node: &BooleanValueNode) -> CodamaResult<Value> {
        Ok(Value::Bool(node.boolean))
    }
    fn visit_string_value(&mut self, node: &StringValueNode) -> CodamaResult<Value> {
        Ok(Value::String(node.string.clone()))
    }
    fn visit_bytes_value(&mut self, node: &BytesValueNode) -> CodamaResult<Value> {
        Ok(Value::Bytes(crate::encoding::decode_bytes(
            node.encoding,
            &node.data,
        )?))
    }
    fn visit_public_key_value(&mut self, node: &PublicKeyValueNode) -> CodamaResult<Value> {
        let bytes = bs58::decode(&node.public_key)
            .into_vec()
            .map_err(|e| CodamaError::InvalidBytesEncoding(e.to_string()))?;
        let key: [u8; 32] = bytes.try_into().map_err(|_| {
            CodamaError::InvalidBytesEncoding("public key is not 32 bytes".to_string())
        })?;
        Ok(Value::PublicKey(key))
    }
    fn visit_array_value(&mut self, node: &ArrayValueNode) -> CodamaResult<Value> {
        Ok(Value::Array(self.items(&node.items)?))
    }
    fn visit_set_value(&mut self, node: &SetValueNode) -> CodamaResult<Value> {
        Ok(Value::Set(self.items(&node.items)?))
    }
    fn visit_tuple_value(&mut self, node: &TupleValueNode) -> CodamaResult<Value> {
        Ok(Value::Tuple(self.items(&node.items)?))
    }
    fn visit_struct_value(&mut self, node: &StructValueNode) -> CodamaResult<Value> {
        Ok(Value::Struct(self.struct_fields(node)?))
    }
    fn visit_some_value(&mut self, node: &SomeValueNode) -> CodamaResult<Value> {
        Ok(Value::Option(Some(Box::new(node.value.accept(self)?))))
    }
    fn visit_none_value(&mut self, _node: &NoneValueNode) -> CodamaResult<Value> {
        Ok(Value::Option(None))
    }
    fn visit_constant_value(&mut self, node: &ConstantValueNode) -> CodamaResult<Value> {
        node.value.accept(self)
    }
    fn visit_map_value(&mut self, node: &MapValueNode) -> CodamaResult<Value> {
        let pairs = node
            .entries
            .iter()
            .map(|e| Ok((e.key.accept(self)?, e.value.accept(self)?)))
            .collect::<CodamaResult<Vec<_>>>()?;
        Ok(Value::Map(pairs))
    }
    fn visit_map_entry_value(&mut self, node: &MapEntryValueNode) -> CodamaResult<Value> {
        Ok(Value::Tuple(vec![
            node.key.accept(self)?,
            node.value.accept(self)?,
        ]))
    }
    fn visit_injected_value(&mut self, node: &InjectedValueNode) -> CodamaResult<Value> {
        if let Some(value) = self.injected.get(node.key.as_str()) {
            return Ok(value.clone());
        }
        if let Some(fallback) = node.fallback.as_ref().as_ref() {
            return fallback.accept(self);
        }
        Err(CodamaError::InjectedValueMissing(node.key.to_string()))
    }
    fn visit_enum_value(&mut self, node: &EnumValueNode) -> CodamaResult<Value> {
        match node.value.as_ref().as_ref() {
            None => Ok(Value::unit_enum(node.variant.to_string())),
            Some(EnumValuePayload::Struct(s)) => Ok(Value::Enum {
                variant: pascal_case(&node.variant),
                fields: Box::new(Value::Struct(self.struct_fields(s)?)),
            }),
            Some(EnumValuePayload::Tuple(t)) => Ok(Value::Enum {
                variant: pascal_case(&node.variant),
                fields: Box::new(Value::struct_from([(
                    "fields".to_string(),
                    Value::Tuple(self.items(&t.items)?),
                )])),
            }),
        }
    }
}

impl ValueVisitor {
    fn items(&mut self, items: &[ValueNode]) -> CodamaResult<Vec<Value>> {
        items.iter().map(|item| item.accept(self)).collect()
    }

    fn struct_fields(&mut self, node: &StructValueNode) -> CodamaResult<IndexMap<String, Value>> {
        let mut map = IndexMap::new();
        for field in &node.fields {
            map.insert(field.name.to_string(), field.value.accept(self)?);
        }
        Ok(map)
    }
}

/// Converts a value node into a runtime [`Value`].
pub fn get_value(node: &ValueNode) -> CodamaResult<Value> {
    get_value_with(node, &std::collections::HashMap::new())
}

/// Converts a value node, resolving [`InjectedValueNode`]s from `injected`
/// before their fallback.
pub fn get_value_with(
    node: &ValueNode,
    injected: &std::collections::HashMap<String, Value>,
) -> CodamaResult<Value> {
    node.accept(&mut ValueVisitor {
        injected: injected.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_nodes::{
        ArrayValueNode, BooleanValueNode, NumberValueNode, SomeValueNode, StringValueNode,
        StructFieldValueNode, StructValueNode,
    };

    #[test]
    fn primitives() {
        assert_eq!(
            get_value(&NumberValueNode::new(42u8).into()).unwrap(),
            42u8.into()
        );
        assert_eq!(
            get_value(&BooleanValueNode::new(true).into()).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            get_value(&StringValueNode::new("hi").into()).unwrap(),
            Value::String("hi".to_string())
        );
    }

    #[test]
    fn nested_struct_with_option_and_array() {
        let node: ValueNode = StructValueNode::new(vec![
            StructFieldValueNode::new("n", NumberValueNode::new(1u8)),
            StructFieldValueNode::new(
                "list",
                ArrayValueNode::new(vec![
                    NumberValueNode::new(2u8).into(),
                    NumberValueNode::new(3u8).into(),
                ]),
            ),
            StructFieldValueNode::new("maybe", SomeValueNode::new(NumberValueNode::new(4u8))),
        ])
        .into();

        let expected = Value::struct_from([
            ("n".into(), 1u8.into()),
            ("list".into(), Value::Array(vec![2u8.into(), 3u8.into()])),
            ("maybe".into(), Value::Option(Some(Box::new(4u8.into())))),
        ]);
        assert_eq!(get_value(&node).unwrap(), expected);
    }

    #[test]
    fn injected_value_uses_fallback_then_override() {
        use codama_nodes::InjectedValueNode;
        use std::collections::HashMap;

        let node = ValueNode::from(InjectedValueNode::with_fallback(
            "amount",
            NumberValueNode::new(7u8),
        ));
        assert_eq!(get_value(&node).unwrap(), 7u8.into());

        let mut injected = HashMap::new();
        injected.insert("amount".into(), 9u8.into());
        assert_eq!(get_value_with(&node, &injected).unwrap(), 9u8.into());

        let missing = ValueNode::from(InjectedValueNode::new("amount"));
        assert!(matches!(
            get_value(&missing).unwrap_err(),
            CodamaError::InjectedValueMissing(_)
        ));
    }
}
