//! The instruction builder facade.

use crate::accounts::account_metas_from_addresses;
use crate::arguments::{
    encode_instruction_arguments, is_omitted, resolve_argument_defaults, ArgumentResolverFn,
};
use codama_dynamic_codecs::Value;
use codama_dynamic_resolution::{AccountsInput, AddressResolver, ArgumentsInput, ResolverFn};
use codama_nodes::{InstructionNode, RootNode};
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;

/// Builds a [`solana_instruction::Instruction`] for a Codama
/// [`InstructionNode`], with no code generation.
///
/// Rust counterpart of the TypeScript `createInstructionsBuilder`. Configure
/// the builder with account addresses, argument values, signers and custom
/// resolvers, then call [`build`](Self::build).
///
/// TypeScript exposes a single `resolvers` map whose functions return either an
/// address (for account defaults) or a value (for argument defaults). Rust
/// separates the two by return type: [`address_resolver`](Self::address_resolver)
/// (`-> Pubkey`) and [`argument_resolver`](Self::argument_resolver) (`-> Value`).
///
/// # Example
/// ```no_run
/// # use codama_dynamic_instructions::InstructionBuilder;
/// # use codama_nodes::{RootNode, InstructionNode};
/// # fn demo(root: &RootNode, ix: &InstructionNode, payer: solana_pubkey::Pubkey) {
/// let instruction = InstructionBuilder::new(root, ix)
///     .account("payer", payer)
///     .argument("amount", 1_000_000_000u64)
///     .build()
///     .unwrap();
/// # }
/// ```
pub struct InstructionBuilder<'a> {
    root: &'a RootNode,
    instruction: &'a InstructionNode,
    accounts: AccountsInput,
    arguments: ArgumentsInput,
    injected: std::collections::HashMap<String, Value>,
    signers: Vec<String>,
    argument_resolvers: HashMap<String, ArgumentResolverFn>,
    address_resolvers: HashMap<String, ResolverFn>,
}

impl<'a> InstructionBuilder<'a> {
    pub fn new(root: &'a RootNode, instruction: &'a InstructionNode) -> Self {
        Self {
            root,
            instruction,
            accounts: AccountsInput::new(),
            arguments: ArgumentsInput::new(),
            injected: std::collections::HashMap::new(),
            signers: Vec::new(),
            argument_resolvers: HashMap::new(),
            address_resolvers: HashMap::new(),
        }
    }

    /// Registers a user-supplied address for an instruction account.
    pub fn account(mut self, name: impl Into<String>, address: Pubkey) -> Self {
        self.accounts.insert(name.into(), address);
        self
    }

    /// Supplies a value for an injected-value node (`injectedValueNode`) by key.
    pub fn inject(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.injected.insert(key.into(), value.into());
        self
    }

    /// Registers a user-supplied instruction argument value.
    pub fn argument(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.arguments.insert(name.into(), value.into());
        self
    }

    /// Marks an account (with ambiguous `either` signer status) as a signer.
    pub fn signer(mut self, name: impl Into<String>) -> Self {
        self.signers.push(name.into());
        self
    }

    /// Marks several accounts as signers.
    pub fn signers(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.signers.extend(names.into_iter().map(Into::into));
        self
    }

    /// Registers a custom resolver for an account default (`ResolverValueNode`).
    pub fn address_resolver(mut self, name: impl Into<String>, f: ResolverFn) -> Self {
        self.address_resolvers.insert(name.into(), f);
        self
    }

    /// Registers a custom resolver for an argument default (`ResolverValueNode`).
    pub fn argument_resolver(mut self, name: impl Into<String>, f: ArgumentResolverFn) -> Self {
        self.argument_resolvers.insert(name.into(), f);
        self
    }

