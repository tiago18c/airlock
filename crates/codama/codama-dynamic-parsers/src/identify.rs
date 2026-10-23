//! Identifying which node a byte buffer belongs to, by discriminator.
//!
//! Rust counterpart of the TypeScript `dynamic-parsers/identify`. Only the
//! root's main program is searched, and candidates are tried in the order
//! accounts → events → instructions, returning the first match — matching the
//! TypeScript traversal.

use crate::discriminators::match_discriminators;
use codama_dynamic_codecs::struct_type_node_from_instruction_arguments;
use codama_nodes::{NestedTypeNodeTrait, Node, ProgramNode, RootNode, StructTypeNode, TypeNode};
use codama_visitors_core::DynamicResult as CodamaResult;
use codama_visitors_core::NodePath;

/// The kinds of node that can be identified from raw data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsableKind {
    Account,
    Event,
    Instruction,
}

/// All identifiable kinds, in traversal order.
pub const ALL_PARSABLE_KINDS: [ParsableKind; 3] = [
    ParsableKind::Account,
    ParsableKind::Event,
    ParsableKind::Instruction,
];

/// Identifies the account whose discriminators match `bytes`.
pub fn identify_account_data(root: &RootNode, bytes: &[u8]) -> CodamaResult<Option<NodePath>> {
    identify_data(root, bytes, &[ParsableKind::Account])
}

/// Identifies the event whose discriminators match `bytes`.
pub fn identify_event_data(root: &RootNode, bytes: &[u8]) -> CodamaResult<Option<NodePath>> {
    identify_data(root, bytes, &[ParsableKind::Event])
}

/// Identifies the instruction whose discriminators match `bytes`.
pub fn identify_instruction_data(root: &RootNode, bytes: &[u8]) -> CodamaResult<Option<NodePath>> {
    identify_data(root, bytes, &[ParsableKind::Instruction])
}

/// Identifies the node (of one of the requested `kinds`) whose discriminators
/// match `bytes`. Returns the [`NodePath`] `[root, program, node]` of the match,
/// or `None` if nothing matches.
pub fn identify_data(
    root: &RootNode,
    bytes: &[u8],
    kinds: &[ParsableKind],
) -> CodamaResult<Option<NodePath>> {
    let program = &root.program;

    if kinds.contains(&ParsableKind::Account) {
        for account in &program.accounts {
            if account.discriminators.is_empty() {
                continue;
            }
            let struct_type = account.data.get_nested_type_node().clone();
            if match_discriminators(bytes, &account.discriminators, Some(&struct_type), root)? {
                return Ok(Some(node_path(root, program, account.clone().into())));
            }
        }
    }

    if kinds.contains(&ParsableKind::Event) {
        for event in &program.events {
            if event.discriminators.is_empty() {
                continue;
            }
            let struct_type = resolve_struct_type(&event.data);
            if match_discriminators(bytes, &event.discriminators, struct_type.as_ref(), root)? {
                return Ok(Some(node_path(root, program, event.clone().into())));
            }
        }
    }

    if kinds.contains(&ParsableKind::Instruction) {
        for instruction in &program.instructions {
            if instruction.discriminators.is_empty() {
                continue;
            }
            let struct_type = struct_type_node_from_instruction_arguments(&instruction.arguments);
            if match_discriminators(bytes, &instruction.discriminators, Some(&struct_type), root)? {
                return Ok(Some(node_path(root, program, instruction.clone().into())));
            }
        }
    }

    Ok(None)
}

/// Resolves a (possibly wrapped) type node down to its inner struct, if any.
fn resolve_struct_type(data: &TypeNode) -> Option<StructTypeNode> {
    codama_nodes::NestedTypeNode::<StructTypeNode>::try_from(data.clone())
        .ok()
        .map(|nested| nested.get_nested_type_node().clone())
}

fn node_path(root: &RootNode, program: &ProgramNode, node: Node) -> NodePath {
    NodePath::new()
        .extended(root.clone())
        .extended(program.clone())
        .extended(node)
}
