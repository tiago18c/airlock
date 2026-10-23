use crate::NodePath;
use crate::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use codama_nodes::Node;

/// A stack of [`NodePath`]s maintained during a traversal.
///
/// - The last path is the *current* path being built as the traversal walks
///   down the tree (via [`push`](NodeStack::push) / [`pop`](NodeStack::pop)).
/// - Additional paths can be saved and restored with
///   [`push_path`](NodeStack::push_path) / [`pop_path`](NodeStack::pop_path)
///   when the traversal jumps to a different part of the tree (e.g. when
///   following a link node).
///
/// There is always at least one path in the stack.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeStack {
    stack: Vec<Vec<Node>>,
}

impl Default for NodeStack {
    fn default() -> Self {
        Self {
            stack: vec![Vec::new()],
        }
    }
}

impl NodeStack {
    /// Creates an empty stack containing a single empty path.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a stack seeded with the given paths. The last path becomes the
    /// current path; if `paths` is empty a single empty path is used.
    pub fn from_paths(paths: Vec<NodePath>) -> Self {
        if paths.is_empty() {
            return Self::default();
        }
        Self {
            stack: paths.into_iter().map(|p| p.0).collect(),
        }
    }

    fn current_mut(&mut self) -> &mut Vec<Node> {
        self.stack
            .last_mut()
            .expect("NodeStack always has at least one path")
    }

    fn current(&self) -> &Vec<Node> {
        self.stack
            .last()
            .expect("NodeStack always has at least one path")
    }

    /// Pushes a node onto the current path.
    pub fn push(&mut self, node: impl Into<Node>) {
        self.current_mut().push(node.into());
    }

    /// Pops the last node off the current path.
    pub fn pop(&mut self) -> Option<Node> {
        self.current_mut().pop()
    }

    /// The last node of the current path, if any.
    pub fn peek(&self) -> Option<&Node> {
        self.current().last()
    }

    /// Starts a new current path (empty by default).
    pub fn push_path(&mut self, path: NodePath) {
        self.stack.push(path.0);
    }

    /// Restores the previous current path, returning the popped one.
    ///
    /// Errors if only the last path remains, since the stack must never be empty.
    pub fn pop_path(&mut self) -> CodamaResult<NodePath> {
        if self.stack.len() <= 1 {
            return Err(CodamaError::CannotRemoveLastPathInNodeStack);
        }
        Ok(NodePath(self.stack.pop().unwrap()))
    }

    /// A clone of the current path.
    pub fn get_path(&self) -> NodePath {
        NodePath(self.current().clone())
    }

    /// Whether the current path is empty.
    pub fn is_empty(&self) -> bool {
        self.current().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_nodes::{HasKind, ProgramNode, RootNode};

    #[test]
    fn push_pop_peek() {
        let mut stack = NodeStack::new();
        assert!(stack.is_empty());
        stack.push(RootNode::new(ProgramNode::default()));
        stack.push(ProgramNode::default());
        assert_eq!(stack.peek().unwrap().kind(), "programNode");
        assert_eq!(stack.get_path().len(), 2);
        let popped = stack.pop().unwrap();
        assert_eq!(popped.kind(), "programNode");
        assert_eq!(stack.get_path().len(), 1);
    }

    #[test]
    fn push_and_pop_path() {
        let mut stack = NodeStack::new();
        stack.push(RootNode::new(ProgramNode::default()));
        stack.push_path(NodePath::new());
        stack.push(ProgramNode::default());
        assert_eq!(stack.get_path().len(), 1);
        let restored = stack.pop_path().unwrap();
        assert_eq!(restored.len(), 1);
        // Back to the original path.
        assert_eq!(stack.get_path().len(), 1);
        assert_eq!(stack.peek().unwrap().kind(), "rootNode");
    }

    #[test]
    fn cannot_pop_last_path() {
        let mut stack = NodeStack::new();
        assert!(matches!(
            stack.pop_path(),
            Err(CodamaError::CannotRemoveLastPathInNodeStack)
        ));
    }
}
