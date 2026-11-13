//! The Templates dialog on Build: a list of every template and a reader for
//! the one picked, as Programs is a list of IDLs and a reader for one.
//!
//! The reader is read from a template and its text, never from where it is
//! kept, so a template read from text that is not kept yet reads the same
//! way ([`template_view`]). Its tabs are [`ReaderQuestions`], in the order
//! the form asks them, and [`ReaderBlock`]s, every block in file order with
//! every field it sets and where the value comes from: nothing a template
//! writes is left out.

use super::kept_templates::{name_words, term_words, FactsFrom};
use super::template::TemplateEntry;
use crate::state::session::Session;
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Arc;
use wallet_core::template::{
    check, FormInput, InputKind, InputScale, TemplateForm, Term, FEE_PAYER, WALLET,
};
use wallet_core::{IdlArgKind, Template, TOKEN_PROGRAM_ID};

/// Whether a row of the list answers the filter: its name or its summary
/// holds the text, whatever the case.
pub fn template_matches(entry: &TemplateEntry, filter: &str) -> bool {
    let wanted = filter.trim().to_lowercase();
    wanted.is_empty()
        || [Some(entry.name.as_str()), entry.summary.as_deref()]
            .into_iter()
            .flatten()
            .any(|text| text.to_lowercase().contains(&wanted))
}

/// The row opened when the dialog opens on a wide window: the first that
/// can be used on this network, or else the first.
pub fn first_usable(entries: &[TemplateEntry]) -> Option<&TemplateEntry> {
    entries
        .iter()
        .find(|entry| entry.problem.is_none())
        .or_else(|| entries.first())
}

/// One template as the reader shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateView {
    pub template: Arc<Template>,
    /// Its file, as kept: what the Text tab shows and Copy copies.
    pub text: Option<String>,
    /// Every problem `check` finds on the network in use, in words. Empty
    /// when it can be used here.
    pub problems: Vec<String>,
    pub questions: ReaderQuestions,
    pub blocks: Vec<ReaderBlock>,
}

/// Read `template`, whose file is `text`, for the reader: checked against
/// the IDLs of the network in use, its questions with the widget each
/// takes, and its blocks with every field they set.
pub fn template_view(
    session: &Session,
    template: Arc<Template>,
    text: Option<String>,
) -> TemplateView {
    let idls = session.idls.read();
    let checked = check(&template, &*idls);
    let problems = match &checked {
        Ok(_) => Vec::new(),
        Err(problems) => problems.iter().map(super::sentence).collect(),
    };
    let questions = reader_questions(&template, checked.as_ref().ok());
    let blocks = reader_blocks(&*idls, &template, FactsFrom::of(session));
    drop(idls);
    TemplateView {
        template,
        text,
        problems,
        questions,
        blocks,
    }
}

/// One question, as the Questions tab lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderQuestion {
    pub name: String,
    pub label: String,
    pub optional: bool,
    pub hint: Option<String>,
    /// What it holds until it is answered, in words: "the wallet in use".
    pub until: Option<String>,
    /// The widget it takes, in words: "an address", "a number in SOL". `None`
    /// when the template does not fit this network, so its fields are not
    /// known.
    pub takes: Option<String>,
}

/// One option, with the questions asked only while it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderOption {
    pub name: String,
    pub label: String,
    pub hint: Option<String>,
    /// On when the form opens.
    pub on_at_first: bool,
    pub questions: Vec<ReaderQuestion>,
    /// Why this network does not offer it, when it does not.
    pub not_here: Option<String>,
}

/// Every question of a template, in the order its form asks them: those
/// asked always, then each option with those asked under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReaderQuestions {
    pub always: Vec<ReaderQuestion>,
    pub options: Vec<ReaderOption>,
}

impl ReaderQuestions {
    /// How many questions there are, under options or not.
    pub fn count(&self) -> usize {
        self.always.len()
            + self
                .options
                .iter()
                .map(|option| option.questions.len())
                .sum::<usize>()
    }
}