    /// Assembles the instruction: validates omitted arguments, resolves custom
    /// argument defaults, encodes the argument data, then resolves every
    /// account address and role.
    pub fn build(self) -> CodamaResult<Instruction> {
        let program_id = Pubkey::from_str(&self.root.program.public_key)
            .map_err(|e| CodamaError::UnexpectedAddressType(e.to_string()))?;

        self.validate_omitted_arguments()?;

        let arguments = resolve_argument_defaults(
            self.instruction,
            &self.arguments,
            &self.accounts,
            &self.argument_resolvers,
        )?;

        // Accounts are resolved before argument bytes so omitted `accountBump`
        // arguments can read the bump produced by PDA derivation.
        let mut resolver = AddressResolver::new(self.root, self.instruction);
        for (name, pubkey) in &self.accounts {
            resolver = resolver.account(name.clone(), *pubkey);
        }
        for (name, value) in &arguments {
            resolver = resolver.argument(name.clone(), value.clone());
        }
        for (name, value) in &self.injected {
            resolver = resolver.inject(name.clone(), value.clone());
        }
        for (name, f) in self.address_resolvers {
            resolver = resolver.resolver(name, f);
        }
        let addresses = resolver.resolve_accounts()?;
        let bumps = resolver.bumps();

        let data = encode_instruction_arguments(
            self.root,
            self.instruction,
            &arguments,
            &bumps,
            &self.injected,
        )?;

        let accounts = account_metas_from_addresses(
            self.root,
            self.instruction,
            &addresses,
            &arguments,
            &self.accounts,
            &self.signers,
        )?;

        Ok(Instruction {
            program_id,
            accounts,
            data,
        })
    }

