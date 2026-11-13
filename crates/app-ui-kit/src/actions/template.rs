//! A template on Build: opening one, its answers, the chain facts it needs,
//! and turning it into blocks or a transaction.
//!
//! The form's state is [`OpenTemplate`] on Build's form. Expanding it is
//! cheap (well under a millisecond: the IDL clients are cached), so a screen
//! keeps [`expand_open`] in a memo and every action expands again from the
//! state as it is.

use super::build::{build_idl_drafts, compile_instructions};
use super::spawn_app_task;
use crate::state::forms::{
    FactRead, FactReadState, GeneratedKey, IxBody, OpenTemplate, TemplateSource,
};
use crate::state::session::Session;
use dioxus::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use wallet_core::template::{Answers, Env, Expansion, Fact, Missing, Problem, TemplateForm};
use wallet_core::BUILTIN_TEMPLATES;

/// How long the answers must stay still before the node is asked for the
/// facts they decide.
pub const FACT_READ_DELAY: Duration = Duration::from_millis(400);

/// One row of the list of templates.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateEntry {
    pub source: TemplateSource,
    pub name: String,
    pub summary: Option<String>,
    /// Why it cannot be opened on the network in use, in words: the first
    /// problem, or why the file is not a template. `None` when it can.
    pub problem: Option<String>,
    /// Every problem `check` finds on the network in use, in words. Empty
    /// when it can be opened, and for a file that is not a template.
    pub problems: Vec<String>,
}

impl TemplateEntry {
    /// A file kept as one of the user's that is not a template.
    pub fn is_broken(&self) -> bool {
        matches!(self.source, TemplateSource::Broken { .. })
    }
}

/// Every template there is, checked against the IDLs of the network in use:
/// the built-in ones, then the user's, once they are read (see
/// [`super::kept_templates::ensure_kept`]).
pub fn template_entries(session: &Session) -> Vec<TemplateEntry> {
    let kept = super::kept_templates::kept_templates(session);
    let idls = session.idls.read();
    BUILTIN_TEMPLATES
        .iter()
        .map(|builtin| TemplateSource::Builtin(builtin.id))
        .chain(kept.kept)
        .map(|source| match source.template() {
            Ok(template) => {
                let problems = wallet_core::template::check(&template, &*idls)
                    .err()
                    .unwrap_or_default();
                TemplateEntry {
                    problem: (!problems.is_empty()).then(|| first_problem(&problems)),
                    problems: problems.iter().map(super::sentence).collect(),
                    name: template.name,
                    summary: template.summary,
                    source,
                }
            }
            Err(err) => TemplateEntry {
                name: match &source {
                    TemplateSource::Builtin(id) => id.to_string(),
                    other => format!("{}.toml", other.slug().unwrap_or_default()),
                },
                summary: None,
                problem: Some(err),
                problems: Vec::new(),
                source,
            },
        })
        .collect()
}

/// The first of `problems`, as a sentence.
fn first_problem(problems: &[Problem]) -> String {
    problems
        .first()
        .map(super::sentence)
        .unwrap_or_else(|| "It does not fit the IDLs of this network.".to_string())
}

/// Open the template `source` in place of the blocks, which are dropped:
/// the screen asks first. A keypair is made for each `new-keypair` value,
/// kept with the keys Build makes, and stays the same while the template is
/// open. Fee payer and Advanced options stay as they are.
pub fn open_template(session: &Session, source: TemplateSource) -> Result<(), String> {
    let template = source.template()?;
    let form = session.forms.build;
    let mut keypairs = BTreeMap::new();
    let mut generated = form.generated;
    for name in template.keypair_values() {
        let key = GeneratedKey::new();
        keypairs.insert(name.to_string(), key.pubkey);
        generated.write().push(key);
    }
    let opened = {
        let mut next_id = form.next_id;
        let id = *next_id.peek();
        next_id.set(id + 1);
        id
    };
    let (mut drafts, mut selected) = (form.drafts, form.selected);
    let (mut error, mut note) = (form.error, form.compiled_note);
    let (mut open, mut fact_read) = (form.template, form.fact_read);
    let mut reset = form.picker_reset;
    drafts.set(Vec::new());
    selected.set(None);
    error.set(None);
    note.set(None);
    let next = fact_read.peek().after();
    fact_read.set(next);
    let next = *reset.peek() + 1;
    reset.set(next);
    open.set(Some(OpenTemplate {
        opened,
        source,
        answers: Answers::for_template(&template),
        template: Arc::new(template),
        typed_facts: BTreeMap::new(),
        read_facts: Default::default(),
        keypairs,
        marked: BTreeSet::new(),
    }));
    Ok(())
}

