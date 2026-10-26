//! Second pass over a decoded v1 transaction: what each instruction means,
//! read from the IDL stored for its program. An instruction with no IDL, or
//! that its IDL cannot parse, is left as raw accounts and bytes.
//!
//! Some instructions carry other instructions in their data: a programmatic
//! signer's `Submit`, the executor call inside it, a transaction stored for a
//! Squads vault, a Vector `Passthrough`. Those are opened and explained the
//! same way, level by level. Each opens from its own bytes. What a Vector
//! `Advance` does to a hash-based key is read with the accounts' schemes,
//! when they are known.

use crate::idl::catalog::{IdlCatalog, IdlCheck, IdlOrigin};
use crate::tx::decode::DecodedTx;
use crate::wallets::programmatic::{
    message_instructions, programmatic_signer_address, Execution, Submission, SubmitRole,
    ED25519_SIGNER_PROGRAM_ID,
};
use crate::wallets::relay::{advance_next_keys, AuthContext, VectorContext};
use crate::wallets::squads::StoredTransaction;
use crate::wallets::vector::{parse_passthrough, SchemeId, PASSTHROUGH_DISCRIMINATOR, PROGRAM_ID};
use codama_dynamic_client::{
    get_instruction_display, required_accounts_for_display, IntentSpan, ProgramClient,
};
use codama_nodes::Node;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;

mod types;

pub use types::*;

#[cfg(test)]
mod tests;

/// One parsed client and its checksum per program, for the length of one explanation.
type Clients = HashMap<Pubkey, Option<(Arc<ProgramClient>, [u8; 32])>>;

/// How many levels of instructions inside instructions are opened.
const MAX_DEPTH: usize = 4;

fn is_vector_passthrough(instruction: &Instruction) -> bool {
    instruction.program_id == PROGRAM_ID
        && instruction.data.first() == Some(&PASSTHROUGH_DISCRIMINATOR)
}

/// What the transaction's `Advance` does to the key of each Winternitz and
/// XMSS account it lists, as far as its layout can be read: from the schemes
/// in `note`, or from the data's length when that allows one reading.
fn key_changes(tx: &DecodedTx, note: &DisplayNote) -> HashMap<Pubkey, NextKey> {
    let known: Vec<VectorContext> = note
        .schemes
        .iter()
        .map(|(account, scheme)| VectorContext {
            account: *account,
            scheme: *scheme,
            nonce: None,
            key: note.keys.get(account).cloned(),
            leaf_floor: 0,
        })
        .collect();
    advance_next_keys(tx, &AuthContext { vector: &known })
        .into_iter()
        .filter_map(|(account, scheme, next)| {
            let change = match (scheme, note.keys.get(&account)) {
                // XMSS names the key it holds to keep it.
                (SchemeId::Xmss, Some(current)) if *current == next => return None,
                (SchemeId::Xmss, Some(_)) => NextKey::Replaces(next),
                (SchemeId::Xmss, None) => NextKey::Names(next),
                // A Winternitz key signs once, so the account always moves.
                _ => NextKey::Replaces(next),
            };
            Some((account, change))
        })
        .collect()
}