/// The questions of `template`. `form` is what `check` gave, when it fits
/// the network in use, for the widget each question takes.
pub fn reader_questions(template: &Template, form: Option<&TemplateForm>) -> ReaderQuestions {
    let question = |input: &wallet_core::template::Input| ReaderQuestion {
        name: input.name.clone(),
        label: input.label.clone(),
        optional: input.optional,
        hint: input.hint.clone(),
        until: input
            .default
            .as_ref()
            // A default is a literal, a built-in name or a program, never a
            // chain fact.
            .map(|default| term_words(template, default, FactsFrom::Node)),
        takes: form
            .and_then(|form| form.inputs.iter().find(|asked| asked.name == input.name))
            .and_then(takes_words),
    };
    ReaderQuestions {
        always: template
            .inputs
            .iter()
            .filter(|input| input.option.is_none())
            .map(question)
            .collect(),
        options: template
            .options
            .iter()
            .map(|option| ReaderOption {
                name: option.name.clone(),
                label: option.label.clone(),
                hint: option.hint.clone(),
                on_at_first: option.default,
                questions: template
                    .inputs
                    .iter()
                    .filter(|input| input.option.as_deref() == Some(option.name.as_str()))
                    .map(question)
                    .collect(),
                not_here: form.and_then(|form| {
                    form.unavailable
                        .iter()
                        .find(|(name, _)| *name == option.name)
                        .map(|(_, why)| why.clone())
                }),
            })
            .collect(),
    }
}

/// The widget a question takes, in a few words.
fn takes_words(input: &FormInput) -> Option<String> {
    match &input.kind {
        InputKind::Address => Some("an address".into()),
        InputKind::Argument { kind, scale } => kind_words(kind, scale),
    }
}

fn kind_words(kind: &IdlArgKind, scale: &InputScale) -> Option<String> {
    Some(match kind {
        IdlArgKind::Option(inner) => return kind_words(inner, scale),
        IdlArgKind::Pubkey => "an address".into(),
        IdlArgKind::Bool => "yes or no".into(),
        IdlArgKind::String => "text".into(),
        IdlArgKind::Bytes => "bytes, in hex".into(),
        IdlArgKind::List(item) => match **item {
            IdlArgKind::String => "a list of text, as JSON".into(),
            IdlArgKind::Pubkey => "a list of addresses, as JSON".into(),
            IdlArgKind::Bool => "a list of yes or no, as JSON".into(),
            IdlArgKind::Struct { .. } => "a list of JSON objects".into(),
            IdlArgKind::Variants { .. } => "a list of choices, as JSON".into(),
            _ => "a list of whole numbers, as JSON".into(),
        },
        IdlArgKind::FixedBytes { len } => format!("text of up to {len} bytes, or hex after 0x"),
        IdlArgKind::Struct { fields } => format!(
            "a JSON object of {}",
            listed(
                &fields
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>(),
                "and"
            )
        ),
        IdlArgKind::Variants { variants } => format!(
            "a choice of {}, as JSON where it holds values",
            listed(
                &variants
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>(),
                "or"
            )
        ),
        IdlArgKind::Enum { variants } if variants.len() > 6 => format!(
            "a choice of {}, from {} to {}",
            variants.len(),
            variants.first()?,
            variants.last()?
        ),
        IdlArgKind::Enum { variants } => format!("a choice of {}", listed(variants, "or")),
        IdlArgKind::Integer { float: true, .. } => "a number".into(),
        IdlArgKind::Integer { .. } => match scale {
            InputScale::None => "a whole number, in base units".into(),
            InputScale::Fixed {
                unit: Some(unit), ..
            } if unit == "%" => "a percentage".into(),
            InputScale::Fixed {
                unit: Some(unit), ..
            } => format!("a number in {unit}"),
            InputScale::Fixed { decimals: 0, .. } => "a whole number".into(),
            InputScale::Fixed { decimals: 1, .. } => "a number with up to 1 decimal place".into(),
            InputScale::Fixed { decimals, .. } => {
                format!("a number with up to {decimals} decimal places")
            }
            InputScale::FromArgument { argument, .. } => {
                format!("a number with as many decimal places as {argument}")
            }
            InputScale::FromValue { value, .. } => {
                format!("a number with as many decimal places as ${value}")
            }
        },
        IdlArgKind::Raw { .. } => return None,
    })
}

/// "A, B or C".
fn listed(words: &[String], last: &str) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., end] => format!("{} {last} {end}", rest.join(", ")),
    }
}

/// One block of a template as the Blocks tab shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderBlock {
    /// Its place in the file, from 1.
    pub number: usize,
    /// `None` while it is the token program of a mint not known yet.
    pub program_id: Option<Pubkey>,
    /// The program's name, as the app knows it.
    pub program: String,
    pub instruction: String,
    /// The label of the option it runs under.
    pub when: Option<String>,
    /// Every account it sets, in the IDL's order where the IDL is here.
    pub accounts: Vec<ReaderField>,
    /// Every argument it sets, the same way.
    pub arguments: Vec<ReaderField>,
}

impl ReaderBlock {
    /// Every field it sets: its accounts, then its arguments.
    pub fn fields(&self) -> impl Iterator<Item = &ReaderField> {
        self.accounts.iter().chain(&self.arguments)
    }
}

