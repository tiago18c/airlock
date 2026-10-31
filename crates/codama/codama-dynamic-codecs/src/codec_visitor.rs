use crate::codec::*;
use crate::struct_type_node_from_instruction_arguments;
use codama_nodes::{
    AccountLinkNode, AccountNode, AmountTypeNode, ArrayTypeNode, BooleanTypeNode, BytesTypeNode,
    ConstantValueNode, CountNode, DateTimeTypeNode, DefinedTypeLinkNode, DefinedTypeNode,
    EnumTypeNode, EnumVariantTypeNode, EventNode, FixedSizeTypeNode, HiddenPrefixTypeNode,
    HiddenSuffixTypeNode, InstructionArgumentNode, InstructionNode, MapTypeNode, NestedTypeNode,
    Node, NumberTypeNode, OptionTypeNode, PostOffsetTypeNode, PreOffsetTypeNode, PublicKeyTypeNode,
    RemainderOptionTypeNode, SentinelTypeNode, SetTypeNode, SizePrefixTypeNode, SolAmountTypeNode,
    StringTypeNode, StructTypeNode, TupleTypeNode, TypeNode, TypeNodeTrait, ZeroableOptionTypeNode,
};
use codama_visitors_core::{
    visit, LinkableDictionary, NodePath, NodeStack, VisitableNode, Visitor,
};
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use std::cell::Cell;
use std::rc::Rc;

/// How deep one type may nest, counting the types its links lead to. A link
/// is followed afresh each time, so an IDL could send the visitor in a circle
/// until the stack runs out. Well above any program's, and it has to fit the
/// smallest stack a codec is built on: a main thread's 1 MiB in a debug build.
pub const MAX_TYPE_DEPTH: usize = 64;

/// How many type nodes one codec may visit. Without a circle, types that each
/// name the next one twice still double the work at every level.
pub const MAX_TYPE_NODES: usize = 1 << 13;

/// Builds [`Codec`]s from Codama type nodes.
///
/// Resolves link nodes through a [`LinkableDictionary`], using a [`NodeStack`]
/// to keep track of the enclosing program/instruction context as it follows
/// links into other parts of the tree.
pub struct CodecVisitor<'a> {
    linkables: &'a LinkableDictionary,
    stack: NodeStack,
    /// Type nodes open now.
    depth: usize,
    /// Type nodes visited so far.
    visited: usize,
    empty_items: Rc<Cell<usize>>,
}

impl<'a> CodecVisitor<'a> {
    pub fn new(linkables: &'a LinkableDictionary, stack: NodeStack) -> Self {
        Self {
            linkables,
            stack,
            depth: 0,
            visited: 0,
            empty_items: Rc::default(),
        }
    }

    /// `item`, counted as an item of one of this codec's lists.
    fn list_item(&self, item: Box<dyn Codec>) -> Box<dyn Codec> {
        Box::new(ListItemCodec {
            item,
            empty_items: self.empty_items.clone(),
        })
    }

    fn nested_codec<T: TypeNodeTrait>(
        &mut self,
        nested: &NestedTypeNode<T>,
    ) -> CodamaResult<Box<dyn Codec>> {
        let ty: TypeNode = nested.clone().into();
        ty.accept(self)
    }

    fn count_to_codec(&mut self, count: &CountNode) -> CodamaResult<CodecCount> {
        Ok(match count {
            CountNode::Fixed(n) => CodecCount::Fixed(n.value as usize),
            CountNode::Prefixed(n) => CodecCount::Prefixed(self.nested_codec(&n.prefix)?),
            CountNode::Remainder(_) => CodecCount::Remainder,
        })
    }

    /// Resolves a link node, then builds the codec of its target — pushing the
    /// resolved path so nested links resolve in the correct program context.
    fn follow_link(&mut self, link: impl Into<Node>) -> CodamaResult<Box<dyn Codec>> {
        let link_path = self.stack.get_path().extended(link);
        let resolved = self.linkables.get_path_or_err(&link_path)?;
        let target = resolved.last().cloned().ok_or(CodamaError::NodeNotFound)?;
        self.stack.push_path(resolved);
        let codec = target.accept(self);
        self.stack.pop_path()?;
        codec
    }

    fn variant_name(variant: &EnumVariantTypeNode) -> String {
        match variant {
            EnumVariantTypeNode::Empty(n) => n.name.to_string(),
            EnumVariantTypeNode::Struct(n) => n.name.to_string(),
            EnumVariantTypeNode::Tuple(n) => n.name.to_string(),
        }
    }