/// The instructions `instruction` carries in its data, if it is one of the
/// kinds this app can open. `changes` are what the transaction's `Advance`
/// does to the keys of the accounts a `Passthrough` runs as.
fn open_instruction(
    instruction: &Instruction,
    changes: &HashMap<Pubkey, NextKey>,
) -> Option<(InnerKind, Vec<Instruction>)> {
    if instruction.program_id == ED25519_SIGNER_PROGRAM_ID {
        let submission = Submission::parse(&instruction.data)?;
        let mut authorities = Vec::new();
        let mut forwarded = Vec::new();
        for signer in &submission.signers {
            match signer.role {
                SubmitRole::Authority { signed, .. } => authorities.push(AuthorizingKey {
                    authority: signer.pubkey,
                    address: programmatic_signer_address(&signer.pubkey),
                    signed,
                }),
                SubmitRole::Forwarded => forwarded.push(signer.pubkey),
            }
        }
        return Some((
            InnerKind::Authorization {
                authorities,
                forwarded,
            },
            message_instructions(&submission.message),
        ));
    }
    if let Some(execution) = Execution::parse(instruction) {
        return Some((
            InnerKind::Execution {
                nonce_account: execution.nonce_account,
                nonce_authority: execution.nonce_authority,
                nonce_value: execution.nonce_value,
            },
            execution.instructions(),
        ));
    }
    if let Some(stored) = StoredTransaction::parse(instruction) {
        return Some((
            InnerKind::SquadsTransaction {
                vault: stored.vault,
                vault_index: stored.vault_index,
            },
            stored.instructions,
        ));
    }
    if is_vector_passthrough(instruction) {
        let parsed = parse_passthrough(instruction).ok()?;
        let signers = parsed
            .signers
            .iter()
            .map(|signer| RunsAs {
                account: signer.account,
                system: signer.system.then(|| signer.address()),
                next_key: changes.get(&signer.account).cloned(),
            })
            .collect();
        return Some((InnerKind::Passthrough { signers }, parsed.instructions));
    }
    None
}

/// Every program `instructions` call, at any depth, first seen first.
fn programs_called(instructions: &[Instruction], depth: usize, out: &mut Vec<Pubkey>) {
    for instruction in instructions {
        if !out.contains(&instruction.program_id) {
            out.push(instruction.program_id);
        }
        if depth < MAX_DEPTH {
            if let Some((_, inner)) = open_instruction(instruction, &HashMap::new()) {
                programs_called(&inner, depth + 1, out);
            }
        }
    }
}

/// Programs in `tx`, at any depth, that this device has no IDL stored for.
pub fn missing_catalog_programs(store: &dyn IdlCatalog, tx: &DecodedTx) -> Vec<Pubkey> {
    let mut called = Vec::new();
    programs_called(&tx.native_instructions(), 0, &mut called);
    called.retain(|program_id| {
        !store
            .records()
            .iter()
            .any(|record| record.program_id == *program_id)
    });
    called
}

pub fn explain_transaction(store: &dyn IdlCatalog, tx: &DecodedTx) -> ExplainedTx {
    explain_with_note(store, tx, &DisplayNote::default())
}

pub fn explain_with_note(
    store: &dyn IdlCatalog,
    tx: &DecodedTx,
    note: &DisplayNote,
) -> ExplainedTx {
    let native = tx.native_instructions();
    let mut called = Vec::new();
    programs_called(&native, 0, &mut called);
    let programs = called
        .into_iter()
        .map(|program_id| program_line(store, program_id, note))
        .collect();
    let mut clients = Clients::new();
    let changes = key_changes(tx, note);
    let instructions = native
        .iter()
        .map(|instruction| explain_nested(store, &mut clients, instruction, note, &changes, 0))
        .collect();
    ExplainedTx {
        programs,
        instructions,
    }
}

/// Explain instructions that are not a transaction's own, such as the calls a
/// simulation reports one program making into another.
pub fn explain_instructions(
    store: &dyn IdlCatalog,
    instructions: &[Instruction],
) -> Vec<ExplainedIx> {
    let mut clients = Clients::new();
    let note = DisplayNote::default();
    let changes = HashMap::new();
    instructions
        .iter()
        .map(|instruction| explain_nested(store, &mut clients, instruction, &note, &changes, 0))
        .collect()
}

fn explain_nested(
    store: &dyn IdlCatalog,
    clients: &mut Clients,
    instruction: &Instruction,
    note: &DisplayNote,
    changes: &HashMap<Pubkey, NextKey>,
    depth: usize,
) -> ExplainedIx {
    let opened = (depth < MAX_DEPTH)
        .then(|| open_instruction(instruction, changes))
        .flatten();
    let inner = opened.as_ref().map(|(kind, instructions)| InnerMessage {
        kind: kind.clone(),
        instructions: instructions
            .iter()
            .map(|inner| explain_nested(store, clients, inner, note, changes, depth + 1))
            .collect(),
    });
    // A Passthrough that cannot be read must not be worded by the IDL alone:
    // that would hide what it carries from quick_sign and Summary.
    let idl = if is_vector_passthrough(instruction) && opened.is_none() {
        None
    } else {
        explain_one(store, clients, instruction, note)
    };
    ExplainedIx {
        program_id: instruction.program_id,
        accounts: instruction
            .accounts
            .iter()
            .map(|meta| meta.pubkey)
            .collect(),
        data: instruction.data.clone(),
        idl,
        inner,
    }
}

