use crate::inputs::*;
use codama_dynamic_codecs::{get_type_codec, get_value_with, Value};
use codama_nodes::{
    ConditionalValueCondition, ConstantPdaSeedNode, HasKind, InstructionAccountNode,
    InstructionInputValueNode, InstructionNode, OptionalAccountStrategy, PdaNode, PdaSeedNode,
    PdaSeedValueValue, PdaValueNode, PdaValuePda, PdaValueProgramId, RootNode, TypeNode, ValueNode,
    VariablePdaSeedNode,
};
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use solana_pubkey::Pubkey;
use std::str::FromStr;

/// Resolves instruction account addresses (including PDAs) from a Codama node
/// tree, given user-supplied accounts, arguments and custom resolvers.
///
/// Rust counterpart of the TypeScript `@codama/dynamic-address-resolution`
/// package, collapsed to a synchronous API (Solana PDA derivation is sync).
pub struct AddressResolver<'a> {
    root: &'a RootNode,
    instruction: &'a InstructionNode,
    accounts: AccountsInput,
    arguments: ArgumentsInput,
    resolvers: std::collections::HashMap<String, ResolverFn>,
    injected: std::collections::HashMap<String, Value>,
    bumps: std::cell::RefCell<std::collections::HashMap<String, u8>>,
}