    /// Encodes a `ConstantValueNode` to its bytes using its own type codec.
    fn constant_bytes(&mut self, constant: &ConstantValueNode) -> CodamaResult<Vec<u8>> {
        let type_codec = constant.r#type.accept(self)?;
        let value = constant
            .value
            .accept(&mut crate::value_visitor::ValueVisitor::default())?;
        type_codec.encode_to_vec(&value)
    }
}

fn pascal_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

impl Visitor<Box<dyn Codec>> for CodecVisitor<'_> {
    fn enter_type(&mut self) -> CodamaResult<()> {
        if self.depth >= MAX_TYPE_DEPTH {
            return Err(CodamaError::TypeTooDeep(MAX_TYPE_DEPTH));
        }
        if self.visited >= MAX_TYPE_NODES {
            return Err(CodamaError::TypeTooLarge(MAX_TYPE_NODES));
        }
        self.depth += 1;
        self.visited += 1;
        Ok(())
    }

    fn leave_type(&mut self) {
        self.depth -= 1;
    }

    // ----- Containers -----
    fn visit_account(&mut self, node: &AccountNode) -> CodamaResult<Box<dyn Codec>> {
        self.nested_codec(&node.data)
    }
    fn visit_defined_type(&mut self, node: &DefinedTypeNode) -> CodamaResult<Box<dyn Codec>> {
        node.r#type.accept(self)
    }
    fn visit_event(&mut self, node: &EventNode) -> CodamaResult<Box<dyn Codec>> {
        node.data.accept(self)
    }
    fn visit_instruction(&mut self, node: &InstructionNode) -> CodamaResult<Box<dyn Codec>> {
        let struct_node = struct_type_node_from_instruction_arguments(&node.arguments);
        TypeNode::Struct(struct_node).accept(self)
    }
    fn visit_instruction_argument(
        &mut self,
        node: &InstructionArgumentNode,
    ) -> CodamaResult<Box<dyn Codec>> {
        node.r#type.accept(self)
    }

    // ----- Links -----
    fn visit_defined_type_link(
        &mut self,
        node: &DefinedTypeLinkNode,
    ) -> CodamaResult<Box<dyn Codec>> {
        self.follow_link(node.clone())
    }
    fn visit_account_link(&mut self, node: &AccountLinkNode) -> CodamaResult<Box<dyn Codec>> {
        self.follow_link(node.clone())
    }

    // ----- Numbers and number-delegating types -----
    fn visit_number_type(&mut self, node: &NumberTypeNode) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(NumberCodec {
            format: node.format,
            endian: node.endian,
        }))
    }
    fn visit_amount_type(&mut self, node: &AmountTypeNode) -> CodamaResult<Box<dyn Codec>> {
        self.nested_codec(&node.number)
    }
    fn visit_sol_amount_type(&mut self, node: &SolAmountTypeNode) -> CodamaResult<Box<dyn Codec>> {
        self.nested_codec(&node.number)
    }
    fn visit_date_time_type(&mut self, node: &DateTimeTypeNode) -> CodamaResult<Box<dyn Codec>> {
        self.nested_codec(&node.number)
    }

    // ----- Primitives -----
    fn visit_boolean_type(&mut self, node: &BooleanTypeNode) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(BoolCodec {
            size: self.nested_codec(&node.size)?,
        }))
    }
    fn visit_bytes_type(&mut self, _node: &BytesTypeNode) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(BytesCodec))
    }
    fn visit_string_type(&mut self, node: &StringTypeNode) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(StringCodec {
            encoding: node.encoding,
        }))
    }
    fn visit_public_key_type(&mut self, _node: &PublicKeyTypeNode) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(PublicKeyCodec))
    }

    // ----- Aggregates -----
    fn visit_struct_type(&mut self, node: &StructTypeNode) -> CodamaResult<Box<dyn Codec>> {
        let mut fields = Vec::with_capacity(node.fields.len());
        for field in &node.fields {
            fields.push((field.name.to_string(), field.r#type.accept(self)?));
        }
        Ok(Box::new(StructCodec { fields }))
    }
    fn visit_tuple_type(&mut self, node: &TupleTypeNode) -> CodamaResult<Box<dyn Codec>> {
        let mut items = Vec::with_capacity(node.items.len());
        for item in &node.items {
            items.push(item.accept(self)?);
        }
        Ok(Box::new(TupleCodec { items }))
    }
    fn visit_array_type(&mut self, node: &ArrayTypeNode) -> CodamaResult<Box<dyn Codec>> {
        let item = node.item.accept(self)?;
        let count = self.count_to_codec(&node.count)?;
        Ok(Box::new(ArrayCodec {
            item: self.list_item(item),
            count,
            as_set: false,
        }))
    }
    fn visit_set_type(&mut self, node: &SetTypeNode) -> CodamaResult<Box<dyn Codec>> {
        let item = node.item.accept(self)?;
        let count = self.count_to_codec(&node.count)?;
        Ok(Box::new(ArrayCodec {
            item: self.list_item(item),
            count,
            as_set: true,
        }))
    }
    fn visit_option_type(&mut self, node: &OptionTypeNode) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(OptionCodec {
            prefix: self.nested_codec(&node.prefix)?,
            item: node.item.accept(self)?,
            fixed: node.fixed.unwrap_or(false),
        }))
    }

    // ----- Enums -----
    fn visit_enum_type(&mut self, node: &EnumTypeNode) -> CodamaResult<Box<dyn Codec>> {
        let size = self.nested_codec(&node.size)?;
        let is_scalar = node
            .variants
            .iter()
            .all(|v| matches!(v, EnumVariantTypeNode::Empty(_)));
        if is_scalar {
            let variants = node.variants.iter().map(Self::variant_name).collect();
            return Ok(Box::new(ScalarEnumCodec { size, variants }));
        }
        let mut variants = Vec::with_capacity(node.variants.len());
        for variant in &node.variants {
            let name = pascal_case(&Self::variant_name(variant));
            let codec: Box<dyn Codec> = match variant {
                EnumVariantTypeNode::Empty(_) => Box::new(StructCodec { fields: vec![] }),
                EnumVariantTypeNode::Struct(n) => self.nested_codec(&n.r#struct)?,
                EnumVariantTypeNode::Tuple(n) => {
                    // Tuple variants are encoded as a struct with a `fields` tuple.
                    let tuple = self.nested_codec(&n.tuple)?;
                    Box::new(StructCodec {
                        fields: vec![("fields".to_string(), tuple)],
                    })
                }
            };
            variants.push((name, codec));
        }
        Ok(Box::new(DataEnumCodec { size, variants }))
    }

    // ----- Wrappers -----
    fn visit_fixed_size_type(
        &mut self,
        node: &FixedSizeTypeNode<TypeNode>,
    ) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(FixedSizeCodec {
            inner: node.r#type.accept(self)?,
            size: node.size,
        }))
    }
    fn visit_size_prefix_type(
        &mut self,
        node: &SizePrefixTypeNode<TypeNode>,
    ) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(SizePrefixCodec {
            inner: node.r#type.accept(self)?,
            prefix: self.nested_codec(&node.prefix)?,
        }))
    }

    fn visit_sentinel_type(
        &mut self,
        node: &SentinelTypeNode<TypeNode>,
    ) -> CodamaResult<Box<dyn Codec>> {
        let inner = node.r#type.accept(self)?;
        let sentinel = self.constant_bytes(&node.sentinel)?;
        Ok(Box::new(SentinelCodec { inner, sentinel }))
    }

    fn visit_hidden_prefix_type(
        &mut self,
        node: &HiddenPrefixTypeNode<TypeNode>,
    ) -> CodamaResult<Box<dyn Codec>> {
        let inner = node.r#type.accept(self)?;
        let constants = node
            .prefix
            .iter()
            .map(|c| self.constant_bytes(c))
            .collect::<CodamaResult<Vec<_>>>()?;
        Ok(Box::new(HiddenPrefixCodec { inner, constants }))
    }

    fn visit_hidden_suffix_type(
        &mut self,
        node: &HiddenSuffixTypeNode<TypeNode>,
    ) -> CodamaResult<Box<dyn Codec>> {
        let inner = node.r#type.accept(self)?;
        let constants = node
            .suffix
            .iter()
            .map(|c| self.constant_bytes(c))
            .collect::<CodamaResult<Vec<_>>>()?;
        Ok(Box::new(HiddenSuffixCodec { inner, constants }))
    }

    fn visit_pre_offset_type(
        &mut self,
        node: &PreOffsetTypeNode<TypeNode>,
    ) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(PreOffsetCodec {
            inner: node.r#type.accept(self)?,
            offset: node.offset,
            strategy: node.strategy,
        }))
    }

    fn visit_post_offset_type(
        &mut self,
        node: &PostOffsetTypeNode<TypeNode>,
    ) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(PostOffsetCodec {
            inner: node.r#type.accept(self)?,
            offset: node.offset,
            strategy: node.strategy,
        }))
    }

    fn visit_zeroable_option_type(
        &mut self,
        node: &ZeroableOptionTypeNode,
    ) -> CodamaResult<Box<dyn Codec>> {
        let item = node.item.accept(self)?;
        let none_value = match &node.zero_value {
            Some(c) => Some(self.constant_bytes(c)?),
            None => None,
        };
        Ok(Box::new(ZeroableOptionCodec { item, none_value }))
    }

    fn visit_remainder_option_type(
        &mut self,
        node: &RemainderOptionTypeNode,
    ) -> CodamaResult<Box<dyn Codec>> {
        Ok(Box::new(RemainderOptionCodec {
            item: node.item.accept(self)?,
        }))
    }

    fn visit_map_type(&mut self, node: &MapTypeNode) -> CodamaResult<Box<dyn Codec>> {
        let key = node.key.accept(self)?;
        let value = node.value.accept(self)?;
        let count = self.count_to_codec(&node.count)?;
        Ok(Box::new(MapCodec {
            key: self.list_item(key),
            value,
            count,
        }))
    }
}

