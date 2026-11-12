//! From the Build form to instructions, and a compiled transaction back onto it.

use crate::state::forms::{IxBody, IxDraft, SignLoad, TxSource};
use crate::state::session::{Session, Tab};
use dioxus::prelude::*;
use wallet_core::edit::{idl_draft_for, open_for_edit, OpenedIx, RawDraft};
use wallet_core::{
    compile_draft, compile_probe, pinned_program, ChainSnapshot, CompiledTx, DecodedTx, IdlDraft,
    IdlRecord, Instruction, Lifetime, PinnedProgram,
};
use wallet_flows::compile::{compile_with_defaults, Draft};

/// A blank block for a pinned program, on the instruction its tab opens with.
pub fn pinned_draft(session: &Session, program: &PinnedProgram) -> IdlDraft {
    session
        .idl_records()
        .into_iter()
        .find(|record| record.program_id == program.program_id)
        .map(|record| draft_for_record(session, &record, Some(program.default_instruction)))
        .unwrap_or_else(IdlDraft::empty)
}

/// A blank block for the first program that has no tab of its own.
pub fn other_draft(session: &Session) -> IdlDraft {
    session
        .idl_records()
        .into_iter()
        .find(|record| pinned_program(&record.program_id).is_none())
        .map(|record| draft_for_record(session, &record, None))
        .unwrap_or_else(IdlDraft::empty)
}

/// A blank block for `record`. An instruction name the IDL does not have, as
/// after importing a different version, falls back to its first instruction.
pub fn draft_for_record(
    session: &Session,
    record: &IdlRecord,
    instruction: Option<&str>,
) -> IdlDraft {
    match session.idls.read().program_client(&record.program_id) {
        Some(client) => {
            let known = instruction.filter(|name| client.instruction_names().contains(name));
            IdlDraft::for_instruction(record.program_id, &record.name, &client, known)
        }
        None => IdlDraft {
            program_id: Some(record.program_id),
            program_name: record.name.clone(),
            ..IdlDraft::empty()
        },
    }
}

/// Encode every block, in order.
pub fn current_instructions(session: &Session) -> Result<Vec<Instruction>, String> {
    let form = session.forms.build;
    compile_blocks(session, &(form.drafts)())
}

/// Encode `drafts`, in order. A block that carries others is written again
/// around what they encode to.
fn compile_blocks(session: &Session, drafts: &[IxDraft]) -> Result<Vec<Instruction>, String> {
    let mut instructions = Vec::new();
    for (i, draft) in drafts.iter().enumerate() {
        let built = compile_block(session, draft).map_err(|err| numbered(i, err))?;
        instructions.push(built);
    }
    Ok(instructions)
}

/// An error of the instruction at `index`, as Build numbers it.
fn numbered(index: usize, err: impl std::fmt::Display) -> String {
    format!("Instruction {} · {err}", index + 1)
}

/// Encode IDL blocks that are not on the form, such as a template's, in
/// order, with the errors numbered as Build's own blocks are.
pub fn build_idl_drafts(
    session: &Session,
    drafts: &mut [IdlDraft],
) -> Result<Vec<Instruction>, String> {
    let idls = session.idls.read();
    drafts
        .iter_mut()
        .enumerate()
        .map(|(i, draft)| {
            let program_id = draft
                .program_id
                .ok_or_else(|| numbered(i, "Choose a program."))?;
            let client = idls
                .program_client(&program_id)
                .ok_or_else(|| numbered(i, "That IDL is no longer stored."))?;
            draft.build(&client).map_err(|err| numbered(i, err))
        })
        .collect()
}

fn compile_block(session: &Session, draft: &IxDraft) -> Result<Instruction, String> {
    match &draft.body {
        IxBody::Idl(_) => compile_idl_draft(session, draft),
        IxBody::Raw(raw) => raw.build(),
        IxBody::Carried { carrier, inner, .. } => {
            let inner = compile_blocks(session, inner)
                .map_err(|err| format!("{} · {err}", carrier.label()))?;
            carrier.wrap(&inner)
        }
    }
}

