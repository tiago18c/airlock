//! Resolving account addresses and building [`AccountMeta`]s.
//!
//! Rust counterpart of the TypeScript `dynamic-instructions/accounts`
//! sub-package. Native [`solana_instruction::AccountMeta`] expresses a role as
//! the `(is_signer, is_writable)` pair rather than web3.js's `AccountRole`
//! enum, so the role computation collapses to those two booleans.

use codama_dynamic_codecs::Value;
use codama_dynamic_resolution::{
    value_to_pubkey, AccountsInput, AddressResolver, ArgumentsInput, ResolverFn,
};
use codama_nodes::{
    InstructionAccountNode, InstructionNode, InstructionRemainingAccountsValue, IsSigner,
    OptionalAccountStrategy, RootNode,
};
use codama_visitors_core::{DynamicError as CodamaError, DynamicResult as CodamaResult};
use solana_instruction::AccountMeta;
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;

/// Resolves every instruction account to an [`AccountMeta`], then appends any
/// remaining accounts drawn from argument values.
///
/// Optional accounts resolved via the `omitted` strategy contribute no meta;
/// those resolved to the program address via the `programId` strategy are
/// downgraded to read-only (the program address can never be writable).
pub fn create_account_metas(
    root: &RootNode,
    ix: &InstructionNode,
    arguments: &ArgumentsInput,
    accounts: &AccountsInput,
    signers: &[String],
    address_resolvers: HashMap<String, ResolverFn>,
) -> CodamaResult<Vec<AccountMeta>> {
    let mut resolver = AddressResolver::new(root, ix);
    for (name, pubkey) in accounts {
        resolver = resolver.account(name.clone(), *pubkey);
    }
    for (name, value) in arguments {
        resolver = resolver.argument(name.clone(), value.clone());
    }
    for (name, f) in address_resolvers {
        resolver = resolver.resolver(name, f);
    }

    let addresses = resolver.resolve_accounts()?;
    account_metas_from_addresses(root, ix, &addresses, arguments, accounts, signers)
}

/// Builds account metas from addresses already resolved in declaration order.
pub(crate) fn account_metas_from_addresses(
    root: &RootNode,
    ix: &InstructionNode,
    addresses: &[Option<Pubkey>],
    arguments: &ArgumentsInput,
    accounts: &AccountsInput,
    signers: &[String],
) -> CodamaResult<Vec<AccountMeta>> {
    let program_id = Pubkey::from_str(&root.program.public_key)
        .map_err(|e| CodamaError::UnexpectedAddressType(e.to_string()))?;

    let mut metas = Vec::with_capacity(ix.accounts.len());
    for (account, address) in ix.accounts.iter().zip(addresses) {
        let Some(address) = *address else {
            // Optional account omitted under the `omitted` strategy.
            continue;
        };

        let is_provided = accounts.contains_key(account.name.as_str());
        let downgrade = account.is_optional.unwrap_or(false)
            && !is_provided
            && ix.optional_account_strategy == Some(OptionalAccountStrategy::ProgramId)
            && address == program_id;

        let is_signer = is_signer_account(account, signers);
        let is_writable = if downgrade {
            false
        } else {
            account.is_writable
        };
        metas.push(AccountMeta {
            pubkey: address,
            is_signer,
            is_writable,
        });
    }

    append_remaining_accounts(ix, arguments, &mut metas)?;
    Ok(metas)
}

/// Resolves the instruction's remaining accounts from argument values.
///
/// See <https://github.com/codama-idl/codama/blob/main/packages/nodes/docs/InstructionRemainingAccountsNode.md>.
fn append_remaining_accounts(
    ix: &InstructionNode,
    arguments: &ArgumentsInput,
    metas: &mut Vec<AccountMeta>,
) -> CodamaResult<()> {
    for remaining in &ix.remaining_accounts {
        let arg_name = match remaining.value.as_ref() {
            InstructionRemainingAccountsValue::Argument(a) => a.name.as_str(),
            InstructionRemainingAccountsValue::Resolver(_) => {
                return Err(CodamaError::UnexpectedNode {
                    expected: "argumentValueNode".to_string(),
                    actual: "resolverValueNode".to_string(),
                });
            }
        };

        let Some(value) = arguments.get(arg_name) else {
            // Required remaining accounts must be provided; optional ones skip.
            if !remaining.is_optional.unwrap_or(false) {
                return Err(CodamaError::ArgumentMissing(arg_name.to_string()));
            }
            continue;
        };

        let Value::Array(items) = value else {
            return Err(CodamaError::InvalidArgumentInput {
                argument: arg_name.to_string(),
                expected: "an array of public keys".to_string(),
            });
        };

        let (is_signer, is_writable) = remaining_account_role(
            remaining.is_signer.unwrap_or(IsSigner::False),
            remaining.is_writable.unwrap_or(false),
        );
        for item in items {
            metas.push(AccountMeta {
                pubkey: value_to_pubkey(item)?,
                is_signer,
                is_writable,
            });
        }
    }
    Ok(())
}

/// Whether an instruction account acts as a signer, consulting the caller's
/// `signers` list for accounts whose signer status is ambiguous (`either`).
fn is_signer_account(account: &InstructionAccountNode, signers: &[String]) -> bool {
    match account.is_signer {
        IsSigner::True => true,
        IsSigner::False => false,
        IsSigner::Either => signers.iter().any(|s| s == account.name.as_str()),
    }
}

/// Role for a remaining account. `either` is treated as a signer, mirroring the
/// TypeScript behaviour (works for Token Program multisig signers).
fn remaining_account_role(is_signer: IsSigner, is_writable: bool) -> (bool, bool) {
    let signer = matches!(is_signer, IsSigner::True | IsSigner::Either);
    (signer, is_writable)
}
