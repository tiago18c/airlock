use codama_nodes::{HasKind, InstructionNode, Node, ProgramNode};
use std::ops::Deref;

/// An ordered chain of nodes from an ancestor down to a target node.
///
/// This is the Rust counterpart of the TypeScript `NodePath` type. Unlike the
/// TypeScript version — which borrows nodes from a shared tree — this owns its
/// nodes (they are cheap `Clone`s of IDL nodes) to avoid threading lifetimes
/// through the visitor and linkable machinery.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NodePath(pub Vec<Node>);

impl NodePath {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Returns a new path with `node` appended at the end.
    pub fn extended(&self, node: impl Into<Node>) -> Self {
        let mut nodes = self.0.clone();
        nodes.push(node.into());
        Self(nodes)
    }

    /// The target node, i.e. the last node in the path.
    pub fn last(&self) -> Option<&Node> {
        self.0.last()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The first node in the path whose kind matches `kind`.
    pub fn find_first(&self, kind: &str) -> Option<&Node> {
        self.0.iter().find(|node| node.kind() == kind)
    }

    /// The last node in the path whose kind matches `kind`.
    pub fn find_last(&self, kind: &str) -> Option<&Node> {
        self.0.iter().rfind(|node| node.kind() == kind)
    }

    /// The closest enclosing program node, searching from the target upwards.
    pub fn find_program(&self) -> Option<&ProgramNode> {
        match self.find_last("programNode") {
            Some(Node::Program(node)) => Some(node),
            _ => None,
        }
    }

    /// The closest enclosing instruction node, searching from the target upwards.
    pub fn find_instruction(&self) -> Option<&InstructionNode> {
        match self.find_last("instructionNode") {
            Some(Node::Instruction(node)) => Some(node),
            _ => None,
        }
    }

    /// The sub-path ending at the last node of the given `kind` (inclusive),
    /// or `None` if no such node exists in the path.
    pub fn until_last_node(&self, kind: &str) -> Option<NodePath> {
        let last_index = self.0.iter().rposition(|node| node.kind() == kind)?;
        Some(NodePath(self.0[..=last_index].to_vec()))
    }

    /// A human-readable representation, e.g. `[rootNode] > [programNode] > [accountNode]`.
    pub fn to_debug_string(&self) -> String {
        self.0
            .iter()
            .map(|node| format!("[{}]", node.kind()))
            .collect::<Vec<_>>()
            .join(" > ")
    }
}

impl From<Vec<Node>> for NodePath {
    fn from(nodes: Vec<Node>) -> Self {
        Self(nodes)
    }
}

impl Deref for NodePath {
    type Target = [Node];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_nodes::{InstructionNode, ProgramNode, RootNode};

    fn sample_path() -> NodePath {
        let instruction = InstructionNode {
            name: "myInstruction".into(),
            ..Default::default()
        };
        let program = ProgramNode {
            name: "myProgram".into(),
            ..Default::default()
        };
        let root = RootNode::new(ProgramNode::default());
        NodePath::new()
            .extended(root)
            .extended(program)
            .extended(instruction)
    }

    #[test]
    fn last() {
        let path = sample_path();
        assert_eq!(path.last().unwrap().kind(), "instructionNode");
    }

    #[test]
    fn find_first_and_last() {
        let path = sample_path();
        assert_eq!(
            path.find_first("programNode").unwrap().kind(),
            "programNode"
        );
        assert_eq!(path.find_last("rootNode").unwrap().kind(), "rootNode");
        assert!(path.find_first("accountNode").is_none());
    }

    #[test]
    fn find_program() {
        let path = sample_path();
        assert_eq!(path.find_program().unwrap().name.as_str(), "myProgram");
    }

    #[test]
    fn until_last_node() {
        let path = sample_path();
        let trimmed = path.until_last_node("programNode").unwrap();
        assert_eq!(trimmed.last().unwrap().kind(), "programNode");
        assert_eq!(trimmed.len(), 2);
        assert!(path.until_last_node("accountNode").is_none());
    }

    #[test]
    fn to_debug_string() {
        let path = sample_path();
        assert_eq!(
            path.to_debug_string(),
            "[rootNode] > [programNode] > [instructionNode]"
        );
    }
}
