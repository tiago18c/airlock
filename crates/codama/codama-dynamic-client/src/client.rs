//! The program client facade.

use crate::collect_pdas::collect_pda_nodes;
use codama_dynamic_codecs::Value;
use codama_dynamic_instructions::InstructionBuilder;
use codama_dynamic_parsers::{
    account_data_filters, parse_account_data, parse_event_data, parse_instruction,
    parse_instruction_data, DataFilter, Instruction, ParsedData, ParsedInstruction,
};
use codama_dynamic_resolution::{resolve_standalone_pda, Pubkey};
use codama_nodes::{AccountNode, InstructionNode, NestedTypeNodeTrait, PdaNode, RootNode};
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::str::FromStr;

/// A runtime client for a Solana program, built from a Codama IDL with no code
/// generation.
///
/// Rust counterpart of the TypeScript `createProgramClient`. Where TypeScript
/// exposes `methods.<name>(args)` and `pdas.<name>(seeds)` via dynamic proxies,
/// the Rust client exposes explicit [`method`](Self::method) and
/// [`derive_pda`](Self::derive_pda) lookups by name.
///
/// # Example
/// ```no_run
/// # use codama_dynamic_client::ProgramClient;
/// # fn demo(idl_json: &str, payer: solana_pubkey::Pubkey) -> Result<(), Box<dyn std::error::Error>> {
/// let client = ProgramClient::from_json(idl_json)?;
/// let instruction = client
///     .method("transfer")?
///     .account("source", payer)
///     .argument("amount", 1_000_000u64)
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct ProgramClient {
    root: RootNode,
    program_address: Pubkey,
    pdas: IndexMap<String, PdaNode>,
}

impl ProgramClient {
    /// Builds a client from a Codama IDL JSON string.
    pub fn from_json(json: &str) -> CodamaResult<Self> {
        let root: RootNode =
            serde_json::from_str(json).map_err(codama_errors::CodamaError::from)?;
        Self::from_root(root)
    }

    /// Builds a client from an already-parsed [`RootNode`].
    pub fn from_root(root: RootNode) -> CodamaResult<Self> {
        let program_address = Pubkey::from_str(&root.program.public_key)
            .map_err(|e| CodamaError::UnexpectedAddressType(e.to_string()))?;
        let pdas = collect_pda_nodes(&root);
        Ok(Self {
            root,
            program_address,
            pdas,
        })
    }

    /// Creates a client from a root node with an explicitly known program id.
    /// Unlike [`Self::from_root`], this works when the IDL declares no program
    /// address (common for converted legacy Anchor IDLs without
    /// `metadata.address`).
    pub fn from_root_with_program_id(mut root: RootNode, program_id: Pubkey) -> CodamaResult<Self> {
        root.program.public_key = program_id.to_string();
        Self::from_root(root)
    }

    /// Overrides the program id, replacing the one declared in the IDL.
    pub fn with_program_id(mut self, program_id: Pubkey) -> Self {
        self.root.program.public_key = program_id.to_string();
        self.program_address = program_id;
        self
    }

    /// The parsed Codama root node, for advanced use-cases.
    pub fn root(&self) -> &RootNode {
        &self.root
    }

    /// The program id.
    pub fn program_address(&self) -> Pubkey {
        self.program_address
    }

    /// The names of every instruction in the program, in IDL order.
    pub fn instruction_names(&self) -> Vec<&str> {
        self.root
            .program
            .instructions
            .iter()
            .map(|ix| ix.name.as_str())
            .collect()
    }

    /// Looks up an instruction node by name.
    pub fn instruction_node(&self, name: &str) -> Option<&InstructionNode> {
        self.root
            .program
            .instructions
            .iter()
            .find(|ix| ix.name.as_str() == name)
    }

