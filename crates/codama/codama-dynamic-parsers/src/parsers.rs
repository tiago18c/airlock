//! Parsing identified data into runtime [`Value`]s.
//!
//! Rust counterpart of the TypeScript `dynamic-parsers/parsers`.

use crate::identify::{identify_data, ParsableKind};
use codama_dynamic_codecs::{get_node_codec, Value};
use codama_nodes::{Node, RootNode};
use codama_visitors_core::DynamicResult as CodamaResult;
use codama_visitors_core::NodePath;
use solana_instruction::{AccountMeta, Instruction};

/// The decoded data and the [`NodePath`] of the node it was decoded against.
#[derive(Debug, Clone)]
pub struct ParsedData {
    pub data: Value,
    pub path: NodePath,
}

/// Parses `bytes` as an account's data.
pub fn parse_account_data(root: &RootNode, bytes: &[u8]) -> CodamaResult<Option<ParsedData>> {
    parse_data(root, bytes, &[ParsableKind::Account])
}

/// Parses `bytes` as an event's data.
pub fn parse_event_data(root: &RootNode, bytes: &[u8]) -> CodamaResult<Option<ParsedData>> {
    parse_data(root, bytes, &[ParsableKind::Event])
}

/// Parses `bytes` as an instruction's data.
pub fn parse_instruction_data(root: &RootNode, bytes: &[u8]) -> CodamaResult<Option<ParsedData>> {
    parse_data(root, bytes, &[ParsableKind::Instruction])
}

/// Identifies the node matching `bytes` (among the requested `kinds`) and
/// decodes the bytes against it.
pub fn parse_data(
    root: &RootNode,
    bytes: &[u8],
    kinds: &[ParsableKind],
) -> CodamaResult<Option<ParsedData>> {
    let Some(path) = identify_data(root, bytes, kinds)? else {
        return Ok(None);
    };
    let codec = get_node_codec(&path)?;
    let mut input = bytes;
    let data = codec.decode(&mut input)?;
    Ok(Some(ParsedData { data, path }))
}

/// Decodes `bytes` against a specific account type by name, bypassing
/// discriminator identification. Useful when the caller already knows the
/// account type through out-of-band means (e.g. token-2022's account-type
/// byte, which size discriminators cannot express). Returns `None` when no
/// account with that name exists.
pub fn parse_account_data_as(
    root: &RootNode,
    name: &str,
    bytes: &[u8],
) -> CodamaResult<Option<ParsedData>> {
    let name = codama_nodes::CamelCaseString::new(name);
    let Some(account) = root.program.accounts.iter().find(|a| a.name == name) else {
        return Ok(None);
    };
    let path = NodePath::from(vec![
        Node::Root(root.clone()),
        Node::Program(root.program.clone()),
        Node::Account(account.clone()),
    ]);
    let codec = get_node_codec(&path)?;
    let mut input = bytes;
    let data = codec.decode(&mut input)?;
    Ok(Some(ParsedData { data, path }))
}

/// An instruction account paired with its Codama name.
#[derive(Debug, Clone)]
pub struct ParsedInstructionAccount {
    pub name: String,
    pub meta: AccountMeta,
}

/// A parsed instruction: its decoded argument data, the matched node path, and
/// its accounts paired with their Codama names (by position).
#[derive(Debug, Clone)]
pub struct ParsedInstruction {
    pub data: Value,
    pub path: NodePath,
    pub accounts: Vec<ParsedInstructionAccount>,
    /// Account metas that follow the named accounts. Display grouping uses these
    /// for `remainingAccounts`; they are not part of the named account list.
    pub remaining_accounts: Vec<AccountMeta>,
}