fn program_line(store: &dyn IdlCatalog, program_id: Pubkey, note: &DisplayNote) -> ProgramLine {
    match store
        .records()
        .iter()
        .find(|record| record.program_id == program_id)
    {
        Some(record) => ProgramLine {
            program_id,
            name: Some(record.name.clone()),
            sha256: Some(record.sha256),
            origin: Some(record.origin),
            status: IdlPinStatus::of(record.check.as_ref()),
            context_pin: context_pin(program_id, Some(record.sha256), note),
        },
        None => ProgramLine {
            program_id,
            name: None,
            sha256: None,
            origin: None,
            status: IdlPinStatus::NoIdl,
            context_pin: context_pin(program_id, None, note),
        },
    }
}

fn context_pin(
    program_id: Pubkey,
    local_sha: Option<[u8; 32]>,
    note: &DisplayNote,
) -> Option<ContextPinMatch> {
    let sha = note
        .pins
        .iter()
        .find(|(id, _)| *id == program_id)
        .map(|(_, sha)| *sha)?;
    Some(match local_sha {
        Some(local) if local == sha => ContextPinMatch::Same,
        Some(_) => ContextPinMatch::Differs,
        None => ContextPinMatch::Missing,
    })
}

fn explain_one(
    store: &dyn IdlCatalog,
    clients: &mut Clients,
    instruction: &Instruction,
    note: &DisplayNote,
) -> Option<IdlExplanation> {
    let program_id = instruction.program_id;
    let slot = clients.entry(program_id).or_insert_with(|| {
        let sha256 = store
            .records()
            .iter()
            .find(|record| record.program_id == program_id)
            .map(|record| record.sha256)?;
        let client = store.program_client(&program_id)?;
        Some((client, sha256))
    });
    let (client, sha256) = slot.as_ref()?;
    let parsed = client.parse_instruction(instruction).ok().flatten()?;
    let empty = HashMap::new();
    let supplied = if note_supplies_accounts(&program_id, Some(*sha256), note) {
        &note.accounts
    } else {
        &empty
    };
    let display = get_instruction_display(client.root(), &parsed, supplied, &HashMap::new())?;
    let mut accounts = Vec::with_capacity(parsed.accounts.len() + parsed.remaining_accounts.len());
    for account in &parsed.accounts {
        accounts.push(IdlAccountLine {
            name: account.name.clone(),
            pubkey: account.meta.pubkey,
            signer: account.meta.is_signer,
            writable: account.meta.is_writable,
        });
    }
    for meta in &parsed.remaining_accounts {
        accounts.push(IdlAccountLine {
            name: "remaining".into(),
            pubkey: meta.pubkey,
            signer: meta.is_signer,
            writable: meta.is_writable,
        });
    }
    let allows_more = match parsed.path.last() {
        Some(Node::Instruction(node)) => !node.remaining_accounts.is_empty(),
        _ => false,
    };
    let unnamed_accounts = if allows_more {
        0
    } else {
        parsed.remaining_accounts.len()
    };
    Some(IdlExplanation {
        unnamed_accounts,
        idl_sha256: *sha256,
        intent: display.intent,
        interpolated_intent: display.interpolated_intent,
        spans: display.spans,
        fields: display
            .fields
            .into_iter()
            .map(|field| IdlField {
                label: field.label,
                value: field.value,
            })
            .collect(),
        accounts_needed: required_accounts_for_display(client.root(), &parsed),
        accounts,
    })
}