    /// Ensures arguments declared with the `omitted` strategy (e.g. a
    /// discriminator) are not supplied by the caller.
    fn validate_omitted_arguments(&self) -> CodamaResult<()> {
        for arg in &self.instruction.arguments {
            if is_omitted(arg) && self.arguments.contains_key(arg.name.as_str()) {
                return Err(CodamaError::InvalidArgumentInput {
                    argument: arg.name.to_string(),
                    expected: "omitted argument must not be provided".to_string(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AccountMeta;
    use codama_nodes::{
        AccountBumpValueNode, AccountValueNode, ArgumentValueNode, DefaultValueStrategy,
        InstructionAccountNode, InstructionArgumentNode, InstructionNode,
        InstructionRemainingAccountsNode, IsSigner, NumberTypeNode, NumberValueNode,
        OptionTypeNode, OptionalAccountStrategy, PdaNode, PdaSeedValueNode, PdaValueNode,
        PdaValuePda, ProgramNode, PublicKeyTypeNode, ResolverValueNode, VariablePdaSeedNode, U64,
        U8,
    };

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn root_with(program: &Pubkey) -> RootNode {
        RootNode::new(ProgramNode::new("prog", program.to_string()))
    }

    #[test]
    fn builds_basic_instruction_with_discriminator_and_roles() {
        let program = pk(1);
        let root = root_with(&program);

        let mut discriminator =
            InstructionArgumentNode::new("discriminator", NumberTypeNode::le(U8));
        discriminator.default_value_strategy = Some(DefaultValueStrategy::Omitted);
        discriminator.default_value = Box::new(Some(NumberValueNode::new(5u8).into()));

        let ix = InstructionNode {
            name: "transfer".into(),
            accounts: vec![
                InstructionAccountNode::new("source", true, true),
                InstructionAccountNode::new("destination", true, false),
            ],
            arguments: vec![
                discriminator,
                InstructionArgumentNode::new("amount", NumberTypeNode::le(U64)),
            ],
            ..Default::default()
        };

        let source = pk(2);
        let destination = pk(3);
        let instruction = InstructionBuilder::new(&root, &ix)
            .account("source", source)
            .account("destination", destination)
            .argument("amount", 1000u64)
            .build()
            .unwrap();

        assert_eq!(instruction.program_id, program);
        let mut expected_data = vec![5u8];
        expected_data.extend_from_slice(&1000u64.to_le_bytes());
        assert_eq!(instruction.data, expected_data);
        assert_eq!(
            instruction.accounts,
            vec![
                AccountMeta {
                    pubkey: source,
                    is_signer: true,
                    is_writable: true,
                },
                AccountMeta {
                    pubkey: destination,
                    is_signer: false,
                    is_writable: true,
                },
            ]
        );
    }

    #[test]
    fn resolves_pda_account_default() {
        let program = pk(9);
        let root = root_with(&program);

        let pda_node = PdaNode::new(
            "authPda",
            vec![VariablePdaSeedNode::new("authority", PublicKeyTypeNode::new()).into()],
        );
        let pda_value = PdaValueNode::new(
            PdaValuePda::Pda(pda_node),
            vec![PdaSeedValueNode::new(
                "authority",
                AccountValueNode::new("authority"),
            )],
        );
        let mut pda_account = InstructionAccountNode::new("pda", false, false);
        pda_account.default_value = Box::new(Some(pda_value.into()));

        let ix = InstructionNode {
            name: "ix".into(),
            accounts: vec![
                InstructionAccountNode::new("authority", false, true),
                pda_account,
            ],
            ..Default::default()
        };

        let authority = pk(7);
        let instruction = InstructionBuilder::new(&root, &ix)
            .account("authority", authority)
            .build()
            .unwrap();

        let (expected, _) = Pubkey::find_program_address(&[authority.as_ref()], &program);
        assert_eq!(instruction.accounts[1].pubkey, expected);
    }

    #[test]
    fn omitted_bump_argument_uses_derived_pda_bump() {
        let program = pk(9);
        let root = root_with(&program);

        let pda_node = PdaNode::new(
            "authPda",
            vec![VariablePdaSeedNode::new("authority", PublicKeyTypeNode::new()).into()],
        );
        let pda_value = PdaValueNode::new(
            PdaValuePda::Pda(pda_node),
            vec![PdaSeedValueNode::new(
                "authority",
                AccountValueNode::new("authority"),
            )],
        );
        let mut pda_account = InstructionAccountNode::new("pda", false, false);
        pda_account.default_value = Box::new(Some(pda_value.into()));

        let mut bump = InstructionArgumentNode::new("bump", NumberTypeNode::le(U8));
        bump.default_value_strategy = Some(DefaultValueStrategy::Omitted);
        bump.default_value = Box::new(Some(AccountBumpValueNode::new("pda").into()));

        let ix = InstructionNode {
            name: "ix".into(),
            accounts: vec![
                InstructionAccountNode::new("authority", false, true),
                pda_account,
            ],
            arguments: vec![bump],
            ..Default::default()
        };

        let authority = pk(7);
        let instruction = InstructionBuilder::new(&root, &ix)
            .account("authority", authority)
            .build()
            .unwrap();

        let (_, expected_bump) = Pubkey::find_program_address(&[authority.as_ref()], &program);
        assert_eq!(instruction.data, vec![expected_bump]);
    }

    #[test]
    fn optional_account_omitted_is_skipped() {
        let program = pk(5);
        let root = root_with(&program);
        let mut optional = InstructionAccountNode::new("maybe", false, false);
        optional.is_optional = Some(true);

        let ix = InstructionNode {
            name: "ix".into(),
            optional_account_strategy: Some(OptionalAccountStrategy::Omitted),
            accounts: vec![InstructionAccountNode::new("payer", true, true), optional],
            ..Default::default()
        };

        let payer = pk(2);
        let instruction = InstructionBuilder::new(&root, &ix)
            .account("payer", payer)
            .build()
            .unwrap();
        assert_eq!(instruction.accounts.len(), 1);
        assert_eq!(instruction.accounts[0].pubkey, payer);
    }

    #[test]
    fn optional_program_id_account_downgraded_to_readonly() {
        let program = pk(5);
        let root = root_with(&program);
        let mut optional = InstructionAccountNode::new("buffer", true, false); // writable
        optional.is_optional = Some(true);

        let ix = InstructionNode {
            name: "ix".into(),
            optional_account_strategy: Some(OptionalAccountStrategy::ProgramId),
            accounts: vec![optional],
            ..Default::default()
        };

        let instruction = InstructionBuilder::new(&root, &ix).build().unwrap();
        assert_eq!(instruction.accounts.len(), 1);
        // Resolved to the program address and downgraded to read-only.
        assert_eq!(
            instruction.accounts[0],
            AccountMeta {
                pubkey: program,
                is_signer: false,
                is_writable: false,
            }
        );
    }

    #[test]
    fn remaining_accounts_from_argument() {
        let program = pk(1);
        let root = root_with(&program);
        let remaining = InstructionRemainingAccountsNode {
            is_optional: Some(false),
            is_signer: Some(IsSigner::False),
            is_writable: Some(true),
            docs: vec![].into(),
            value: Box::new(ArgumentValueNode::new("extra").into()),
            display: None,
        };
        let ix = InstructionNode {
            name: "ix".into(),
            remaining_accounts: vec![remaining],
            ..Default::default()
        };

        let a = pk(2);
        let b = pk(3);
        let instruction = InstructionBuilder::new(&root, &ix)
            .argument(
                "extra",
                Value::Array(vec![
                    Value::PublicKey(a.to_bytes()),
                    Value::PublicKey(b.to_bytes()),
                ]),
            )
            .build()
            .unwrap();

        assert_eq!(
            instruction.accounts,
            vec![
                AccountMeta {
                    pubkey: a,
                    is_signer: false,
                    is_writable: true,
                },
                AccountMeta {
                    pubkey: b,
                    is_signer: false,
                    is_writable: true,
                },
            ]
        );
    }

    #[test]
    fn optional_argument_encodes_as_none() {
        let program = pk(1);
        let root = root_with(&program);
        let ix = InstructionNode {
            name: "ix".into(),
            arguments: vec![InstructionArgumentNode::new(
                "maybe",
                OptionTypeNode::new(NumberTypeNode::le(U8)),
            )],
            ..Default::default()
        };

        let instruction = InstructionBuilder::new(&root, &ix).build().unwrap();
        // Option prefix `0` = none.
        assert_eq!(instruction.data, vec![0u8]);
    }

    #[test]
    fn custom_argument_resolver_fills_default() {
        let program = pk(1);
        let root = root_with(&program);
        let mut arg = InstructionArgumentNode::new("amount", NumberTypeNode::le(U8));
        arg.default_value = Box::new(Some(ResolverValueNode::new("getAmount").into()));
        let ix = InstructionNode {
            name: "ix".into(),
            arguments: vec![arg],
            ..Default::default()
        };

        let instruction = InstructionBuilder::new(&root, &ix)
            .argument_resolver("getAmount", Box::new(|_, _| Ok(Value::from(7u8))))
            .build()
            .unwrap();
        assert_eq!(instruction.data, vec![7u8]);
    }

    #[test]
    fn providing_omitted_argument_errors() {
        let program = pk(1);
        let root = root_with(&program);
        let mut discriminator =
            InstructionArgumentNode::new("discriminator", NumberTypeNode::le(U8));
        discriminator.default_value_strategy = Some(DefaultValueStrategy::Omitted);
        discriminator.default_value = Box::new(Some(NumberValueNode::new(5u8).into()));
        let ix = InstructionNode {
            name: "ix".into(),
            arguments: vec![discriminator],
            ..Default::default()
        };

        let err = InstructionBuilder::new(&root, &ix)
            .argument("discriminator", 9u8)
            .build()
            .unwrap_err();
        assert!(matches!(err, CodamaError::InvalidArgumentInput { .. }));
    }

    #[test]
    fn missing_required_account_errors() {
        let program = pk(1);
        let root = root_with(&program);
        let ix = InstructionNode {
            name: "ix".into(),
            accounts: vec![InstructionAccountNode::new("authority", false, true)],
            ..Default::default()
        };
        let err = InstructionBuilder::new(&root, &ix).build().unwrap_err();
        assert!(matches!(err, CodamaError::AccountMissing(_)));
    }

    #[test]
    fn missing_required_argument_errors() {
        let program = pk(1);
        let root = root_with(&program);
        let ix = InstructionNode {
            name: "ix".into(),
            arguments: vec![InstructionArgumentNode::new(
                "amount",
                NumberTypeNode::le(U64),
            )],
            ..Default::default()
        };
        let err = InstructionBuilder::new(&root, &ix).build().unwrap_err();
        assert!(matches!(err, CodamaError::ArgumentMissing(_)));
    }
}
