use crate::NodePath;
use crate::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use codama_nodes::{
    CamelCaseString, HasKind, InstructionNode, LinkNode, Node, ProgramNode, RootNode,
};

/// Resolves [`LinkNode`]s back to the path of their target node.
///
/// Mirrors the TypeScript `LinkableDictionary`. A path owns its nodes, so a
/// path to anything holds a copy of the whole root. Recording one for every
/// linkable node up front copies a large IDL hundreds of times; this keeps
/// the tree once and builds a path only for the link that is asked for.
#[derive(Debug, Clone, Default)]
pub struct LinkableDictionary {
    root: Option<RootNode>,
}

impl LinkableDictionary {
    pub fn new() -> Self {
        Self::default()
    }

    /// A dictionary of every linkable node reachable from `root`.
    pub fn record_all(root: &RootNode) -> Self {
        Self {
            root: Some(root.clone()),
        }
    }

    /// Resolves a path ending in a [`LinkNode`] to the path of its target node.
    ///
    /// `link_path` is the full path to the link node (its ancestors provide the
    /// program / instruction context when the link node does not specify one).
    pub fn get_path(&self, link_path: &NodePath) -> Option<NodePath> {
        let link = match link_path.last()? {
            Node::Link(link) => link,
            _ => return None,
        };
        let root = self.root.as_ref()?;
        let program = self.program(root, link, link_path)?;
        let instruction = self.instruction(program, link, link_path);
        let in_program = |target: Option<Node>| {
            target.map(|node| NodePath(vec![root.clone().into(), program.clone().into(), node]))
        };
        let in_instruction = |instruction: &InstructionNode, target: Option<Node>| {
            target.map(|node| {
                NodePath(vec![
                    root.clone().into(),
                    program.clone().into(),
                    instruction.clone().into(),
                    node,
                ])
            })
        };

        // Where two nodes share a name the later one is the one linked to.
        match link {
            LinkNode::Account(n) => in_program(
                program
                    .accounts
                    .iter()
                    .rfind(|account| account.name == n.name)
                    .map(|account| account.clone().into()),
            ),
            LinkNode::DefinedType(n) => in_program(
                program
                    .defined_types
                    .iter()
                    .rfind(|defined| defined.name == n.name)
                    .map(|defined| defined.clone().into()),
            ),
            LinkNode::Pda(n) => in_program(
                program
                    .pdas
                    .iter()
                    .rfind(|pda| pda.name == n.name)
                    .map(|pda| pda.clone().into()),
            ),
            LinkNode::InstructionAccount(n) => {
                let instruction = instruction?;
                in_instruction(
                    instruction,
                    instruction
                        .accounts
                        .iter()
                        .rfind(|account| account.name == n.name)
                        .map(|account| account.clone().into()),
                )
            }
            LinkNode::InstructionArgument(n) => {
                let instruction = instruction?;
                in_instruction(
                    instruction,
                    instruction
                        .arguments
                        .iter()
                        .rfind(|argument| argument.name == n.name)
                        .map(|argument| argument.clone().into()),
                )
            }
            LinkNode::Instruction(_) => in_program(Some(instruction?.clone().into())),
            LinkNode::Program(_) => {
                Some(NodePath(vec![root.clone().into(), program.clone().into()]))
            }
        }
    }

    /// Like [`get_path`](Self::get_path) but errors if the link cannot be resolved.
    pub fn get_path_or_err(&self, link_path: &NodePath) -> CodamaResult<NodePath> {
        self.get_path(link_path).ok_or_else(|| {
            let (kind, name) = match link_path.last() {
                Some(node @ Node::Link(link)) => (node.kind().to_string(), link_name(link)),
                Some(node) => (node.kind().to_string(), String::new()),
                None => ("None".to_string(), String::new()),
            };
            CodamaError::LinkedNodeNotFound { kind, name }
        })
    }

    /// Resolves a link path to its target node (a clone).
    pub fn get(&self, link_path: &NodePath) -> Option<Node> {
        self.get_path(link_path)
            .and_then(|path| path.last().cloned())
    }

    /// Whether the given link path resolves to a recorded node.
    pub fn has(&self, link_path: &NodePath) -> bool {
        self.get_path(link_path).is_some()
    }

    fn program<'a>(
        &self,
        root: &'a RootNode,
        link: &LinkNode,
        link_path: &NodePath,
    ) -> Option<&'a ProgramNode> {
        let name = link_program_name(link)
            .or_else(|| link_path.find_program().map(|program| program.name.clone()))?;
        root.additional_programs
            .iter()
            .rfind(|program| program.name == name)
            .or_else(|| Some(&root.program).filter(|program| program.name == name))
    }

    fn instruction<'a>(
        &self,
        program: &'a ProgramNode,
        link: &LinkNode,
        link_path: &NodePath,
    ) -> Option<&'a InstructionNode> {
        let name = link_instruction_name(link).or_else(|| {
            link_path
                .find_instruction()
                .map(|instruction| instruction.name.clone())
        })?;
        program
            .instructions
            .iter()
            .rfind(|instruction| instruction.name == name)
    }
}

