use crate::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use codama_nodes::*;

/// A visitor over the Codama node tree.
///
/// This mirrors the TypeScript `Visitor<T>` type: there is one `visit_*` method
/// per registered node kind. Every method has a default implementation that
/// delegates to [`visit_fallback`](Visitor::visit_fallback), so a concrete
/// visitor only needs to override the kinds it actually supports. The default
/// `visit_fallback` returns a [`CodamaError::UnsupportedNodeKind`] error.
///
/// Dispatch is performed by [`visit`] (or [`VisitableNode::accept`]), which
/// matches on the concrete node kind and calls the corresponding method.
#[allow(unused_variables)]
pub trait Visitor<T> {
    /// Called by every default `visit_*` method. Override to provide a single
    /// catch-all behaviour for unsupported nodes.
    fn visit_fallback(&mut self, kind: &str) -> CodamaResult<T> {
        Err(CodamaError::UnsupportedNodeKind(kind.to_string()))
    }

    // Top-level nodes.
    fn visit_root(&mut self, node: &RootNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_program(&mut self, node: &ProgramNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_pda(&mut self, node: &PdaNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_account(&mut self, node: &AccountNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_event(&mut self, node: &EventNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_constant(&mut self, node: &ConstantNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_defined_type(&mut self, node: &DefinedTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_error(&mut self, node: &ErrorNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction(&mut self, node: &InstructionNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_account(&mut self, node: &InstructionAccountNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_argument(&mut self, node: &InstructionArgumentNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_byte_delta(&mut self, node: &InstructionByteDeltaNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_remaining_accounts(
        &mut self,
        node: &InstructionRemainingAccountsNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_status(&mut self, node: &InstructionStatusNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }

    // Type nodes.
    fn visit_amount_type(&mut self, node: &AmountTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_array_type(&mut self, node: &ArrayTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_boolean_type(&mut self, node: &BooleanTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_bytes_type(&mut self, node: &BytesTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_date_time_type(&mut self, node: &DateTimeTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_enum_type(&mut self, node: &EnumTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_fixed_size_type(&mut self, node: &FixedSizeTypeNode<TypeNode>) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_hidden_prefix_type(
        &mut self,
        node: &HiddenPrefixTypeNode<TypeNode>,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_hidden_suffix_type(
        &mut self,
        node: &HiddenSuffixTypeNode<TypeNode>,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_map_type(&mut self, node: &MapTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_number_type(&mut self, node: &NumberTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_option_type(&mut self, node: &OptionTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_post_offset_type(&mut self, node: &PostOffsetTypeNode<TypeNode>) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_pre_offset_type(&mut self, node: &PreOffsetTypeNode<TypeNode>) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_public_key_type(&mut self, node: &PublicKeyTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_remainder_option_type(&mut self, node: &RemainderOptionTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_sentinel_type(&mut self, node: &SentinelTypeNode<TypeNode>) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_set_type(&mut self, node: &SetTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_size_prefix_type(&mut self, node: &SizePrefixTypeNode<TypeNode>) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_sol_amount_type(&mut self, node: &SolAmountTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_string_type(&mut self, node: &StringTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_struct_type(&mut self, node: &StructTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_tuple_type(&mut self, node: &TupleTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_zeroable_option_type(&mut self, node: &ZeroableOptionTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_struct_field_type(&mut self, node: &StructFieldTypeNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_enum_empty_variant_type(
        &mut self,
        node: &EnumEmptyVariantTypeNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_enum_struct_variant_type(
        &mut self,
        node: &EnumStructVariantTypeNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_enum_tuple_variant_type(
        &mut self,
        node: &EnumTupleVariantTypeNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }

    // Link nodes.
    fn visit_account_link(&mut self, node: &AccountLinkNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_defined_type_link(&mut self, node: &DefinedTypeLinkNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_link(&mut self, node: &InstructionLinkNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_account_link(
        &mut self,
        node: &InstructionAccountLinkNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_argument_link(
        &mut self,
        node: &InstructionArgumentLinkNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_pda_link(&mut self, node: &PdaLinkNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_program_link(&mut self, node: &ProgramLinkNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }

    // Value nodes.
    fn visit_array_value(&mut self, node: &ArrayValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_boolean_value(&mut self, node: &BooleanValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_bytes_value(&mut self, node: &BytesValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_constant_value(&mut self, node: &ConstantValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_enum_value(&mut self, node: &EnumValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_injected_value(&mut self, node: &InjectedValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_map_value(&mut self, node: &MapValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_none_value(&mut self, node: &NoneValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_number_value(&mut self, node: &NumberValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_public_key_value(&mut self, node: &PublicKeyValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_set_value(&mut self, node: &SetValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_some_value(&mut self, node: &SomeValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_string_value(&mut self, node: &StringValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_struct_value(&mut self, node: &StructValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_tuple_value(&mut self, node: &TupleValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_struct_field_value(&mut self, node: &StructFieldValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_map_entry_value(&mut self, node: &MapEntryValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }

    // Contextual value nodes.
    fn visit_account_value(&mut self, node: &AccountValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_account_bump_value(&mut self, node: &AccountBumpValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_account_field_value(&mut self, node: &AccountFieldValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_argument_value(&mut self, node: &ArgumentValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_conditional_value(&mut self, node: &ConditionalValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_identity_value(&mut self, node: &IdentityValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_payer_value(&mut self, node: &PayerValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_pda_value(&mut self, node: &PdaValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_program_id_value(&mut self, node: &ProgramIdValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_resolver_value(&mut self, node: &ResolverValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_pda_seed_value(&mut self, node: &PdaSeedValueNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }

    // Count nodes.
    fn visit_fixed_count(&mut self, node: &FixedCountNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_prefixed_count(&mut self, node: &PrefixedCountNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_remainder_count(&mut self, node: &RemainderCountNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }

    // Discriminator nodes.
    fn visit_constant_discriminator(
        &mut self,
        node: &ConstantDiscriminatorNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_field_discriminator(&mut self, node: &FieldDiscriminatorNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_size_discriminator(&mut self, node: &SizeDiscriminatorNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }

    // Pda seed nodes.
    fn visit_constant_pda_seed(&mut self, node: &ConstantPdaSeedNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_variable_pda_seed(&mut self, node: &VariablePdaSeedNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }

    fn visit_plugin(&mut self, node: &PluginNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_provided(&mut self, node: &ProvidedNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_amount_number_display(&mut self, node: &AmountNumberDisplayNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_date_time_number_display(
        &mut self,
        node: &DateTimeNumberDisplayNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_duration_number_display(
        &mut self,
        node: &DurationNumberDisplayNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_enum_variant_display(&mut self, node: &EnumVariantDisplayNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_display(&mut self, node: &InstructionDisplayNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_instruction_account_display(
        &mut self,
        node: &InstructionAccountDisplayNode,
    ) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_string_display(&mut self, node: &StringDisplayNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
    fn visit_struct_field_display(&mut self, node: &StructFieldDisplayNode) -> CodamaResult<T> {
        self.visit_fallback(node.kind())
    }
}

/// Dispatches a node to the corresponding [`Visitor`] method.
pub fn visit<T, V: Visitor<T> + ?Sized>(node: &Node, visitor: &mut V) -> CodamaResult<T> {
    node.accept(visitor)
}

/// A node (or node union) that knows how to dispatch itself to a [`Visitor`].
pub trait VisitableNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, visitor: &mut V) -> CodamaResult<T>;
}

impl VisitableNode for Node {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            Node::ContextualValue(node) => node.accept(v),
            Node::Count(node) => node.accept(v),
            Node::Discriminator(node) => node.accept(v),
            Node::Link(node) => node.accept(v),
            Node::PdaSeed(node) => node.accept(v),
            Node::Type(node) => node.accept(v),
            Node::Value(node) => node.accept(v),
            Node::Account(node) => v.visit_account(node),
            Node::Constant(node) => v.visit_constant(node),
            Node::DefinedType(node) => v.visit_defined_type(node),
            Node::Error(node) => v.visit_error(node),
            Node::Event(node) => v.visit_event(node),
            Node::Instruction(node) => v.visit_instruction(node),
            Node::InstructionAccount(node) => v.visit_instruction_account(node),
            Node::InstructionArgument(node) => v.visit_instruction_argument(node),
            Node::InstructionByteDelta(node) => v.visit_instruction_byte_delta(node),
            Node::InstructionRemainingAccounts(node) => {
                v.visit_instruction_remaining_accounts(node)
            }
            Node::InstructionStatus(node) => v.visit_instruction_status(node),
            Node::Pda(node) => v.visit_pda(node),
            Node::Plugin(node) => v.visit_plugin(node),
            Node::Program(node) => v.visit_program(node),
            Node::Provided(node) => v.visit_provided(node),
            Node::Root(node) => v.visit_root(node),
            Node::Display(node) => node.accept(v),
        }
    }
}

/// Generates the shared standalone type-node match arms for both `TypeNode`
/// and `RegisteredTypeNode` (their 24 standalone variants are identical).
macro_rules! standalone_type_node_arms {
    ($enum:ident, $self:ident, $v:ident) => {
        match $self {
            $enum::Amount(n) => $v.visit_amount_type(n),
            $enum::Array(n) => $v.visit_array_type(n),
            $enum::Boolean(n) => $v.visit_boolean_type(n),
            $enum::Bytes(n) => $v.visit_bytes_type(n),
            $enum::DateTime(n) => $v.visit_date_time_type(n),
            $enum::Enum(n) => $v.visit_enum_type(n),
            $enum::FixedSize(n) => $v.visit_fixed_size_type(n),
            $enum::HiddenPrefix(n) => $v.visit_hidden_prefix_type(n),
            $enum::HiddenSuffix(n) => $v.visit_hidden_suffix_type(n),
            $enum::Map(n) => $v.visit_map_type(n),
            $enum::Number(n) => $v.visit_number_type(n),
            $enum::Option(n) => $v.visit_option_type(n),
            $enum::PostOffset(n) => $v.visit_post_offset_type(n),
            $enum::PreOffset(n) => $v.visit_pre_offset_type(n),
            $enum::PublicKey(n) => $v.visit_public_key_type(n),
            $enum::RemainderOption(n) => $v.visit_remainder_option_type(n),
            $enum::Sentinel(n) => $v.visit_sentinel_type(n),
            $enum::Set(n) => $v.visit_set_type(n),
            $enum::SizePrefix(n) => $v.visit_size_prefix_type(n),
            $enum::SolAmount(n) => $v.visit_sol_amount_type(n),
            $enum::String(n) => $v.visit_string_type(n),
            $enum::Struct(n) => $v.visit_struct_type(n),
            $enum::Tuple(n) => $v.visit_tuple_type(n),
            $enum::ZeroableOption(n) => $v.visit_zeroable_option_type(n),
            #[allow(unreachable_patterns)]
            _ => unreachable!("non-standalone type node variant"),
        }
    };
}

impl VisitableNode for TypeNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            TypeNode::Link(n) => v.visit_defined_type_link(n),
            other => standalone_type_node_arms!(TypeNode, other, v),
        }
    }
}

impl VisitableNode for RegisteredTypeNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            RegisteredTypeNode::StructField(n) => v.visit_struct_field_type(n),
            RegisteredTypeNode::EnumEmptyVariant(n) => v.visit_enum_empty_variant_type(n),
            RegisteredTypeNode::EnumStructVariant(n) => v.visit_enum_struct_variant_type(n),
            RegisteredTypeNode::EnumTupleVariant(n) => v.visit_enum_tuple_variant_type(n),
            other => standalone_type_node_arms!(RegisteredTypeNode, other, v),
        }
    }
}

macro_rules! standalone_value_node_arms {
    ($enum:ident, $self:ident, $v:ident) => {
        match $self {
            $enum::Array(n) => $v.visit_array_value(n),
            $enum::Boolean(n) => $v.visit_boolean_value(n),
            $enum::Bytes(n) => $v.visit_bytes_value(n),
            $enum::Constant(n) => $v.visit_constant_value(n),
            $enum::Enum(n) => $v.visit_enum_value(n),
            $enum::Injected(n) => $v.visit_injected_value(n),
            $enum::Map(n) => $v.visit_map_value(n),
            $enum::None(n) => $v.visit_none_value(n),
            $enum::Number(n) => $v.visit_number_value(n),
            $enum::PublicKey(n) => $v.visit_public_key_value(n),
            $enum::Set(n) => $v.visit_set_value(n),
            $enum::Some(n) => $v.visit_some_value(n),
            $enum::String(n) => $v.visit_string_value(n),
            $enum::Struct(n) => $v.visit_struct_value(n),
            $enum::Tuple(n) => $v.visit_tuple_value(n),
            #[allow(unreachable_patterns)]
            _ => unreachable!("non-standalone value node variant"),
        }
    };
}

impl VisitableNode for ValueNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        standalone_value_node_arms!(ValueNode, self, v)
    }
}

impl VisitableNode for RegisteredValueNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            RegisteredValueNode::StructField(n) => v.visit_struct_field_value(n),
            RegisteredValueNode::MapEntry(n) => v.visit_map_entry_value(n),
            other => standalone_value_node_arms!(RegisteredValueNode, other, v),
        }
    }
}

macro_rules! standalone_contextual_value_node_arms {
    ($enum:ident, $self:ident, $v:ident) => {
        match $self {
            $enum::Account(n) => $v.visit_account_value(n),
            $enum::AccountBump(n) => $v.visit_account_bump_value(n),
            $enum::AccountField(n) => $v.visit_account_field_value(n),
            $enum::Argument(n) => $v.visit_argument_value(n),
            $enum::Conditional(n) => $v.visit_conditional_value(n),
            $enum::Identity(n) => $v.visit_identity_value(n),
            $enum::Payer(n) => $v.visit_payer_value(n),
            $enum::Pda(n) => $v.visit_pda_value(n),
            $enum::ProgramId(n) => $v.visit_program_id_value(n),
            $enum::Resolver(n) => $v.visit_resolver_value(n),
            #[allow(unreachable_patterns)]
            _ => unreachable!("non-standalone contextual value node variant"),
        }
    };
}

impl VisitableNode for ContextualValueNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        standalone_contextual_value_node_arms!(ContextualValueNode, self, v)
    }
}

impl VisitableNode for RegisteredContextualValueNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            RegisteredContextualValueNode::PdaSeed(n) => v.visit_pda_seed_value(n),
            other => {
                standalone_contextual_value_node_arms!(RegisteredContextualValueNode, other, v)
            }
        }
    }
}

impl VisitableNode for LinkNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            LinkNode::Account(n) => v.visit_account_link(n),
            LinkNode::DefinedType(n) => v.visit_defined_type_link(n),
            LinkNode::Instruction(n) => v.visit_instruction_link(n),
            LinkNode::InstructionAccount(n) => v.visit_instruction_account_link(n),
            LinkNode::InstructionArgument(n) => v.visit_instruction_argument_link(n),
            LinkNode::Pda(n) => v.visit_pda_link(n),
            LinkNode::Program(n) => v.visit_program_link(n),
        }
    }
}

impl VisitableNode for CountNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            CountNode::Fixed(n) => v.visit_fixed_count(n),
            CountNode::Prefixed(n) => v.visit_prefixed_count(n),
            CountNode::Remainder(n) => v.visit_remainder_count(n),
        }
    }
}

impl VisitableNode for DiscriminatorNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            DiscriminatorNode::Constant(n) => v.visit_constant_discriminator(n),
            DiscriminatorNode::Field(n) => v.visit_field_discriminator(n),
            DiscriminatorNode::Size(n) => v.visit_size_discriminator(n),
        }
    }
}

impl VisitableNode for PdaSeedNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            PdaSeedNode::Constant(n) => v.visit_constant_pda_seed(n),
            PdaSeedNode::Variable(n) => v.visit_variable_pda_seed(n),
        }
    }
}

impl VisitableNode for DisplayNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        match self {
            DisplayNode::AmountNumber(n) => v.visit_amount_number_display(n),
            DisplayNode::DateTimeNumber(n) => v.visit_date_time_number_display(n),
            DisplayNode::DurationNumber(n) => v.visit_duration_number_display(n),
            DisplayNode::EnumVariant(n) => v.visit_enum_variant_display(n),
            DisplayNode::Instruction(n) => v.visit_instruction_display(n),
            DisplayNode::InstructionAccount(n) => v.visit_instruction_account_display(n),
            DisplayNode::String(n) => v.visit_string_display(n),
            DisplayNode::StructField(n) => v.visit_struct_field_display(n),
        }
    }
}

impl VisitableNode for InstructionInputValueNode {
    fn accept<T, V: Visitor<T> + ?Sized>(&self, v: &mut V) -> CodamaResult<T> {
        use InstructionInputValueNode as N;
        match self {
            N::AccountBumpValue(n) => v.visit_account_bump_value(n),
            N::AccountFieldValue(n) => v.visit_account_field_value(n),
            N::AccountValue(n) => v.visit_account_value(n),
            N::ArgumentValue(n) => v.visit_argument_value(n),
            N::ArrayValue(n) => v.visit_array_value(n),
            N::BooleanValue(n) => v.visit_boolean_value(n),
            N::BytesValue(n) => v.visit_bytes_value(n),
            N::ConditionalValue(n) => v.visit_conditional_value(n),
            N::ConstantValue(n) => v.visit_constant_value(n),
            N::EnumValue(n) => v.visit_enum_value(n),
            N::IdentityValue(n) => v.visit_identity_value(n),
            N::InjectedValue(n) => v.visit_injected_value(n),
            N::MapValue(n) => v.visit_map_value(n),
            N::NoneValue(n) => v.visit_none_value(n),
            N::NumberValue(n) => v.visit_number_value(n),
            N::PayerValue(n) => v.visit_payer_value(n),
            N::PdaValue(n) => v.visit_pda_value(n),
            N::ProgramIdValue(n) => v.visit_program_id_value(n),
            N::ProgramLink(n) => v.visit_program_link(n),
            N::PublicKeyValue(n) => v.visit_public_key_value(n),
            N::ResolverValue(n) => v.visit_resolver_value(n),
            N::SetValue(n) => v.visit_set_value(n),
            N::SomeValue(n) => v.visit_some_value(n),
            N::StringValue(n) => v.visit_string_value(n),
            N::StructValue(n) => v.visit_struct_value(n),
            N::TupleValue(n) => v.visit_tuple_value(n),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A visitor that returns the kind string of supported nodes and falls back
    /// to a tagged error for everything else.
    #[derive(Default)]
    struct KindVisitor;

    impl Visitor<String> for KindVisitor {
        fn visit_number_type(&mut self, node: &NumberTypeNode) -> CodamaResult<String> {
            Ok(format!("number:{}", node.kind()))
        }
        fn visit_struct_type(&mut self, _node: &StructTypeNode) -> CodamaResult<String> {
            Ok("struct".to_string())
        }
        fn visit_program(&mut self, node: &ProgramNode) -> CodamaResult<String> {
            Ok(format!("program:{}", node.name.as_str()))
        }
    }

    #[test]
    fn dispatches_to_overridden_methods() {
        let mut v = KindVisitor;
        let number: Node = NumberTypeNode::le(U32).into();
        assert_eq!(visit(&number, &mut v).unwrap(), "number:numberTypeNode");

        let program: Node = ProgramNode {
            name: "myProgram".into(),
            ..Default::default()
        }
        .into();
        assert_eq!(visit(&program, &mut v).unwrap(), "program:myProgram");
    }

    #[test]
    fn dispatches_through_type_node_union() {
        let mut v = KindVisitor;
        let ty = TypeNode::Struct(StructTypeNode::new(vec![]));
        assert_eq!(ty.accept(&mut v).unwrap(), "struct");
    }

    #[test]
    fn unsupported_node_falls_back_to_error() {
        let mut v = KindVisitor;
        let node: Node = ProgramIdValueNode::new().into();
        assert!(matches!(
            visit(&node, &mut v),
            Err(CodamaError::UnsupportedNodeKind(kind)) if kind == "programIdValueNode"
        ));
    }
}