/// Builds a codec for the node at the end of `path`.
///
/// Link nodes are resolved against a [`LinkableDictionary`] recorded from the
/// root node at the head of the path (so links must be reachable from it). The
/// nodes preceding the target seed the program/instruction context.
pub fn get_node_codec(path: &NodePath) -> CodamaResult<Box<dyn Codec>> {
    let target = path.last().cloned().ok_or(CodamaError::NodeNotFound)?;
    let linkables = match path.first() {
        Some(Node::Root(root)) => LinkableDictionary::record_all(root),
        _ => LinkableDictionary::new(),
    };
    let ancestors = NodePath(path[..path.len() - 1].to_vec());
    let mut visitor = CodecVisitor::new(&linkables, NodeStack::from_paths(vec![ancestors]));
    visit(&target, &mut visitor)
}

/// Builds a codec for a standalone type node (no link resolution context).
pub fn get_type_codec(type_node: &TypeNode) -> CodamaResult<Box<dyn Codec>> {
    let linkables = LinkableDictionary::new();
    let mut visitor = CodecVisitor::new(&linkables, NodeStack::new());
    type_node.accept(&mut visitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;
    use codama_nodes::{
        DefinedTypeNode, Endianness, NumberFormat::*, ProgramNode, RootNode, StructFieldTypeNode,
    };

    #[test]
    fn builds_struct_codec_from_type_node() {
        let ty = TypeNode::Struct(StructTypeNode::new(vec![
            StructFieldTypeNode::new("a", NumberTypeNode::le(U8)),
            StructFieldTypeNode::new("b", NumberTypeNode::new(U16, Endianness::Le)),
        ]));
        let codec = get_type_codec(&ty).unwrap();
        let value = Value::struct_from([("a".into(), 1u8.into()), ("b".into(), 2u16.into())]);
        assert_eq!(codec.encode_to_vec(&value).unwrap(), vec![1, 2, 0]);
    }

    #[test]
    fn resolves_defined_type_link_through_dictionary() {
        // A program with a defined type `seed = u32`, and a struct referencing it.
        let program = ProgramNode {
            name: "myProgram".into(),
            defined_types: vec![DefinedTypeNode::new("seed", NumberTypeNode::le(U32))],
            ..Default::default()
        };
        let root = RootNode::new(program);

        let target = TypeNode::Struct(StructTypeNode::new(vec![StructFieldTypeNode::new(
            "field",
            DefinedTypeLinkNode::new("seed"),
        )]));
        // Path: root > program > (target struct as the account-like node).
        let path = NodePath::new()
            .extended(root.clone())
            .extended(root.program.clone())
            .extended(Node::Type(codama_nodes::RegisteredTypeNode::Struct(
                match &target {
                    TypeNode::Struct(s) => s.clone(),
                    _ => unreachable!(),
                },
            )));
        let codec = get_node_codec(&path).unwrap();
        let value = Value::struct_from([("field".into(), 7u32.into())]);
        assert_eq!(codec.encode_to_vec(&value).unwrap(), vec![7, 0, 0, 0]);
    }

    #[test]
    fn scalar_vs_data_enum() {
        // Built directly from EnumTypeNode via get_type_codec.
        use codama_nodes::{EnumEmptyVariantTypeNode, EnumVariantTypeNode};
        let scalar = TypeNode::Enum(EnumTypeNode::new(vec![
            EnumVariantTypeNode::Empty(EnumEmptyVariantTypeNode::new("a")),
            EnumVariantTypeNode::Empty(EnumEmptyVariantTypeNode::new("b")),
        ]));
        let codec = get_type_codec(&scalar).unwrap();
        assert_eq!(
            codec.encode_to_vec(&Value::unit_enum("b")).unwrap(),
            vec![1]
        );
    }
}