/// Close the template and leave Build empty. The keys it made stay, as
/// Build's own do.
pub fn close_template(session: &Session) {
    let form = session.forms.build;
    let (mut open, mut fact_read) = (form.template, form.fact_read);
    let (mut error, mut note) = (form.error, form.compiled_note);
    let mut reset = form.picker_reset;
    open.set(None);
    let next = fact_read.peek().after();
    fact_read.set(next);
    error.set(None);
    note.set(None);
    let next = *reset.peek() + 1;
    reset.set(next);
}

fn change(session: &Session, f: impl FnOnce(&mut OpenTemplate)) {
    let mut open = session.forms.build.template;
    let mut open = open.write();
    if let Some(template) = open.as_mut() {
        f(template);
    }
}

/// Answer input `name` with `text`, as typed. An empty `text` clears it,
/// default and all.
pub fn answer(session: &Session, name: &str, text: String) {
    change(session, |open| {
        open.answers.inputs.insert(name.to_string(), text);
    });
}

/// Turn option `name` on or off.
pub fn set_option(session: &Session, name: &str, on: bool) {
    change(session, |open| {
        if on {
            open.answers.options.insert(name.to_string());
        } else {
            open.answers.options.remove(name);
        }
    });
}

/// Type the answer to a chain fact by hand.
pub fn type_fact(session: &Session, fact: Fact, text: String) {
    change(session, |open| {
        open.typed_facts.insert(fact, text);
    });
}

/// What the template is expanded against: the address the selected wallet
/// acts as (a multisig's vault, not its member), Build's fee payer, and the
/// keys made for it.
pub fn template_env(session: &Session, open: &OpenTemplate) -> Env {
    let wallet = session
        .selected_wallet()
        .and_then(|id| session.route_of(id).ok())
        .map(|route| route.address);
    Env {
        wallet,
        fee_payer: (session.forms.build.fee_payer)(),
        keypairs: open.keypairs.clone(),
    }
}

/// The open template, what it was expanded against, and its expansion.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenExpansion {
    pub open: OpenTemplate,
    pub env: Env,
    /// The questions and options, each input with its widget. Empty when
    /// the template does not fit the IDLs, which `expansion` then says.
    pub form: TemplateForm,
    pub expansion: Result<Expansion, Vec<Problem>>,
}

/// Expand the open template from the form as it is. Reads the template,
/// the fee payer, the wallets and the IDLs, so a memo over it follows each.
pub fn expand_open(session: &Session) -> Option<OpenExpansion> {
    let open = (session.forms.build.template)()?;
    let env = template_env(session, &open);
    let idls = session.idls.read();
    let form = wallet_core::template::check(&open.template, &*idls).unwrap_or(TemplateForm {
        inputs: Vec::new(),
        options: Vec::new(),
        unavailable: Vec::new(),
    });
    let expansion =
        wallet_core::template::expand(&open.template, &open.answers, &env, &open.facts(), &*idls);
    drop(idls);
    Some(OpenExpansion {
        open,
        env,
        form,
        expansion,
    })
}

