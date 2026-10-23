//! An instruction read back onto its IDL's form.

use crate::idl::build::{idl_instruction_form, IdlAccountSource, IdlArgKind};
use crate::idl::draft::IdlDraft;
use codama_dynamic_client::{NumberValue, ProgramClient, Value};
use codama_nodes::Node;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

/// The form `client` builds `instruction` from, filled in so that it builds
/// those same bytes and accounts. `None` when the IDL does not read it, or
/// when the form cannot hold it: an argument the form has no field for,
/// accounts beyond the ones it names, signer flags it would set otherwise.
pub fn idl_draft_for(
    client: &ProgramClient,
    program_name: &str,
    instruction: &Instruction,
) -> Option<IdlDraft> {
    let parsed = client.parse_instruction(instruction).ok()??;
    let name = match parsed.path.last()? {
        Node::Instruction(node) => node.name.to_string(),
        _ => return None,
    };
    let mut draft =
        IdlDraft::for_instruction(instruction.program_id, program_name, client, Some(&name));
    for account in &parsed.accounts {
        draft.set_account(&account.name, Some(account.meta.pubkey));
    }
    let form = idl_instruction_form(client, &name, &draft.input()).ok()?;
    for argument in &form.arguments {
        let value = match &parsed.data {
            Value::Struct(fields) => fields.get(&argument.name)?,
            _ => return None,
        };
        draft.set_argument(&argument.name, argument_text(&argument.kind, value)?);
    }

    // A derived account that is what the IDL derives stays derived, so it
    // follows its seeds when they are edited. One that is not is filled in.
    let full = draft.clone();
    let form = idl_instruction_form(client, &name, &draft.input()).ok()?;
    for field in &form.accounts {
        if field.source != IdlAccountSource::Derived {
            continue;
        }
        if field.address.is_some() && field.address == full.account(&field.name) {
            draft.set_account(&field.name, None);
        } else if full.account(&field.name).is_some() {
            draft.manual.push(field.name.clone());
        }
    }
    if builds_the_same(client, &draft, instruction) {
        return Some(draft);
    }
    // Every account as it came.
    let mut full = full;
    full.manual = form
        .accounts
        .iter()
        .filter(|field| field.source == IdlAccountSource::Derived)
        .filter(|field| full.account(&field.name).is_some())
        .map(|field| field.name.clone())
        .collect();
    builds_the_same(client, &full, instruction).then_some(full)
}

fn builds_the_same(client: &ProgramClient, draft: &IdlDraft, instruction: &Instruction) -> bool {
    let mut draft = draft.clone();
    draft.build(client).is_ok_and(|built| built == *instruction)
}

/// The text an argument field holds for `value`, as `parse_argument` reads it.
fn argument_text(kind: &IdlArgKind, value: &Value) -> Option<String> {
    match (kind, value) {
        (IdlArgKind::Option(_), Value::Option(None)) => Some(String::new()),
        (IdlArgKind::Option(inner), Value::Option(Some(value))) => {
            let text = argument_text(inner, value)?;
            // Blank reads as none.
            (!text.trim().is_empty()).then_some(text)
        }
        (IdlArgKind::Integer { .. }, Value::Number(number)) => Some(match number {
            NumberValue::Unsigned(n) => n.to_string(),
            NumberValue::Signed(n) => n.to_string(),
            NumberValue::Float(n) => n.to_string(),
        }),
        (IdlArgKind::Pubkey, Value::PublicKey(bytes)) => {
            Some(Pubkey::new_from_array(*bytes).to_string())
        }
        (IdlArgKind::String, Value::String(text)) => Some(text.clone()),
        (IdlArgKind::Bool, Value::Bool(flag)) => Some(flag.to_string()),
        (IdlArgKind::Enum { variants }, Value::Enum { variant, .. }) => {
            variants.contains(variant).then(|| variant.clone())
        }
        (IdlArgKind::Bytes, Value::Bytes(bytes)) => Some(hex::encode(bytes)),
        _ => None,
    }
}
