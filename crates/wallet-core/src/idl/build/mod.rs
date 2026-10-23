//! Build one instruction from a stored IDL.
//!
//! The form is a description of an [`InstructionNode`]: omitted defaults stay
//! hidden, PDA defaults are derived, and accounts the encoder cannot fill ask
//! for an address. After a successful build the same client parses the
//! instruction back. A different name refuses the instruction.

use codama_dynamic_client::{
    encode_constant_seed, encode_seed_literal, get_type_codec, DynamicError, ProgramClient, Value,
};
use codama_nodes::{
    ConstantPdaSeedValue, DefaultValueStrategy, HasKind, InstructionAccountNode,
    InstructionArgumentNode, InstructionInputValueNode, NestedTypeNodeTrait, Node, Number,
    NumberFormat, PdaSeedNode, PdaSeedValueValue, PdaValueNode, PdaValuePda, PdaValueProgramId,
    RootNode, TypeNode,
};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;

mod arguments;
mod form;
mod pda;
mod types;

#[cfg(test)]
mod tests;

use arguments::*;
pub use form::*;
use pda::*;
pub use types::*;

/// Builds the instruction and parses it back with the same client.
///
/// A parsed name other than `instruction` is refused. A PDA or other default
/// the encoder cannot resolve returns [`IdlBuildFailure::FillAddress`] for that
/// account and does not drop the rest of the form.
pub fn build_idl_instruction(
    client: &ProgramClient,
    instruction: &str,
    input: &IdlBuildInput,
) -> Result<Instruction, IdlBuildFailure> {
    let form =
        idl_instruction_form(client, instruction, input).map_err(IdlBuildFailure::Message)?;
    let ready = prepare(&form, input)?;
    match run_builder(client, instruction, &ready, None) {
        Ok(built) => confirm_name(client, instruction, built),
        Err(err) if is_gap(&err) => Err(attribute_gap(client, instruction, &form, &ready, err)),
        Err(err) => Err(IdlBuildFailure::Message(err.to_string())),
    }
}

struct Ready {
    accounts: Vec<(String, Pubkey)>,
    arguments: Vec<(String, Value)>,
    /// Accounts the form shows as signers. An IDL may leave an authority's
    /// signer status open (`either`) so a multisig can stand in. This form has
    /// no multisig signer list, so the authority itself always signs.
    signers: Vec<String>,
}

fn prepare(form: &IdlForm, input: &IdlBuildInput) -> Result<Ready, IdlBuildFailure> {
    let supplied = filled_accounts(input);
    let mut accounts = Vec::new();
    for field in &form.accounts {
        match (
            field.source,
            supplied.get(&field.name).copied(),
            field.optional,
        ) {
            (_, Some(address), _) => accounts.push((field.name.clone(), address)),
            (IdlAccountSource::Derived, None, _) => {}
            (IdlAccountSource::Picker, None, true) => {}
            (IdlAccountSource::Picker, None, false) => {
                return Err(IdlBuildFailure::Message(format!("Pick a {}.", field.name)));
            }
            (IdlAccountSource::Fill, None, _) => {
                return Err(IdlBuildFailure::FillAddress {
                    account: field.name.clone(),
                    reason: field.reason.clone(),
                });
            }
        }
    }

    let text = argument_text(input);
    let mut arguments = Vec::new();
    for field in &form.arguments {
        let raw = text.get(&field.name).map(String::as_str).unwrap_or("");
        if let Some(value) = parse_argument(&field.kind, raw, field.blank_uses_default)
            .map_err(IdlBuildFailure::Message)?
        {
            arguments.push((field.name.clone(), value))
        }
    }
    let signers = form
        .accounts
        .iter()
        .filter(|field| field.signer)
        .map(|field| field.name.clone())
        .collect();
    Ok(Ready {
        accounts,
        arguments,
        signers,
    })
}

fn run_builder(
    client: &ProgramClient,
    instruction: &str,
    ready: &Ready,
    extra: Option<(&str, Pubkey)>,
) -> Result<Instruction, DynamicError> {
    let mut builder = client.method(instruction)?;
    for (name, address) in &ready.accounts {
        builder = builder.account(name, *address);
    }
    if let Some((name, address)) = extra {
        if !ready.accounts.iter().any(|(have, _)| have == name) {
            builder = builder.account(name, address);
        }
    }
    for (name, value) in &ready.arguments {
        builder = builder.argument(name, value.clone());
    }
    builder.signers(ready.signers.iter().cloned()).build()
}

fn confirm_name(
    client: &ProgramClient,
    instruction: &str,
    built: Instruction,
) -> Result<Instruction, IdlBuildFailure> {
    let parsed = client
        .parse_instruction(&built)
        .map_err(|err| IdlBuildFailure::Message(err.to_string()))?;
    let Some(parsed) = parsed else {
        return Err(IdlBuildFailure::Message(format!(
            "Built {instruction} did not parse back to an instruction. Refusing to stage this instruction."
        )));
    };
    let got = match parsed.path.last() {
        Some(Node::Instruction(node)) => node.name.as_str(),
        _ => "",
    };
    if got != instruction {
        return Err(IdlBuildFailure::Message(format!(
            "Built {instruction} parsed back as {got}. Refusing to stage this instruction."
        )));
    }
    Ok(built)
}

fn attribute_gap(
    client: &ProgramClient,
    instruction: &str,
    form: &IdlForm,
    ready: &Ready,
    err: DynamicError,
) -> IdlBuildFailure {
    let reason = err.to_string();
    let dummy = Pubkey::new_from_array([255; 32]);
    let derived: Vec<&str> = form
        .accounts
        .iter()
        .filter(|field| {
            field.source == IdlAccountSource::Derived
                && !ready.accounts.iter().any(|(name, _)| name == &field.name)
        })
        .map(|field| field.name.as_str())
        .collect();
    for name in &derived {
        match run_builder(client, instruction, ready, Some((name, dummy))) {
            Ok(_) => {
                return IdlBuildFailure::FillAddress {
                    account: (*name).to_string(),
                    reason: reason.clone(),
                };
            }
            Err(next) if !is_gap(&next) => {
                return IdlBuildFailure::FillAddress {
                    account: (*name).to_string(),
                    reason,
                };
            }
            Err(_) => {}
        }
    }
    match derived.first() {
        Some(name) => IdlBuildFailure::FillAddress {
            account: (*name).to_string(),
            reason,
        },
        None => IdlBuildFailure::Message(reason),
    }
}

fn is_gap(err: &DynamicError) -> bool {
    matches!(
        err,
        DynamicError::FailedToDerivePda(_)
            | DynamicError::UnexpectedAddressType(_)
            | DynamicError::AccountResolverMissing(_)
            | DynamicError::UnsupportedNodeKind(_)
            | DynamicError::InjectedValueMissing(_)
            | DynamicError::UnrecognizedNodeKind(_)
    )
}

fn filled_accounts(input: &IdlBuildInput) -> HashMap<String, Pubkey> {
    input
        .accounts
        .iter()
        .filter_map(|(name, address)| address.map(|pk| (name.clone(), pk)))
        .collect()
}

fn argument_text(input: &IdlBuildInput) -> HashMap<String, String> {
    input
        .arguments
        .iter()
        .map(|(name, text)| (name.clone(), text.clone()))
        .collect()
}
