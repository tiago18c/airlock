//! Collecting the PDA definitions reachable from an IDL.

use codama_nodes::{InstructionInputValueNode, PdaNode, PdaValuePda, RootNode};
use indexmap::IndexMap;

/// Collects unique PDA definitions from the IDL, keyed by name.
///
/// Scans `root.program.pdas` (registered PDAs) first, then inline
/// `PdaValueNode > PdaNode` definitions inside instruction account default
/// values. Registered PDAs take precedence; inline PDAs are added only when
/// their name is not already present. Counterpart of the TypeScript
/// `collectPdaNodes`.
pub fn collect_pda_nodes(root: &RootNode) -> IndexMap<String, PdaNode> {
    let mut pdas = IndexMap::new();

    for pda in &root.program.pdas {
        pdas.insert(pda.name.to_string(), pda.clone());
    }

    for instruction in &root.program.instructions {
        for account in &instruction.accounts {
            let Some(InstructionInputValueNode::PdaValue(pda_value)) =
                account.default_value.as_ref().as_ref()
            else {
                continue;
            };
            if let PdaValuePda::Pda(pda_node) = pda_value.pda.as_ref() {
                pdas.entry(pda_node.name.to_string())
                    .or_insert_with(|| pda_node.clone());
            }
        }
    }

    pdas
}