/// One field a block sets, and where its value comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderField {
    pub field: String,
    pub origin: FieldOrigin,
    /// The unit the IDL gives an argument: "SOL".
    pub unit: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldOrigin {
    /// The answer to a question.
    Asked { input: String, label: String },
    /// A literal, or the address of a program alias, as written.
    Fixed {
        text: String,
        /// The literal read as an address, when it is one.
        address: Option<Pubkey>,
        /// The alias it was written as.
        alias: Option<String>,
    },
    /// Worked out when the form is filled: a value, the wallet in use or the
    /// fee payer, in words.
    Worked {
        /// The value it was written as, `mint` for `$mint`.
        value: Option<String>,
        words: String,
    },
}

/// Every block of `template`, read from the file itself, so a block under
/// an option and a template that does not fit this network are shown too.
/// A chain fact says where it comes from, `facts`.
pub fn reader_blocks(
    idls: &dyn wallet_core::IdlCatalog,
    template: &Template,
    facts: FactsFrom,
) -> Vec<ReaderBlock> {
    let program_name = |program_id: &Pubkey| match wallet_core::pinned_program(program_id) {
        Some(pinned) => pinned.label.to_string(),
        None => idls
            .records()
            .iter()
            .find(|record| record.program_id == *program_id)
            .map(|record| record.name.clone())
            .unwrap_or_else(|| "A program with no IDL here".to_string()),
    };
    template
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let program_id = template.programs.get(&block.program).copied();
            let program = match &program_id {
                Some(id) => program_name(id),
                None => format!(
                    "The token program of the mint {}",
                    template
                        .values
                        .get(&block.program)
                        .and_then(|call| call.args.first())
                        .map(|arg| name_words(template, arg))
                        .unwrap_or_default()
                ),
            };
            // A block on the token program of a mint is laid out as Token's,
            // as checking and expanding lay it out.
            let form = idls
                .program_client(&program_id.unwrap_or(TOKEN_PROGRAM_ID))
                .and_then(|client| {
                    wallet_core::idl_instruction_form(
                        &client,
                        &block.instruction,
                        &Default::default(),
                    )
                    .ok()
                });
            let account_order: Vec<String> = form
                .as_ref()
                .map(|form| form.accounts.iter().map(|a| a.name.clone()).collect())
                .unwrap_or_default();
            let argument_order: Vec<String> = form
                .as_ref()
                .map(|form| form.arguments.iter().map(|a| a.name.clone()).collect())
                .unwrap_or_default();
            let unit = |field: &str| {
                form.as_ref()?
                    .arguments
                    .iter()
                    .find(|argument| argument.name == field)?
                    .scale
                    .unit()
                    .map(str::to_string)
            };
            let fields = |bound: &std::collections::BTreeMap<String, Term>,
                          order: &[String],
                          units: bool| {
                let mut rows: Vec<ReaderField> = bound
                    .iter()
                    .map(|(field, term)| ReaderField {
                        field: field.clone(),
                        origin: origin(template, term, facts),
                        unit: if units { unit(field) } else { None },
                    })
                    .collect();
                // The IDL's order; a field it does not name goes last, as
                // the file sorts it.
                rows.sort_by_key(|row| {
                    order
                        .iter()
                        .position(|name| *name == row.field)
                        .unwrap_or(usize::MAX)
                });
                rows
            };
            ReaderBlock {
                number: index + 1,
                program_id,
                program,
                instruction: block.instruction.clone(),
                when: block.when.as_ref().map(|name| {
                    template
                        .option(name)
                        .map_or(name.clone(), |option| option.label.clone())
                }),
                accounts: fields(&block.accounts, &account_order, false),
                arguments: fields(&block.arguments, &argument_order, true),
            }
        })
        .collect()
}

/// Where the value `term` gives a field comes from.
fn origin(template: &Template, term: &Term, facts: FactsFrom) -> FieldOrigin {
    match term {
        Term::Literal(text) => FieldOrigin::Fixed {
            address: Pubkey::from_str(text.trim()).ok(),
            text: text.clone(),
            alias: None,
        },
        Term::Ref(name) => {
            if let Some(input) = template.input(name) {
                return FieldOrigin::Asked {
                    input: name.clone(),
                    label: input.label.clone(),
                };
            }
            if let Some(address) = template.programs.get(name) {
                return FieldOrigin::Fixed {
                    text: address.to_string(),
                    address: Some(*address),
                    alias: Some(name.clone()),
                };
            }
            FieldOrigin::Worked {
                value: (name != WALLET && name != FEE_PAYER).then(|| name.clone()),
                words: term_words(template, term, facts),
            }
        }
    }
}

#[cfg(test)]
mod tests;