/// The explicit program name carried by a link node, if any.
fn link_program_name(link: &LinkNode) -> Option<CamelCaseString> {
    match link {
        LinkNode::Program(n) => Some(n.name.clone()),
        LinkNode::Account(n) => n.program.as_ref().map(|p| p.name.clone()),
        LinkNode::DefinedType(n) => n.program.as_ref().map(|p| p.name.clone()),
        LinkNode::Pda(n) => n.program.as_ref().map(|p| p.name.clone()),
        LinkNode::Instruction(n) => n.program.as_ref().map(|p| p.name.clone()),
        LinkNode::InstructionAccount(n) => n
            .instruction
            .as_ref()
            .and_then(|i| i.program.as_ref())
            .map(|p| p.name.clone()),
        LinkNode::InstructionArgument(n) => n
            .instruction
            .as_ref()
            .and_then(|i| i.program.as_ref())
            .map(|p| p.name.clone()),
    }
}

/// The explicit instruction name carried by a link node, if any.
fn link_instruction_name(link: &LinkNode) -> Option<CamelCaseString> {
    match link {
        LinkNode::Instruction(n) => Some(n.name.clone()),
        LinkNode::InstructionAccount(n) => n.instruction.as_ref().map(|i| i.name.clone()),
        LinkNode::InstructionArgument(n) => n.instruction.as_ref().map(|i| i.name.clone()),
        _ => None,
    }
}

fn link_name(link: &LinkNode) -> String {
    use codama_nodes::HasName;
    link.name().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_nodes::{
        AccountLinkNode, AccountNode, DefinedTypeLinkNode, DefinedTypeNode,
        InstructionArgumentLinkNode, InstructionArgumentNode, InstructionLinkNode, InstructionNode,
        NumberTypeNode, PdaLinkNode, PdaNode, ProgramLinkNode, ProgramNode, RootNode,
        StructTypeNode, U32,
    };

    fn root() -> RootNode {
        let program = ProgramNode {
            name: "myProgram".into(),
            accounts: vec![AccountNode::new("myAccount", StructTypeNode::new(vec![]))],
            defined_types: vec![DefinedTypeNode::new("myType", NumberTypeNode::le(U32))],
            pdas: vec![PdaNode::new("myPda", vec![])],
            instructions: vec![InstructionNode {
                name: "myIx".into(),
                arguments: vec![InstructionArgumentNode::new(
                    "myArg",
                    NumberTypeNode::le(U32),
                )],
                ..Default::default()
            }],
            ..Default::default()
        };
        RootNode::new(program)
    }

    fn link_path(root: &RootNode, link: impl Into<Node>) -> NodePath {
        NodePath::new()
            .extended(root.clone())
            .extended(root.program.clone())
            .extended(link)
    }

    #[test]
    fn resolves_account_link() {
        let root = root();
        let dict = LinkableDictionary::record_all(&root);
        let path = link_path(&root, AccountLinkNode::new("myAccount"));
        let resolved = dict.get_path(&path).unwrap();
        assert_eq!(resolved.last().unwrap().kind(), "accountNode");
    }

    #[test]
    fn resolves_defined_type_and_pda_and_program_links() {
        let root = root();
        let dict = LinkableDictionary::record_all(&root);
        assert_eq!(
            dict.get(&link_path(&root, DefinedTypeLinkNode::new("myType")))
                .unwrap()
                .kind(),
            "definedTypeNode"
        );
        assert_eq!(
            dict.get(&link_path(&root, PdaLinkNode::new("myPda")))
                .unwrap()
                .kind(),
            "pdaNode"
        );
        assert_eq!(
            dict.get(&link_path(&root, ProgramLinkNode::new("myProgram")))
                .unwrap()
                .kind(),
            "programNode"
        );
    }

    #[test]
    fn resolves_instruction_scoped_link_via_path_context() {
        let root = root();
        let dict = LinkableDictionary::record_all(&root);
        // An instruction-argument link nested inside its instruction's path.
        let path = NodePath::new()
            .extended(root.clone())
            .extended(root.program.clone())
            .extended(root.program.instructions[0].clone())
            .extended(InstructionArgumentLinkNode::new("myArg"));
        let resolved = dict.get_path(&path).unwrap();
        assert_eq!(resolved.last().unwrap().kind(), "instructionArgumentNode");
    }

    #[test]
    fn resolves_instruction_link_via_explicit_program() {
        let root = root();
        let dict = LinkableDictionary::record_all(&root);
        let link = InstructionLinkNode::new_from_program("myIx", ProgramLinkNode::new("myProgram"));
        // No instruction in the ancestor path; relies on the link's own context.
        let path = NodePath::new().extended(root.clone()).extended(link);
        let resolved = dict.get_path(&path).unwrap();
        assert_eq!(resolved.last().unwrap().kind(), "instructionNode");
    }

    #[test]
    fn missing_link_errors() {
        let root = root();
        let dict = LinkableDictionary::record_all(&root);
        let path = link_path(&root, AccountLinkNode::new("doesNotExist"));
        assert!(dict.get_path(&path).is_none());
        assert!(matches!(
            dict.get_path_or_err(&path),
            Err(CodamaError::LinkedNodeNotFound { .. })
        ));
    }
}