    /// Starts building the named instruction, returning an
    /// [`InstructionBuilder`] borrowing this client's root node.
    pub fn method(&self, name: &str) -> CodamaResult<InstructionBuilder<'_>> {
        let instruction =
            self.instruction_node(name)
                .ok_or_else(|| CodamaError::InstructionNotFound {
                    name: name.to_string(),
                    available: self.instruction_names().join(", "),
                })?;
        Ok(InstructionBuilder::new(&self.root, instruction))
    }

    /// The names of every account in the program, in IDL order.
    pub fn account_names(&self) -> Vec<&str> {
        self.root
            .program
            .accounts
            .iter()
            .map(|account| account.name.as_str())
            .collect()
    }

    /// Looks up an account node by name.
    pub fn account_node(&self, name: &str) -> Option<&AccountNode> {
        self.root
            .program
            .accounts
            .iter()
            .find(|account| account.name.as_str() == name)
    }

    /// Computes the byte-level [`DataFilter`]s that identify the named account
    /// type on-chain, derived from its discriminators. Returns an empty vector
    /// when the account has no discriminators (it cannot be narrowed
    /// server-side, and callers must filter results themselves).
    pub fn account_filters(&self, name: &str) -> CodamaResult<Vec<DataFilter>> {
        let account = self
            .account_node(name)
            .ok_or_else(|| CodamaError::AccountNotFound {
                name: name.to_string(),
                available: self.account_names().join(", "),
            })?;
        let struct_type = account.data.get_nested_type_node().clone();
        account_data_filters(&self.root, &account.discriminators, Some(&struct_type))
    }

    /// The names of every PDA reachable from the IDL (registered or inline).
    pub fn pda_names(&self) -> Vec<&str> {
        self.pdas.keys().map(|name| name.as_str()).collect()
    }

    /// Derives a standalone PDA by name from the given variable seed values.
    /// Returns the derived address and its bump.
    pub fn derive_pda(
        &self,
        name: &str,
        seeds: HashMap<String, Value>,
    ) -> CodamaResult<(Pubkey, u8)> {
        let pda = self
            .pdas
            .get(name)
            .ok_or_else(|| CodamaError::PdaNotFound {
                name: name.to_string(),
                available: self.pda_names().join(", "),
            })?;
        resolve_standalone_pda(&self.root, pda, &seeds)
    }

    // ----- Parsing (delegates to codama-dynamic-parsers) -----

    /// Identifies and decodes raw account data.
    pub fn parse_account_data(&self, bytes: &[u8]) -> CodamaResult<Option<ParsedData>> {
        parse_account_data(&self.root, bytes)
    }

    /// Decodes raw account data as a specific account type by name, bypassing
    /// discriminator identification. Returns `None` for an unknown name.
    pub fn parse_account_data_as(
        &self,
        name: &str,
        bytes: &[u8],
    ) -> CodamaResult<Option<ParsedData>> {
        codama_dynamic_parsers::parse_account_data_as(&self.root, name, bytes)
    }

    /// Identifies and decodes raw event data.
    pub fn parse_event_data(&self, bytes: &[u8]) -> CodamaResult<Option<ParsedData>> {
        parse_event_data(&self.root, bytes)
    }

    /// Identifies and decodes raw instruction data.
    pub fn parse_instruction_data(&self, bytes: &[u8]) -> CodamaResult<Option<ParsedData>> {
        parse_instruction_data(&self.root, bytes)
    }

    /// Parses a native [`Instruction`], decoding its data and pairing each
    /// account with its Codama name.
    pub fn parse_instruction(
        &self,
        instruction: &Instruction,
    ) -> CodamaResult<Option<ParsedInstruction>> {
        parse_instruction(&self.root, instruction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_nodes::{
        AccountNode, DefaultValueStrategy, FieldDiscriminatorNode, InstructionAccountNode,
        InstructionArgumentNode, InstructionNode, Node, NumberTypeNode, NumberValueNode, PdaNode,
        ProgramNode, PublicKeyTypeNode, StructFieldTypeNode, StructTypeNode, VariablePdaSeedNode,
        U64, U8,
    };

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn transfer_instruction() -> InstructionNode {
        let mut discriminator =
            InstructionArgumentNode::new("discriminator", NumberTypeNode::le(U8));
        discriminator.default_value_strategy = Some(DefaultValueStrategy::Omitted);
        discriminator.default_value = Box::new(Some(NumberValueNode::new(5u8).into()));
        InstructionNode {
            name: "transfer".into(),
            accounts: vec![
                InstructionAccountNode::new("source", true, true),
                InstructionAccountNode::new("destination", true, false),
            ],
            arguments: vec![
                discriminator,
                InstructionArgumentNode::new("amount", NumberTypeNode::le(U64)),
            ],
            discriminators: vec![FieldDiscriminatorNode::new("discriminator", 0).into()],
            ..Default::default()
        }
    }

    fn counter_account() -> AccountNode {
        let mut discriminator = StructFieldTypeNode::new("discriminator", NumberTypeNode::le(U8));
        discriminator.default_value = Box::new(Some(NumberValueNode::new(42u8).into()));
        let data = StructTypeNode::new(vec![
            discriminator,
            StructFieldTypeNode::new("value", NumberTypeNode::le(U64)),
        ]);
        let mut account = AccountNode::new("counter", data);
        account.discriminators = vec![FieldDiscriminatorNode::new("discriminator", 0).into()];
        account
    }

    fn sample_root(program: &Pubkey) -> RootNode {
        let pda = PdaNode::new(
            "myPda",
            vec![VariablePdaSeedNode::new("authority", PublicKeyTypeNode::new()).into()],
        );
        let program_node = ProgramNode::new("myProgram", program.to_string())
            .add_instruction(transfer_instruction())
            .add_account(counter_account())
            .add_pda(pda);
        RootNode::new(program_node)
    }

    fn expected_transfer_data(amount: u64) -> Vec<u8> {
        let mut data = vec![5u8];
        data.extend_from_slice(&amount.to_le_bytes());
        data
    }

    #[test]
    fn builds_instruction_via_method() {
        let program = pk(1);
        let client = ProgramClient::from_root(sample_root(&program)).unwrap();
        let source = pk(2);
        let destination = pk(3);

        let instruction = client
            .method("transfer")
            .unwrap()
            .account("source", source)
            .account("destination", destination)
            .argument("amount", 1000u64)
            .build()
            .unwrap();

        assert_eq!(instruction.program_id, program);
        assert_eq!(instruction.data, expected_transfer_data(1000));
        assert_eq!(instruction.accounts.len(), 2);
        assert_eq!(instruction.accounts[0].pubkey, source);
        assert!(instruction.accounts[0].is_signer && instruction.accounts[0].is_writable);
        assert_eq!(instruction.accounts[1].pubkey, destination);
        assert!(!instruction.accounts[1].is_signer && instruction.accounts[1].is_writable);
    }

    #[test]
    fn from_json_round_trips() {
        let program = pk(1);
        let json = serde_json::to_string(&sample_root(&program)).unwrap();
        let client = ProgramClient::from_json(&json).unwrap();

        let instruction = client
            .method("transfer")
            .unwrap()
            .account("source", pk(2))
            .account("destination", pk(3))
            .argument("amount", 42u64)
            .build()
            .unwrap();
        assert_eq!(instruction.program_id, program);
        assert_eq!(instruction.data, expected_transfer_data(42));
    }

    #[test]
    fn with_program_id_overrides_address() {
        let client = ProgramClient::from_root(sample_root(&pk(1)))
            .unwrap()
            .with_program_id(pk(9));
        assert_eq!(client.program_address(), pk(9));

        let instruction = client
            .method("transfer")
            .unwrap()
            .account("source", pk(2))
            .account("destination", pk(3))
            .argument("amount", 1u64)
            .build()
            .unwrap();
        assert_eq!(instruction.program_id, pk(9));
    }

    #[test]
    fn derives_pda_by_name() {
        let program = pk(1);
        let client = ProgramClient::from_root(sample_root(&program)).unwrap();
        let authority = pk(7);

        let mut seeds = HashMap::new();
        seeds.insert(
            "authority".to_string(),
            Value::PublicKey(authority.to_bytes()),
        );
        let (address, _bump) = client.derive_pda("myPda", seeds).unwrap();

        let (expected, _) = Pubkey::find_program_address(&[authority.as_ref()], &program);
        assert_eq!(address, expected);
        assert_eq!(client.pda_names(), vec!["myPda"]);
    }

    #[test]
    fn unknown_instruction_and_pda_error() {
        let client = ProgramClient::from_root(sample_root(&pk(1))).unwrap();
        assert!(matches!(
            client.method("nope").err().unwrap(),
            CodamaError::InstructionNotFound { .. }
        ));
        assert!(matches!(
            client.derive_pda("nope", HashMap::new()).err().unwrap(),
            CodamaError::PdaNotFound { .. }
        ));
        assert_eq!(client.instruction_names(), vec!["transfer"]);
    }

    #[test]
    fn account_filters_from_field_discriminator() {
        let client = ProgramClient::from_root(sample_root(&pk(1))).unwrap();
        assert_eq!(client.account_names(), vec!["counter"]);

        // The field discriminator's u8 default (42) becomes a memcmp at offset 0.
        let filters = client.account_filters("counter").unwrap();
        assert_eq!(
            filters,
            vec![DataFilter::Memcmp {
                offset: 0,
                bytes: vec![42],
            }]
        );

        // Unknown account names surface a typed error.
        assert!(matches!(
            client.account_filters("nope").err().unwrap(),
            CodamaError::AccountNotFound { .. }
        ));
    }

    #[test]
    fn build_then_parse_round_trips() {
        let client = ProgramClient::from_root(sample_root(&pk(1))).unwrap();
        let instruction = client
            .method("transfer")
            .unwrap()
            .account("source", pk(2))
            .account("destination", pk(3))
            .argument("amount", 1000u64)
            .build()
            .unwrap();

        // Parse the raw data back.
        let parsed = client
            .parse_instruction_data(&instruction.data)
            .unwrap()
            .unwrap();
        assert!(matches!(parsed.path.last(), Some(Node::Instruction(_))));
        assert_eq!(
            parsed.data,
            Value::struct_from([
                ("discriminator".into(), 5u8.into()),
                ("amount".into(), 1000u64.into()),
            ])
        );

        // Parse the whole instruction, pairing account names.
        let parsed_ix = client.parse_instruction(&instruction).unwrap().unwrap();
        let names: Vec<&str> = parsed_ix.accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["source", "destination"]);
    }
}