/// Block `draft` written as its program, accounts and bytes: what it
/// encodes to, when it encodes, or its program alone.
pub fn raw_draft(session: &Session, draft: &IxDraft) -> RawDraft {
    if let Ok(built) = compile_block(session, draft) {
        return RawDraft::from_instruction(&built);
    }
    RawDraft {
        program_id: draft.idl().and_then(|idl| idl.program_id),
        ..RawDraft::default()
    }
}

/// `raw` read with its program's IDL, when one is stored and reads it.
pub fn read_with_idl(session: &Session, raw: &RawDraft) -> Option<IdlDraft> {
    let built = raw.build().ok()?;
    let idls = session.idls.read();
    let record = idls
        .records()
        .iter()
        .find(|record| record.program_id == built.program_id)?;
    let client = idls.program_client(&built.program_id)?;
    idl_draft_for(&client, &record.name, &built)
}

/// Open `tx` on Build to change it. Each instruction comes back as the
/// block it would have been built from, opened as deep as it goes. What
/// was signed stays on Sign until Build compiles a new transaction.
pub fn edit_on_build(session: &Session, tx: &DecodedTx) {
    let opened = open_for_edit(&*session.idls.read(), tx);
    let form = session.forms.build;
    let (carried, raw) = count_blocks(&opened.instructions);
    form.load(opened, session.selected_wallet());
    let mut note = "Opened from Review and Sign. Compiling makes a new transaction, so it is signed again. A blank blockhash is fetched on compile.".to_string();
    if raw > 0 {
        note.push_str(&format!(
            " {raw} instruction(s) no stored IDL reads are shown as accounts and bytes."
        ));
    }
    if carried > 0 {
        note.push_str(" Open a block that carries others to change what is inside it.");
    }
    let mut compiled_note = form.compiled_note;
    compiled_note.set(Some(note));
    session.goto(Tab::Build);
}

/// How many blocks carry others, and how many are raw, at any depth.
fn count_blocks(instructions: &[OpenedIx]) -> (usize, usize) {
    instructions
        .iter()
        .fold((0, 0), |(carried, raw), ix| match ix {
            OpenedIx::Idl(_) => (carried, raw),
            OpenedIx::Raw(_) => (carried, raw + 1),
            OpenedIx::Carried { inner, .. } => {
                let (c, r) = count_blocks(inner);
                (carried + 1 + c, raw + r)
            }
        })
}

/// Hand a compiled transaction to Sign. `template` is the name of the
/// template it was compiled from, if any, and `extra` is what compiling
/// adds; both go in its note, and the name in its source line, the only
/// place Review and Sign names a template.
fn review_compiled(
    session: &Session,
    compiled: CompiledTx,
    template: Option<&str>,
    extra: &str,
    last_valid_height: Option<u64>,
) {
    let from = template
        .map(super::template::template_source_note)
        .unwrap_or_default();
    session.open_in_sign(
        SignLoad::new(
            compiled.wire,
            TxSource::Build {
                template: template.map(str::to_string),
            },
        )
        .with_note(format!("Compiled on Build{from}{extra}."))
        .expiring_at(last_valid_height),
    );
}

/// Encode one block. An account the encoder cannot derive becomes a field on
/// that block, so the next attempt can ask for it.
pub fn compile_idl_draft(session: &Session, draft: &IxDraft) -> Result<Instruction, String> {
    let before = draft.idl().ok_or("This block is not filled from an IDL.")?;
    let mut idl = before.clone();
    let program_id = idl.program_id.ok_or("Choose a program.")?;
    let client = session
        .idls
        .read()
        .program_client(&program_id)
        .ok_or("That IDL is no longer stored.")?;
    let built = idl.build(&client).map_err(|err| err.to_string());
    if idl != *before {
        session.forms.build.patch_idl(draft.id, |item| *item = idl);
    }
    built
}

