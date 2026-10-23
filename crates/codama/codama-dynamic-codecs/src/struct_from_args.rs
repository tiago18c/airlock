//! Turn instruction arguments into a struct type so they can be encoded with
//! the same codec path as account data.

use codama_nodes::{
    InstructionArgumentNode, InstructionInputValueNode, StructFieldTypeNode, StructTypeNode,
    ValueNode,
};

/// Builds a [`StructTypeNode`] from instruction arguments, one field per argument.
///
/// Counterpart of the TypeScript `structTypeNodeFromInstructionArgumentNodes`.
pub fn struct_type_node_from_instruction_arguments(
    nodes: &[InstructionArgumentNode],
) -> StructTypeNode {
    StructTypeNode::new(
        nodes
            .iter()
            .map(struct_field_type_node_from_instruction_argument)
            .collect(),
    )
}

/// Converts an instruction argument into a struct field.
///
/// The default value is kept only when it is a plain [`ValueNode`]. Contextual
/// and resolver defaults are dropped, along with their strategy, matching
/// `structFieldTypeNodeFromInstructionArgumentNode`.
pub fn struct_field_type_node_from_instruction_argument(
    node: &InstructionArgumentNode,
) -> StructFieldTypeNode {
    let default_value = node
        .default_value
        .as_ref()
        .as_ref()
        .and_then(|value| ValueNode::try_from(InstructionInputValueNode::clone(value)).ok());
    let strategy = if default_value.is_some() {
        node.default_value_strategy
    } else {
        None
    };
    StructFieldTypeNode {
        name: node.name.clone(),
        default_value_strategy: strategy,
        docs: node.docs.clone(),
        r#type: node.r#type.clone(),
        default_value: Box::new(default_value),
        display: None,
    }
}