/// Whether the form asks for the chain facts by hand: offline, when the
/// node would not answer, and while any fact is typed. Typing one makes it
/// known, so nothing is needed from the node any more and its failure is
/// forgotten; the fields stay, so the rest of a number can be typed and a
/// typed value corrected.
pub fn asks_for_facts(session: &Session, open: &OpenTemplate) -> bool {
    let failed = matches!(
        session.forms.build.fact_read.read().state,
        FactReadState::Failed(_)
    );
    session.rpc().is_err() || failed || !open.typed_facts.is_empty()
}

/// Ask the node for `needs`, once the answers have been still for
/// [`FACT_READ_DELAY`]. Asking again for what is being read, or was just
/// refused, does nothing; a reply to an older question is dropped. Offline
/// it does nothing: the form asks for the facts instead.
pub fn request_facts(session: Session, needs: Vec<Fact>) {
    let form = session.forms.build;
    let mut fact_read = form.fact_read;
    let Some(opened) = form.template.peek().as_ref().map(|open| open.opened) else {
        return;
    };
    let current = fact_read.peek().clone();
    if needs.is_empty() {
        if current.state != FactReadState::Idle {
            fact_read.set(FactRead {
                round: current.round + 1,
                ..FactRead::default()
            });
        }
        return;
    }
    if current.needs == needs && current.state != FactReadState::Idle {
        return;
    }
    let Ok(rpc) = session.rpc() else {
        return;
    };
    let round = current.round + 1;
    fact_read.set(FactRead {
        round,
        needs: needs.clone(),
        state: FactReadState::Reading,
    });
    // In the app's scope, so leaving Build does not leave it reading.
    spawn_app_task(async move {
        crate::rt::sleep(FACT_READ_DELAY).await;
        if fact_read.peek().round != round {
            return;
        }
        let read = wallet_flows::template_facts(rpc.as_ref(), &needs).await;
        let still_open = form
            .template
            .peek()
            .as_ref()
            .is_some_and(|open| open.opened == opened);
        if fact_read.peek().round != round || !still_open {
            return;
        }
        match read {
            Ok(facts) => {
                change(&session, |open| open.read_facts.merge(facts));
                fact_read.set(FactRead {
                    round,
                    needs,
                    state: FactReadState::Idle,
                });
            }
            Err(err) => fact_read.set(FactRead {
                round,
                needs,
                state: FactReadState::Failed(super::sentence(err)),
            }),
        }
    });
}

/// "from the template “Create a token”": what Sign's note says a
/// transaction was compiled from. The template's name is the only thing of
/// it that reaches Review and Sign.
pub fn template_source_note(name: &str) -> String {
    format!(" from the template “{name}”")
}

/// Write the template's blocks onto Build as ordinary blocks, and leave the
/// template. Blank answers leave blank fields. An answer a function cannot
/// take stops it, named and marked as Review does: the field it feeds would
/// be blank, and a blank optional field means none. There is no way back:
/// Clear starts over.
pub fn show_as_blocks(session: &Session) {
    let Some(OpenExpansion {
        open, expansion, ..
    }) = expand_open(session)
    else {
        return;
    };
    let form = session.forms.build;
    let mut error = form.error;
    let expansion = match expansion {
        Ok(expansion) => expansion,
        Err(problems) => {
            error.set(Some(first_problem(&problems)));
            return;
        }
    };
    let refused: Vec<Missing> = expansion
        .missing
        .iter()
        .filter(|item| matches!(item, Missing::Invalid { .. }))
        .cloned()
        .collect();
    if !refused.is_empty() {
        let (words, inputs) = missing_words(&open, &refused);
        change(session, |open| open.marked = inputs);
        error.set(Some(format!("Fix {} first.", listed(&words))));
        return;
    }
    close_template(session);
    for block in expansion.blocks {
        form.add_instruction(None, IxBody::Idl(block.draft));
    }
    let first = form.drafts.peek().first().map(|draft| draft.id);
    let mut selected = form.selected;
    selected.set(first);
    let mut note = form.compiled_note;
    note.set(Some(format!(
        "These blocks come from the template “{}”. Changing them does not change the template.",
        open.template.name
    )));
}