/// Compile what is on the Build form and open it on Sign. Online, what
/// Advanced options leave blank is filled from the node first.
pub fn compile(session: Session, compiling: Signal<bool>) {
    let form = session.forms.build;
    let (mut error, mut compiled_note) = (form.error, form.compiled_note);
    error.set(None);
    compiled_note.set(None);

    match current_instructions(&session) {
        Ok(instructions) => compile_instructions(session, instructions, None, compiling),
        Err(msg) => error.set(Some(msg)),
    }
}

/// Compile `instructions` with Build's fee payer and Advanced options, and
/// open the transaction on Sign. Online, what Advanced options leave blank
/// is filled from the node first. `template` is the name of the template
/// the instructions were written from, if any.
pub(crate) fn compile_instructions(
    session: Session,
    instructions: Vec<Instruction>,
    template: Option<String>,
    mut compiling: Signal<bool>,
) {
    let form = session.forms.build;
    let mut error = form.error;
    let fee_payer = (form.fee_payer)();
    let settings = form.settings();
    // Offline, or nothing left blank: compile what was typed.
    let rpc = session
        .rpc()
        .ok()
        .filter(|_| settings.wants_blockhash() || settings.wants_limits());
    let Some(rpc) = rpc else {
        match compile_draft(fee_payer, instructions, &settings) {
            Ok(tx) => review_compiled(&session, tx, template.as_deref(), "", None),
            Err(err) => error.set(Some(err.to_string())),
        }
        return;
    };
    // Check what was typed before asking the node anything.
    let probe = match compile_probe(fee_payer, instructions.clone(), &settings) {
        Ok(tx) => tx,
        Err(err) => {
            error.set(Some(err.to_string()));
            return;
        }
    };
    compiling.set(true);
    spawn(async move {
        let draft = Draft {
            fee_payer,
            instructions,
            settings,
        };
        match compile_with_defaults(rpc.as_ref(), draft, &probe).await {
            Ok((tx, note, height)) => {
                review_compiled(&session, tx, template.as_deref(), &note, height)
            }
            Err(err) => error.set(Some(err.to_string())),
        }
        compiling.set(false);
    });
}

/// Fetch the one thing the transaction's lifetime needs into the form: the
/// selected nonce account when a nonce is used, the latest blockhash
/// otherwise. One request.
pub fn fetch_lifetime(session: Session, mut fetching: Signal<bool>) {
    let form = session.forms.build;
    let (mut error, mut compiled_note) = (form.error, form.compiled_note);
    let (mut blockhash, mut nonce_hash) = (form.blockhash, form.nonce_hash);
    let mut nonce_authority = form.nonce_authority;
    error.set(None);
    let wanted = if (form.use_nonce)() {
        let Some(pk) = (form.nonce)() else {
            error.set(Some("Select a nonce account first.".to_string()));
            return;
        };
        Some(pk)
    } else {
        None
    };
    let rpc = session.rpc();
    fetching.set(true);
    spawn(async move {
        let fetched = match rpc {
            Ok(rpc) => wallet_flows::refresh::fetch_lifetime(rpc.as_ref(), wanted).await,
            Err(err) => Err(err),
        };
        match fetched {
            Ok(Lifetime::Blockhash(hash)) => {
                blockhash.set(hash.to_string());
                compiled_note.set(Some("Fetched blockhash.".to_string()));
            }
            Ok(Lifetime::Nonce(found)) => {
                nonce_hash.set(found.hash.to_string());
                nonce_authority.set(Some(found.authority));
                let snap = ChainSnapshot {
                    nonces: vec![found],
                    ..ChainSnapshot::default()
                };
                match session.apply_chain_snapshot(&snap) {
                    Ok(()) => compiled_note.set(Some("Fetched nonce.".to_string())),
                    Err(err) => error.set(Some(err.to_string())),
                }
            }
            Err(err) => error.set(Some(err.to_string())),
        }
        fetching.set(false);
    });
}