/// Parses a native [`Instruction`]: decodes its data and pairs each account
/// meta with the corresponding account name from the identified instruction
/// node (by position). Returns `None` if the data matches no instruction.
pub fn parse_instruction(
    root: &RootNode,
    instruction: &Instruction,
) -> CodamaResult<Option<ParsedInstruction>> {
    let Some(parsed) = parse_data(root, &instruction.data, &[ParsableKind::Instruction])? else {
        return Ok(None);
    };
    let Some(Node::Instruction(instruction_node)) = parsed.path.last() else {
        return Ok(None);
    };

    let named_len = instruction_node.accounts.len();
    let (named_metas, extra) = if instruction.accounts.len() > named_len {
        (
            &instruction.accounts[..named_len],
            &instruction.accounts[named_len..],
        )
    } else {
        (instruction.accounts.as_slice(), &[][..])
    };
    let accounts = instruction_node
        .accounts
        .iter()
        .zip(named_metas.iter())
        .map(|(account, meta)| ParsedInstructionAccount {
            name: account.name.to_string(),
            meta: (*meta).clone(),
        })
        .collect();

    Ok(Some(ParsedInstruction {
        data: parsed.data,
        path: parsed.path,
        accounts,
        remaining_accounts: extra.to_vec(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_nodes::{
        AccountNode, ConstantDiscriminatorNode, ConstantValueNode, DefaultValueStrategy, EventNode,
        FieldDiscriminatorNode, InstructionAccountNode, InstructionArgumentNode, InstructionNode,
        NumberTypeNode, NumberValueNode, ProgramNode, SizeDiscriminatorNode, StructFieldTypeNode,
        StructTypeNode, U32, U64, U8,
    };
    use solana_pubkey::Pubkey;

    fn root_with_program(program: ProgramNode) -> RootNode {
        RootNode::new(program)
    }

    fn account_struct() -> StructTypeNode {
        StructTypeNode::new(vec![
            StructFieldTypeNode::new("tag", NumberTypeNode::le(U8)),
            StructFieldTypeNode::new("value", NumberTypeNode::le(U32)),
        ])
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

    #[test]
    fn identifies_and_parses_account_by_constant_discriminator() {
        let mut account = AccountNode::new("myAccount", account_struct());
        account.discriminators = vec![ConstantDiscriminatorNode::new(
            ConstantValueNode::new(NumberTypeNode::le(U8), NumberValueNode::new(7u8)),
            0,
        )
        .into()];
        let mut program = ProgramNode::new("prog", Pubkey::new_from_array([1; 32]).to_string());
        program.accounts = vec![account];
        let root = root_with_program(program);

        // bytes = [tag=7, value=1]
        let bytes = [7u8, 1, 0, 0, 0];
        let parsed = parse_account_data(&root, &bytes).unwrap().unwrap();
        assert!(matches!(parsed.path.last(), Some(Node::Account(_))));
        assert_eq!(
            parsed.data,
            Value::struct_from([("tag".into(), 7u8.into()), ("value".into(), 1u32.into())])
        );
    }

    #[test]
    fn account_with_wrong_discriminator_is_not_identified() {
        let mut account = AccountNode::new("myAccount", account_struct());
        account.discriminators = vec![ConstantDiscriminatorNode::new(
            ConstantValueNode::new(NumberTypeNode::le(U8), NumberValueNode::new(7u8)),
            0,
        )
        .into()];
        let mut program = ProgramNode::new("prog", Pubkey::new_from_array([1; 32]).to_string());
        program.accounts = vec![account];
        let root = root_with_program(program);

        let bytes = [9u8, 1, 0, 0, 0]; // tag 9 != 7
        assert!(parse_account_data(&root, &bytes).unwrap().is_none());
    }

    #[test]
    fn identifies_account_by_size_discriminator() {
        let mut account = AccountNode::new(
            "sized",
            StructTypeNode::new(vec![StructFieldTypeNode::new(
                "value",
                NumberTypeNode::le(U32),
            )]),
        );
        account.discriminators = vec![SizeDiscriminatorNode::new(4).into()];
        let mut program = ProgramNode::new("prog", Pubkey::new_from_array([1; 32]).to_string());
        program.accounts = vec![account];
        let root = root_with_program(program);

        let bytes = [1u8, 0, 0, 0];
        let parsed = parse_account_data(&root, &bytes).unwrap().unwrap();
        assert_eq!(
            parsed.data,
            Value::struct_from([("value".into(), 1u32.into())])
        );
        // A buffer of the wrong size is not identified.
        assert!(parse_account_data(&root, &[1u8, 0, 0]).unwrap().is_none());
    }

    #[test]
    fn parses_instruction_data_by_field_discriminator() {
        let mut program = ProgramNode::new("prog", Pubkey::new_from_array([1; 32]).to_string());
        program.instructions = vec![transfer_instruction()];
        let root = root_with_program(program);

        // bytes = [discriminator=5, amount=1000u64 le]
        let mut bytes = vec![5u8];
        bytes.extend_from_slice(&1000u64.to_le_bytes());

        let parsed = parse_instruction_data(&root, &bytes).unwrap().unwrap();
        assert!(matches!(parsed.path.last(), Some(Node::Instruction(_))));
        assert_eq!(
            parsed.data,
            Value::struct_from([
                ("discriminator".into(), 5u8.into()),
                ("amount".into(), 1000u64.into()),
            ])
        );
    }

    #[test]
    fn parse_instruction_pairs_account_names() {
        let mut program = ProgramNode::new("prog", Pubkey::new_from_array([1; 32]).to_string());
        let program_id = Pubkey::new_from_array([1; 32]);
        program.instructions = vec![transfer_instruction()];
        let root = root_with_program(program);

        let mut data = vec![5u8];
        data.extend_from_slice(&1000u64.to_le_bytes());
        let source = Pubkey::new_from_array([2; 32]);
        let destination = Pubkey::new_from_array([3; 32]);
        let instruction = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(source, true),
                AccountMeta::new(destination, false),
            ],
            data,
        };

        let parsed = parse_instruction(&root, &instruction).unwrap().unwrap();
        let names: Vec<&str> = parsed.accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["source", "destination"]);
        assert_eq!(parsed.accounts[0].meta.pubkey, source);
        assert_eq!(
            parsed.data,
            Value::struct_from([
                ("discriminator".into(), 5u8.into()),
                ("amount".into(), 1000u64.into()),
            ])
        );
    }

    #[test]
    fn kind_filter_restricts_search() {
        // An instruction whose field discriminator would match, but we only ask
        // for accounts → no match.
        let mut program = ProgramNode::new("prog", Pubkey::new_from_array([1; 32]).to_string());
        program.instructions = vec![transfer_instruction()];
        let root = root_with_program(program);

        let mut bytes = vec![5u8];
        bytes.extend_from_slice(&1000u64.to_le_bytes());
        assert!(parse_account_data(&root, &bytes).unwrap().is_none());
        assert!(parse_instruction_data(&root, &bytes).unwrap().is_some());
    }

    #[test]
    fn identifies_event_by_constant_discriminator() {
        let mut event = EventNode::new(
            "myEvent",
            StructTypeNode::new(vec![StructFieldTypeNode::new(
                "value",
                NumberTypeNode::le(U32),
            )]),
        );
        event.discriminators = vec![ConstantDiscriminatorNode::new(
            ConstantValueNode::new(NumberTypeNode::le(U8), NumberValueNode::new(3u8)),
            0,
        )
        .into()];
        let mut program = ProgramNode::new("prog", Pubkey::new_from_array([1; 32]).to_string());
        program.events = vec![event];
        let root = root_with_program(program);

        // The constant discriminator (u8=3) sits at offset 0, before the value.
        let bytes = [3u8, 0, 0, 0];
        let parsed = parse_event_data(&root, &bytes).unwrap().unwrap();
        assert!(matches!(parsed.path.last(), Some(Node::Event(_))));
    }
}