/// What is still missing, in words, and the inputs among it.
fn missing_words(open: &OpenTemplate, missing: &[Missing]) -> (Vec<String>, BTreeSet<String>) {
    let mut words = Vec::new();
    let mut inputs = BTreeSet::new();
    for item in missing {
        match item {
            Missing::Input(name) => {
                inputs.insert(name.clone());
                let label = open
                    .template
                    .input(name)
                    .map(|input| input.label.clone())
                    .unwrap_or_else(|| name.clone());
                words.push(label);
            }
            Missing::Invalid { input, message } => {
                inputs.insert(input.clone());
                let label = open
                    .template
                    .input(input)
                    .map(|input| input.label.clone())
                    .unwrap_or_else(|| input.clone());
                words.push(format!("{label} ({message})"));
            }
            Missing::Wallet => words.push("a wallet to act as".to_string()),
            Missing::FeePayer => words.push("Fee payer".to_string()),
            // Made when the template opens: only a template changed under
            // the form could lack one.
            Missing::Keypair(name) => words.push(format!("a new keypair for {name}")),
        }
    }
    (words, inputs)
}

/// "A, B and C".
fn listed(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Expand the template and compile it as Build's own button does: same fee
/// payer, Advanced options, simulation and blockhash, and errors numbered
/// by instruction. A required input left blank stops it, named in the error
/// and marked on its field.
pub fn compile_template(session: Session, compiling: Signal<bool>) {
    let form = session.forms.build;
    let (mut error, mut note) = (form.error, form.compiled_note);
    error.set(None);
    note.set(None);
    let Some(OpenExpansion {
        open, expansion, ..
    }) = expand_open(&session)
    else {
        return;
    };
    let mut expansion = match expansion {
        Ok(expansion) => expansion,
        Err(problems) => {
            error.set(Some(first_problem(&problems)));
            return;
        }
    };
    let (words, inputs) = missing_words(&open, &expansion.missing);
    change(&session, |open| open.marked = inputs);
    if !words.is_empty() {
        error.set(Some(format!("Fill in {} first.", listed(&words))));
        return;
    }
    if !expansion.needs.is_empty() {
        let message = if session.rpc().is_ok()
            && !matches!(form.fact_read.peek().state, FactReadState::Failed(_))
        {
            "Still reading from the node. Try again in a moment.".to_string()
        } else {
            let facts: Vec<String> = expansion.needs.iter().map(|f| f.to_string()).collect();
            format!("Fill in {} first.", listed(&facts))
        };
        error.set(Some(message));
        return;
    }
    let mut drafts: Vec<_> = expansion
        .blocks
        .iter_mut()
        .map(|block| std::mem::replace(&mut block.draft, wallet_core::IdlDraft::empty()))
        .collect();
    match build_idl_drafts(&session, &mut drafts) {
        Ok(instructions) => compile_instructions(
            session,
            instructions,
            Some(open.template.name.to_string()),
            compiling,
        ),
        Err(msg) => error.set(Some(msg)),
    }
}

/// The program and instruction of a template's block, as its heading in the
/// form reads.
pub fn block_heading(session: &Session, draft: &wallet_core::IdlDraft) -> String {
    let program = match draft.program_id {
        None => "Token or Token-2022, not known yet".to_string(),
        Some(program_id) => match wallet_core::pinned_program(&program_id) {
            Some(pinned) => pinned.label.to_string(),
            None if !draft.program_name.is_empty() => draft.program_name.clone(),
            None => session
                .idls
                .read()
                .records()
                .iter()
                .find(|record| record.program_id == program_id)
                .map(|record| record.name.clone())
                .unwrap_or_else(|| "Program".to_string()),
        },
    };
    format!("{program} · {}", draft.instruction)
}

#[cfg(test)]
pub(crate) mod tests;