impl<'a> AddressResolver<'a> {
    pub fn new(root: &'a RootNode, instruction: &'a InstructionNode) -> Self {
        Self {
            root,
            instruction,
            accounts: AccountsInput::new(),
            arguments: ArgumentsInput::new(),
            resolvers: std::collections::HashMap::new(),
            injected: std::collections::HashMap::new(),
            bumps: std::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// Supplies a value for an [`codama_nodes::InjectedValueNode`] key.
    pub fn inject(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.injected.insert(key.into(), value.into());
        self
    }

    /// PDA bumps recorded while resolving accounts, keyed by account name.
    pub fn bumps(&self) -> std::collections::HashMap<String, u8> {
        self.bumps.borrow().clone()
    }

    /// Resolves every instruction account, in declaration order.
    ///
    /// `None` entries are optional accounts omitted by strategy. PDA bumps
    /// found along the way are available from [`bumps`](Self::bumps).
    pub fn resolve_accounts(&self) -> CodamaResult<Vec<Option<Pubkey>>> {
        self.instruction
            .accounts
            .iter()
            .map(|account| self.resolve_account(account.name.as_str()))
            .collect()
    }

    /// Registers a user-supplied address for an instruction account.
    pub fn account(mut self, name: impl Into<String>, address: Pubkey) -> Self {
        self.accounts.insert(name.into(), address);
        self
    }

    /// Registers a user-supplied instruction argument value.
    pub fn argument(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.arguments.insert(name.into(), value.into());
        self
    }

    /// Registers a custom resolver function by name.
    pub fn resolver(mut self, name: impl Into<String>, f: ResolverFn) -> Self {
        self.resolvers.insert(name.into(), f);
        self
    }

    /// Resolves the address of the named instruction account.
    ///
    /// Returns `Ok(None)` when an optional account resolves to nothing under
    /// the [`OptionalAccountStrategy::Omitted`] strategy.
    pub fn resolve_account(&self, name: &str) -> CodamaResult<Option<Pubkey>> {
        self.resolve_account_by_name(name, &[])
    }

    // ----- internal -----

    /// Resolves an account by name, detecting circular dependencies along the
    /// current resolution `path` (the chain of account names being resolved).
    fn resolve_account_by_name(&self, name: &str, path: &[String]) -> CodamaResult<Option<Pubkey>> {
        if path.iter().any(|n| n == name) {
            let mut chain: Vec<String> = path.to_vec();
            chain.push(name.to_string());
            return Err(CodamaError::CircularAccountDependency {
                chain: chain.join(" -> "),
            });
        }
        // A directly-provided address short-circuits the lookup.
        if let Some(addr) = self.accounts.get(name) {
            return Ok(Some(*addr));
        }
        let account = self
            .find_account_node(name)
            .ok_or_else(|| CodamaError::AccountMissing(name.to_string()))?;
        let mut next_path = path.to_vec();
        next_path.push(name.to_string());
        self.resolve_account_node(account, &next_path)
    }

    fn resolve_account_node(
        &self,
        account: &InstructionAccountNode,
        path: &[String],
    ) -> CodamaResult<Option<Pubkey>> {
        if let Some(addr) = self.accounts.get(account.name.as_str()) {
            return Ok(Some(*addr));
        }
        if let Some(default) = account.default_value.as_ref().as_ref() {
            match self.resolve_input_value(default, account, path)? {
                Some(addr) => Ok(Some(addr)),
                None => self.apply_optional(account),
            }
        } else {
            self.apply_optional(account)
        }
    }

    /// Applies the instruction's optional-account strategy, or errors if the
    /// account is required and was not resolved.
    fn apply_optional(&self, account: &InstructionAccountNode) -> CodamaResult<Option<Pubkey>> {
        if account.is_optional.unwrap_or(false) {
            match self.instruction.optional_account_strategy {
                Some(OptionalAccountStrategy::ProgramId) => Ok(Some(self.program_id()?)),
                Some(OptionalAccountStrategy::Omitted) | None => Ok(None),
            }
        } else {
            Err(CodamaError::AccountMissing(account.name.to_string()))
        }
    }

    /// Resolves an [`InstructionInputValueNode`] used as an account default to
    /// an address. Returns `Ok(None)` only for the explicit none/omitted paths.
    fn resolve_input_value(
        &self,
        value: &InstructionInputValueNode,
        account: &InstructionAccountNode,
        path: &[String],
    ) -> CodamaResult<Option<Pubkey>> {
        use InstructionInputValueNode as I;
        match value {
            I::AccountValue(n) => self.resolve_account_by_name(&n.name, path),
            I::ArgumentValue(n) => {
                let v = self
                    .arguments
                    .get(n.name.as_str())
                    .ok_or_else(|| CodamaError::ArgumentMissing(n.name.to_string()))?;
                Ok(Some(value_to_pubkey(v)?))
            }
            I::PdaValue(p) => {
                let (address, bump) = self.resolve_pda_value(p, path)?;
                if let Some(name) = path.last() {
                    self.bumps.borrow_mut().insert(name.clone(), bump);
                }
                Ok(Some(address))
            }
            I::InjectedValue(n) => {
                if let Some(value) = self.injected.get(n.key.as_str()) {
                    return Ok(Some(value_to_pubkey(value)?));
                }
                match n.fallback.as_ref().as_ref() {
                    Some(fallback) => Ok(Some(value_to_pubkey(&get_value_with(
                        fallback,
                        &self.injected,
                    )?)?)),
                    None => Err(CodamaError::InjectedValueMissing(n.key.to_string())),
                }
            }
            I::AccountFieldValue(n) => Err(CodamaError::UnexpectedAddressType(format!(
                "account field `{}` requires fetched account data",
                n.account.as_str()
            ))),
            I::AccountBumpValue(n) => Err(CodamaError::UnexpectedAddressType(format!(
                "account bump `{}` is not an address",
                n.name.as_str()
            ))),
            I::ProgramIdValue(_) | I::ProgramLink(_) => Ok(Some(self.program_id()?)),
            I::PublicKeyValue(n) => Pubkey::from_str(&n.public_key)
                .map(Some)
                .map_err(|e| CodamaError::UnexpectedAddressType(e.to_string())),
            I::IdentityValue(_) | I::PayerValue(_) => {
                Ok(self.accounts.get(account.name.as_str()).copied())
            }
            I::ResolverValue(r) => {
                let f = self
                    .resolvers
                    .get(r.name.as_str())
                    .ok_or_else(|| CodamaError::AccountResolverMissing(r.name.to_string()))?;
                Ok(Some(f(&self.accounts, &self.arguments)?))
            }
            I::ConditionalValue(c) => self.resolve_conditional(c, account, path),
            other => Err(CodamaError::UnexpectedAddressType(format!(
                "{} cannot be resolved to an address",
                other.kind()
            ))),
        }
    }

    fn resolve_conditional(
        &self,
        node: &codama_nodes::ConditionalValueNode,
        account: &InstructionAccountNode,
        path: &[String],
    ) -> CodamaResult<Option<Pubkey>> {
        let condition_holds = self.evaluate_condition(node, path)?;
        let branch = if condition_holds {
            &*node.if_true
        } else {
            &*node.if_false
        };
        match branch {
            Some(v) => self.resolve_input_value(v, account, path),
            None => Ok(None),
        }
    }

    fn evaluate_condition(
        &self,
        node: &codama_nodes::ConditionalValueNode,
        path: &[String],
    ) -> CodamaResult<bool> {
        match node.condition.as_ref() {
            ConditionalValueCondition::Argument(a) => {
                let actual = self.arguments.get(a.name.as_str());
                match node.value.as_ref().as_ref() {
                    Some(expected_node) => {
                        let expected = get_value_with(expected_node, &self.injected)?;
                        Ok(actual.map(|v| *v == expected).unwrap_or(false))
                    }
                    None => Ok(actual.map(is_truthy).unwrap_or(false)),
                }
            }
            ConditionalValueCondition::Account(a) => {
                let resolved = self.resolve_account_by_name(&a.name, path).ok().flatten();
                match node.value.as_ref().as_ref() {
                    Some(expected_node) => {
                        match (resolved, get_value_with(expected_node, &self.injected)?) {
                            (Some(addr), Value::PublicKey(b)) => Ok(addr.to_bytes() == b),
                            _ => Ok(false),
                        }
                    }
                    None => Ok(resolved.is_some()),
                }
            }
            ConditionalValueCondition::Resolver(r) => match self.resolvers.get(r.name.as_str()) {
                Some(f) => Ok(f(&self.accounts, &self.arguments).is_ok()),
                None => Ok(false),
            },
        }
    }

    /// Derives the program address described by a [`PdaValueNode`].
    fn resolve_pda_value(
        &self,
        pda_value: &PdaValueNode,
        path: &[String],
    ) -> CodamaResult<(Pubkey, u8)> {
        let pda_node = match pda_value.pda.as_ref() {
            PdaValuePda::Pda(n) => n,
            PdaValuePda::PdaLink(link) => self.find_pda(&link.name)?,
        };

        let program_id = match &*pda_value.program_id {
            Some(PdaValueProgramId::Account(a)) => self
                .resolve_account_by_name(&a.name, path)?
                .ok_or_else(|| CodamaError::AccountMissing(a.name.to_string()))?,
            Some(PdaValueProgramId::Argument(a)) => {
                let v = self
                    .arguments
                    .get(a.name.as_str())
                    .ok_or_else(|| CodamaError::ArgumentMissing(a.name.to_string()))?;
                value_to_pubkey(v)?
            }
            None => match &pda_node.program_id {
                Some(s) => Pubkey::from_str(s)
                    .map_err(|e| CodamaError::FailedToDerivePda(e.to_string()))?,
                None => self.program_id()?,
            },
        };

        let mut seeds: Vec<Vec<u8>> = Vec::with_capacity(pda_node.seeds.len());
        for seed in &pda_node.seeds {
            seeds.push(match seed {
                PdaSeedNode::Constant(c) => encode_constant_seed(c, &self.injected, &program_id)?,
                PdaSeedNode::Variable(v) => self.encode_variable_seed(v, pda_value, path)?,
            });
        }
        let seed_refs: Vec<&[u8]> = seeds.iter().map(|s| s.as_slice()).collect();
        Ok(Pubkey::find_program_address(&seed_refs, &program_id))
    }

    fn encode_variable_seed(
        &self,
        seed: &VariablePdaSeedNode,
        pda_value: &PdaValueNode,
        path: &[String],
    ) -> CodamaResult<Vec<u8>> {
        let value_node = pda_value
            .seeds
            .iter()
            .find(|s| s.name.as_str() == seed.name.as_str())
            .ok_or_else(|| CodamaError::ArgumentMissing(seed.name.to_string()))?;
        match value_node.value.as_ref() {
            PdaSeedValueValue::Account(a) => {
                let addr = self
                    .resolve_account_by_name(&a.name, path)?
                    .ok_or_else(|| CodamaError::AccountMissing(a.name.to_string()))?;
                Ok(addr.to_bytes().to_vec())
            }
            PdaSeedValueValue::Argument(a) => {
                let v = self
                    .arguments
                    .get(a.name.as_str())
                    .ok_or_else(|| CodamaError::ArgumentMissing(a.name.to_string()))?;
                let codec = get_type_codec(&seed.r#type)?;
                codec.encode_to_vec(v)
            }
            literal => {
                encode_seed_literal(&seed.r#type, literal, &self.injected).unwrap_or_else(|| {
                    Err(CodamaError::UnsupportedNodeKind(
                        "pda seed value".to_string(),
                    ))
                })
            }
        }
    }

    fn program_id(&self) -> CodamaResult<Pubkey> {
        Pubkey::from_str(&self.root.program.public_key)
            .map_err(|e| CodamaError::UnexpectedAddressType(e.to_string()))
    }

    fn find_account_node(&self, name: &str) -> Option<&InstructionAccountNode> {
        self.instruction
            .accounts
            .iter()
            .find(|a| a.name.as_str() == name)
    }

    fn find_pda(&self, name: &str) -> CodamaResult<&PdaNode> {
        std::iter::once(&self.root.program)
            .chain(self.root.additional_programs.iter())
            .flat_map(|p| p.pdas.iter())
            .find(|p| p.name.as_str() == name)
            .ok_or_else(|| CodamaError::LinkedNodeNotFound {
                kind: "pda".to_string(),
                name: name.to_string(),
            })
    }
}

/// Encodes a constant PDA seed. `injected` supplies the values of
/// `injectedValueNode`s.
pub fn encode_constant_seed(
    seed: &ConstantPdaSeedNode,
    injected: &std::collections::HashMap<String, Value>,
    program_id: &Pubkey,
) -> CodamaResult<Vec<u8>> {
    // A `programId` constant seed is the derivation program id itself, encoded
    // as raw address bytes. The seed's declared type is not consulted.
    if let codama_nodes::ConstantPdaSeedValue::ProgramId(_) = seed.value.as_ref() {
        return Ok(program_id.to_bytes().to_vec());
    }
    let codec = get_type_codec(&seed.r#type)?;
    let value_node = constant_seed_to_value_node(seed.value.as_ref())?;
    let value = get_value_with(&value_node, injected)?;
    codec.encode_to_vec(&value)
}

/// Encodes a literal seed value with the seed's declared type. `None` for an
/// `Account` or `Argument` value, which only an instruction's inputs supply.
pub fn encode_seed_literal(
    seed_type: &TypeNode,
    value: &PdaSeedValueValue,
    injected: &std::collections::HashMap<String, Value>,
) -> Option<CodamaResult<Vec<u8>>> {
    let value_node = seed_value_to_value_node(value)?;
    Some(
        get_value_with(&value_node, injected)
            .and_then(|value| get_type_codec(seed_type)?.encode_to_vec(&value)),
    )
}

fn constant_seed_to_value_node(
    value: &codama_nodes::ConstantPdaSeedValue,
) -> CodamaResult<ValueNode> {
    use codama_nodes::ConstantPdaSeedValue as P;
    match value {
        P::Array(n) => Ok(n.clone().into()),
        P::Boolean(n) => Ok((*n).into()),
        P::Bytes(n) => Ok(n.clone().into()),
        P::Constant(n) => Ok(n.clone().into()),
        P::Enum(n) => Ok(n.clone().into()),
        P::Map(n) => Ok(n.clone().into()),
        P::None(n) => Ok((*n).into()),
        P::Number(n) => Ok((*n).into()),
        P::PublicKey(n) => Ok(n.clone().into()),
        P::Set(n) => Ok(n.clone().into()),
        P::Some(n) => Ok(n.clone().into()),
        P::String(n) => Ok(n.clone().into()),
        P::Struct(n) => Ok(n.clone().into()),
        P::Tuple(n) => Ok(n.clone().into()),
        P::Injected(n) => Ok(n.clone().into()),
        P::ProgramId(_) => Err(CodamaError::UnsupportedNodeKind(value.kind().to_string())),
    }
}

/// Converts the literal variants of a [`PdaSeedValueValue`] into a
/// [`ValueNode`]. Returns `None` for the contextual `Account`/`Argument` cases,
/// which must be resolved through context rather than encoded directly.
fn seed_value_to_value_node(value: &PdaSeedValueValue) -> Option<ValueNode> {
    use PdaSeedValueValue as P;
    Some(match value {
        P::Array(n) => n.clone().into(),
        P::Boolean(n) => (*n).into(),
        P::Bytes(n) => n.clone().into(),
        P::Constant(n) => n.clone().into(),
        P::Enum(n) => n.clone().into(),
        P::Map(n) => n.clone().into(),
        P::None(n) => (*n).into(),
        P::Number(n) => (*n).into(),
        P::PublicKey(n) => n.clone().into(),
        P::Set(n) => n.clone().into(),
        P::Some(n) => n.clone().into(),
        P::String(n) => n.clone().into(),
        P::Struct(n) => n.clone().into(),
        P::Tuple(n) => n.clone().into(),
        P::Injected(n) => n.clone().into(),
        P::Account(_) | P::Argument(_) => return None,
    })
}

/// Derives a PDA from a standalone [`PdaNode`], outside any instruction context.
///
/// `seeds` maps each variable seed's name to its runtime [`Value`]; constant
/// seeds are evaluated from the node itself.
pub fn resolve_standalone_pda(
    root: &RootNode,
    pda: &PdaNode,
    seeds: &std::collections::HashMap<String, Value>,
) -> CodamaResult<(Pubkey, u8)> {
    let program_id = match &pda.program_id {
        Some(s) => {
            Pubkey::from_str(s).map_err(|e| CodamaError::FailedToDerivePda(e.to_string()))?
        }
        None => Pubkey::from_str(&root.program.public_key)
            .map_err(|e| CodamaError::UnexpectedAddressType(e.to_string()))?,
    };

    let mut seed_bytes: Vec<Vec<u8>> = Vec::with_capacity(pda.seeds.len());
    for seed in &pda.seeds {
        seed_bytes.push(match seed {
            PdaSeedNode::Constant(c) => {
                encode_constant_seed(c, &std::collections::HashMap::new(), &program_id)?
            }
            PdaSeedNode::Variable(v) => {
                let value = seeds
                    .get(v.name.as_str())
                    .ok_or_else(|| CodamaError::ArgumentMissing(v.name.to_string()))?;
                let codec = get_type_codec(&v.r#type)?;
                codec.encode_to_vec(value)?
            }
        });
    }
    let seed_refs: Vec<&[u8]> = seed_bytes.iter().map(|s| s.as_slice()).collect();
    Ok(Pubkey::find_program_address(&seed_refs, &program_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_nodes::{
        AccountValueNode, ArgumentValueNode, ConditionalValueNode, ConstantPdaSeedNode,
        InstructionNode, PdaSeedValueNode, ProgramIdValueNode, ProgramNode, PublicKeyTypeNode,
        StringTypeNode, StringValueNode,
    };
    use std::collections::HashMap;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn root_with_program(program_pk: &Pubkey) -> RootNode {
        RootNode::new(ProgramNode {
            name: "prog".into(),
            public_key: program_pk.to_string(),
            ..Default::default()
        })
    }

    #[test]
    fn standalone_pda_matches_find_program_address() {
        let program_pk = pk(9);
        let root = root_with_program(&program_pk);
        let pda = PdaNode::new(
            "myPda",
            vec![
                ConstantPdaSeedNode::new(StringTypeNode::utf8(), StringValueNode::new("prefix"))
                    .into(),
                VariablePdaSeedNode::new("authority", PublicKeyTypeNode::new()).into(),
            ],
        );
        let auth = pk(7);
        let mut seeds = HashMap::new();
        seeds.insert("authority".to_string(), Value::PublicKey(auth.to_bytes()));

        let (addr, bump) = resolve_standalone_pda(&root, &pda, &seeds).unwrap();
        let (exp, exp_bump) =
            Pubkey::find_program_address(&[b"prefix", auth.as_ref()], &program_pk);
        assert_eq!((addr, bump), (exp, exp_bump));
    }

    #[test]
    fn program_id_constant_seed_is_the_derivation_program() {
        let program_pk = pk(9);
        let root = root_with_program(&program_pk);
        let pda = PdaNode::new(
            "myPda",
            vec![
                ConstantPdaSeedNode::new(PublicKeyTypeNode::new(), ProgramIdValueNode::new())
                    .into(),
            ],
        );
        let (addr, bump) = resolve_standalone_pda(&root, &pda, &HashMap::new()).unwrap();
        let (exp, exp_bump) = Pubkey::find_program_address(&[program_pk.as_ref()], &program_pk);
        assert_eq!((addr, bump), (exp, exp_bump));

        let other = pk(2);
        let mut pda = PdaNode::new(
            "otherPda",
            vec![
                ConstantPdaSeedNode::new(PublicKeyTypeNode::new(), ProgramIdValueNode::new())
                    .into(),
            ],
        );
        pda.program_id = Some(other.to_string());
        let (addr, bump) = resolve_standalone_pda(&root, &pda, &HashMap::new()).unwrap();
        let (exp, exp_bump) = Pubkey::find_program_address(&[other.as_ref()], &other);
        assert_eq!((addr, bump), (exp, exp_bump));
    }

    #[test]
    fn user_provided_account_short_circuits() {
        let program_pk = pk(4);
        let root = root_with_program(&program_pk);
        let ix = InstructionNode {
            name: "ix".into(),
            accounts: vec![InstructionAccountNode::new("authority", false, true)],
            ..Default::default()
        };
        let auth = pk(8);
        let resolver = AddressResolver::new(&root, &ix).account("authority", auth);
        assert_eq!(resolver.resolve_account("authority").unwrap(), Some(auth));
    }

    #[test]
    fn pda_default_value_resolves() {
        let program_pk = pk(3);
        let root = root_with_program(&program_pk);
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

        let auth = pk(7);
        let resolver = AddressResolver::new(&root, &ix).account("authority", auth);
        let resolved = resolver.resolve_account("pda").unwrap().unwrap();
        let (exp, _) = Pubkey::find_program_address(&[auth.as_ref()], &program_pk);
        assert_eq!(resolved, exp);
    }

    #[test]
    fn optional_account_strategies() {
        let program_pk = pk(5);
        let root = root_with_program(&program_pk);
        let mut opt = InstructionAccountNode::new("maybe", false, false);
        opt.is_optional = Some(true);

        let ix_omitted = InstructionNode {
            name: "ix".into(),
            optional_account_strategy: Some(OptionalAccountStrategy::Omitted),
            accounts: vec![opt.clone()],
            ..Default::default()
        };
        let r = AddressResolver::new(&root, &ix_omitted);
        assert_eq!(r.resolve_account("maybe").unwrap(), None);

        let ix_program_id = InstructionNode {
            name: "ix".into(),
            optional_account_strategy: Some(OptionalAccountStrategy::ProgramId),
            accounts: vec![opt],
            ..Default::default()
        };
        let r2 = AddressResolver::new(&root, &ix_program_id);
        assert_eq!(r2.resolve_account("maybe").unwrap(), Some(program_pk));
    }

    #[test]
    fn required_account_missing_errors() {
        let program_pk = pk(6);
        let root = root_with_program(&program_pk);
        let ix = InstructionNode {
            name: "ix".into(),
            accounts: vec![InstructionAccountNode::new("authority", false, true)],
            ..Default::default()
        };
        let r = AddressResolver::new(&root, &ix);
        assert!(matches!(
            r.resolve_account("authority").unwrap_err(),
            CodamaError::AccountMissing(_)
        ));
    }

    #[test]
    fn program_id_default_value() {
        let program_pk = pk(11);
        let root = root_with_program(&program_pk);
        let mut acc = InstructionAccountNode::new("prog", false, false);
        acc.default_value = Box::new(Some(ProgramIdValueNode::default().into()));
        let ix = InstructionNode {
            name: "ix".into(),
            accounts: vec![acc],
            ..Default::default()
        };
        let r = AddressResolver::new(&root, &ix);
        assert_eq!(r.resolve_account("prog").unwrap(), Some(program_pk));
    }

    #[test]
    fn circular_dependency_detected() {
        let program_pk = pk(1);
        let root = root_with_program(&program_pk);
        let mut a = InstructionAccountNode::new("a", false, false);
        a.default_value = Box::new(Some(AccountValueNode::new("b").into()));
        let mut b = InstructionAccountNode::new("b", false, false);
        b.default_value = Box::new(Some(AccountValueNode::new("a").into()));
        let ix = InstructionNode {
            name: "ix".into(),
            accounts: vec![a, b],
            ..Default::default()
        };
        let r = AddressResolver::new(&root, &ix);
        assert!(matches!(
            r.resolve_account("a").unwrap_err(),
            CodamaError::CircularAccountDependency { .. }
        ));
    }

    #[test]
    fn conditional_picks_branch() {
        let program_pk = pk(2);
        let root = root_with_program(&program_pk);
        let cond = ConditionalValueNode {
            condition: Box::new(ArgumentValueNode::new("useProgram").into()),
            value: Box::new(None),
            if_true: Box::new(Some(ProgramIdValueNode::default().into())),
            if_false: Box::new(None),
        };
        let mut acc = InstructionAccountNode::new("target", false, false);
        acc.is_optional = Some(true);
        acc.default_value = Box::new(Some(cond.into()));
        let ix = InstructionNode {
            name: "ix".into(),
            optional_account_strategy: Some(OptionalAccountStrategy::Omitted),
            accounts: vec![acc],
            ..Default::default()
        };

        // Truthy condition → if_true branch → program id.
        let r = AddressResolver::new(&root, &ix).argument("useProgram", true);
        assert_eq!(r.resolve_account("target").unwrap(), Some(program_pk));

        // Falsy condition → if_false is None → optional omitted → None.
        let r2 = AddressResolver::new(&root, &ix).argument("useProgram", false);
        assert_eq!(r2.resolve_account("target").unwrap(), None);
    }
}
