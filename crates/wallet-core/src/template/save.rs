//! Making a template from Build's blocks, and the name its file is kept by.
//!
//! [`SavePlan::new`] reads the blocks: every account and argument their IDL
//! form asks for, what each holds, and what can become of it. A [`Choice`]
//! per field then says whether it keeps its value, is asked each time, or
//! names the wallet or the fee payer, and [`SavePlan::template`] writes the
//! template. What is left out is what the IDL fills in: an account it
//! derives and nobody overrode, an argument left blank for its default. An
//! address that is a keypair made on Build is a `new-keypair` value wherever
//! it appears; that is not a choice.
//!
//! The template it writes reads back from its file to itself and checks
//! against the IDLs it was made from. Opened with the values the blocks held
//! as its answers, it writes the same blocks.

use super::builtin::{BuiltinTemplate, BUILTIN_TEMPLATES};
use super::check::{check, field_kind, strip_option, InputKind, InputScale};
use super::expand::Answers;
use super::model::*;
use super::parse::{MAX_BLOCKS, MAX_INPUTS, MAX_PROGRAMS, MAX_VALUES};
use crate::idl::build::{idl_instruction_form, IdlAccountSource, IdlArgKind};
use crate::idl::catalog::IdlCatalog;
use crate::idl::draft::IdlDraft;
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

/// The longest name or label a template may hold, in characters.
const MAX_LABEL_CHARS: usize = 80;
/// The longest summary, in characters.
const MAX_SUMMARY_CHARS: usize = 300;
/// The longest name a template gives an input, a value or a program.
const MAX_NAME_CHARS: usize = 40;
/// The longest slug, in characters.
pub const MAX_SLUG_CHARS: usize = 48;

/// What becomes of one field when the blocks are saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Choice {
    /// Its value, as typed: in the field's unit for a scaled amount. A blank
    /// field is left out.
    Keep,
    /// A question of the template's, asked each time it is opened.
    Ask,
    /// `$wallet`: the wallet in use when the template is opened.
    Wallet,
    /// `$fee-payer`: Build's fee payer when the template is opened.
    FeePayer,
}

/// One field of one block: the block's place on Build, from 0, and the
/// account's or argument's name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldAt {
    pub block: usize,
    pub field: String,
}

impl FieldAt {
    pub fn new(block: usize, field: &str) -> Self {
        Self {
            block,
            field: field.to_string(),
        }
    }
}

/// A field whose fate the person does not choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locked {
    /// It holds a keypair made on Build: a new one is made each time.
    NewKeypair,
    /// It is blank and the program needs it: it is asked each time.
    MustAsk,
}

/// One account or argument of a block, as Build holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveField {
    pub at: FieldAt,
    /// An account, rather than an argument.
    pub account: bool,
    /// What Build holds, as typed. Empty when it is blank.
    pub text: String,
    /// The address it holds, for an account or a public key argument.
    pub address: Option<Pubkey>,
    /// The unit an amount is typed in, from the IDL.
    pub unit: Option<String>,
    /// The program does without it: it can be blank.
    pub optional: bool,
    /// What can become of it, [`Choice::Keep`] first. Empty when locked.
    pub offers: Vec<Choice>,
    pub locked: Option<Locked>,
    /// The widget an input bound to it takes.
    kind: InputKind,
}

impl SaveField {
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// What it becomes when no choice was made: kept, or asked when it is
    /// blank and needed.
    pub fn default_choice(&self) -> Choice {
        match self.locked {
            Some(Locked::MustAsk) => Choice::Ask,
            _ => Choice::Keep,
        }
    }
}

/// One block, with the fields its IDL form asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveBlock {
    pub program_id: Pubkey,
    /// The program's name in its IDL.
    pub program_name: String,
    pub instruction: String,
    pub fields: Vec<SaveField>,
}

/// One question the template will ask, and the fields it fills.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedInput {
    pub name: String,
    pub label: String,
    pub optional: bool,
    /// What the fields hold now: the answer that writes the blocks as they
    /// are.
    pub text: String,
    /// More than one when fields holding the same value are asked once.
    pub fields: Vec<FieldAt>,
}

/// Build's blocks, read for saving as a template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavePlan {
    pub blocks: Vec<SaveBlock>,
    /// The keypairs made on Build that the blocks hold, each once, in the
    /// order they first appear, with the name of the value that makes it.
    keypairs: Vec<(Pubkey, String)>,
    /// Each program, once, with its alias.
    aliases: Vec<(Pubkey, String)>,
}

/// A template made from blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub template: Template,
    /// The values the blocks held, as answers to its questions.
    pub answers: Answers,
    /// The keypair each `new-keypair` value stood for in the blocks.
    pub keypairs: BTreeMap<String, Pubkey>,
}

impl SavePlan {
    /// Reads `drafts`, Build's blocks in order. `keypairs` are the keys made
    /// on Build, `wallet` and `fee_payer` what `$wallet` and `$fee-payer`
    /// stand for now. Fails on a block that has no program or instruction
    /// yet, or whose program has no IDL in `idls`.
    pub fn new(
        drafts: &[IdlDraft],
        keypairs: &[Pubkey],
        wallet: Option<Pubkey>,
        fee_payer: Option<Pubkey>,
        idls: &dyn IdlCatalog,
    ) -> Result<Self, String> {
        if drafts.is_empty() {
            return Err("There are no blocks on Build to save.".to_string());
        }
        let mut blocks = Vec::new();
        for (index, draft) in drafts.iter().enumerate() {
            let number = index + 1;
            let Some(program_id) = draft.program_id else {
                return Err(format!("Block {number} has no program yet."));
            };
            if draft.instruction.is_empty() {
                return Err(format!("Block {number} has no instruction yet."));
            }
            let client = idls
                .program_client(&program_id)
                .ok_or_else(|| format!("Block {number}'s program has no IDL on this network."))?;
            let form = idl_instruction_form(&client, &draft.instruction, &draft.input())
                .map_err(|err| format!("Block {number}: {err}"))?;
            let mut fields = Vec::new();
            for account in &form.accounts {
                let manual = draft.is_manual(&account.name);
                if account.source == IdlAccountSource::Derived && !manual {
                    continue;
                }
                let address = draft.account(&account.name);
                let optional = account.optional || account.source == IdlAccountSource::Derived;
                fields.push(field(
                    FieldAt::new(index, &account.name),
                    true,
                    address.map(|a| a.to_string()).unwrap_or_default(),
                    address,
                    None,
                    optional,
                    InputKind::Address,
                    keypairs,
                    wallet,
                    fee_payer,
                ));
            }
            for argument in &form.arguments {
                let text = draft.argument(&argument.name).to_string();
                let blank = text.trim().is_empty();
                if blank && argument.blank_uses_default {
                    continue;
                }
                let address = (*strip_option(&argument.kind) == IdlArgKind::Pubkey)
                    .then(|| Pubkey::from_str(text.trim()).ok())
                    .flatten();
                let optional =
                    argument.blank_uses_default || matches!(argument.kind, IdlArgKind::Option(_));
                fields.push(field(
                    FieldAt::new(index, &argument.name),
                    false,
                    text,
                    address,
                    argument.scale.unit().map(str::to_string),
                    optional,
                    field_kind(number, argument),
                    keypairs,
                    wallet,
                    fee_payer,
                ));
            }
            blocks.push(SaveBlock {
                program_id,
                program_name: if draft.program_name.is_empty() {
                    record_name(idls, &program_id)
                } else {
                    draft.program_name.clone()
                },
                instruction: draft.instruction.clone(),
                fields,
            });
        }

        let mut used: BTreeSet<String> = BUILT_IN_NAMES.iter().map(|n| n.to_string()).collect();
        let mut aliases = Vec::new();
        for block in &blocks {
            if aliases.iter().any(|(id, _)| *id == block.program_id) {
                continue;
            }
            let alias = unique(&slug_name(&block.program_name, "program"), &mut used);
            aliases.push((block.program_id, alias));
        }
        let mut keys = Vec::new();
        for field in blocks.iter().flat_map(|block| &block.fields) {
            if field.locked != Some(Locked::NewKeypair) {
                continue;
            }
            let Some(address) = field.address else {
                continue;
            };
            if keys.iter().any(|(key, _)| *key == address) {
                continue;
            }
            let name = unique(&name_of(&field.at.field), &mut used);
            keys.push((address, name));
        }
        Ok(Self {
            blocks,
            keypairs: keys,
            aliases,
        })
    }

    pub fn field(&self, at: &FieldAt) -> Option<&SaveField> {
        self.blocks
            .get(at.block)?
            .fields
            .iter()
            .find(|field| field.at.field == at.field)
    }

    fn fields(&self) -> impl Iterator<Item = &SaveField> {
        self.blocks.iter().flat_map(|block| &block.fields)
    }

    /// What field `field` becomes under `choices`: its choice, or its
    /// default without one. A locked field's choice does not count.
    pub fn choice(&self, choices: &BTreeMap<FieldAt, Choice>, field: &SaveField) -> Choice {
        match field.locked {
            Some(Locked::NewKeypair) => Choice::Keep,
            _ => choices
                .get(&field.at)
                .copied()
                .unwrap_or(field.default_choice()),
        }
    }

    /// The questions the template asks under `choices`, in the order their
    /// first field comes. Fields asked that hold the same value and take the
    /// same widget are one question.
    pub fn inputs(&self, choices: &BTreeMap<FieldAt, Choice>) -> Vec<PlannedInput> {
        let mut used: BTreeSet<String> = BUILT_IN_NAMES.iter().map(|n| n.to_string()).collect();
        used.extend(self.aliases.iter().map(|(_, alias)| alias.clone()));
        used.extend(self.keypairs.iter().map(|(_, name)| name.clone()));
        let asked: Vec<&SaveField> = self
            .fields()
            .filter(|field| {
                field.locked != Some(Locked::NewKeypair)
                    && self.choice(choices, field) == Choice::Ask
            })
            .collect();

        // Amounts counted in the units of another argument come last: they
        // are one question only where that argument is bound the same way.
        let scaled_later = |field: &SaveField| {
            matches!(
                field.kind,
                InputKind::Argument {
                    scale: InputScale::FromArgument { .. },
                    ..
                }
            )
        };
        let mut inputs: Vec<PlannedInput> = Vec::new();
        let mut keys: Vec<Option<String>> = Vec::new();
        for pass in [false, true] {
            for field in asked.iter().filter(|field| scaled_later(field) == pass) {
                let key = (!field.is_blank())
                    .then(|| self.join_key(field, choices, &inputs))
                    .flatten();
                if let Some(at) = key
                    .as_ref()
                    .and_then(|key| keys.iter().position(|k| k.as_ref() == Some(key)))
                {
                    let input = &mut inputs[at];
                    input.fields.push(field.at.clone());
                    input.optional &= field.optional;
                    continue;
                }
                keys.push(key);
                inputs.push(PlannedInput {
                    name: unique(&name_of(&field.at.field), &mut used),
                    label: words(&field.at.field),
                    optional: field.optional,
                    text: field.text.clone(),
                    fields: vec![field.at.clone()],
                });
            }
        }
        inputs.sort_by_key(|input| {
            let first = &input.fields[0];
            let place = self.blocks[first.block]
                .fields
                .iter()
                .position(|field| field.at == *first)
                .unwrap_or(0);
            (first.block, place)
        });
        // Two questions with one label are told apart by their block.
        let mut labels = BTreeSet::new();
        for input in &mut inputs {
            if !labels.insert(input.label.clone()) {
                input.label = format!("{} (block {})", input.label, input.fields[0].block + 1);
                labels.insert(input.label.clone());
            }
        }
        inputs
    }

    /// What two asked fields must share to be one question: the value, the
    /// widget, and for an amount counted in another argument's units, how
    /// that argument is bound. `None` when that argument is not bound yet.
    fn join_key(
        &self,
        field: &SaveField,
        choices: &BTreeMap<FieldAt, Choice>,
        inputs: &[PlannedInput],
    ) -> Option<String> {
        let value = field.text.trim();
        match &field.kind {
            InputKind::Argument {
                kind,
                scale: InputScale::FromArgument { argument, unit, .. },
            } => {
                let at = FieldAt::new(field.at.block, argument);
                let other = self.field(&at)?;
                let bound = match self.choice(choices, other) {
                    Choice::Ask => {
                        let input = inputs.iter().find(|input| input.fields.contains(&at))?;
                        format!("${}", input.name)
                    }
                    _ => self.term(other, choices, inputs)?.text(),
                };
                Some(format!("{value}\n{kind:?} scaled by {bound} {unit:?}"))
            }
            kind => Some(format!("{value}\n{kind:?}")),
        }
    }

    /// What the template binds `field` to under `choices`, `None` when it
    /// is left out.
    fn term(
        &self,
        field: &SaveField,
        choices: &BTreeMap<FieldAt, Choice>,
        inputs: &[PlannedInput],
    ) -> Option<Term> {
        if field.locked == Some(Locked::NewKeypair) {
            let address = field.address?;
            let (_, name) = self.keypairs.iter().find(|(key, _)| *key == address)?;
            return Some(Term::Ref(name.clone()));
        }
        match self.choice(choices, field) {
            Choice::Keep if field.is_blank() => None,
            Choice::Keep => Some(Term::Literal(field.text.clone())),
            Choice::Ask => inputs
                .iter()
                .find(|input| input.fields.contains(&field.at))
                .map(|input| Term::Ref(input.name.clone())),
            Choice::Wallet => Some(Term::Ref(WALLET.to_string())),
            Choice::FeePayer => Some(Term::Ref(FEE_PAYER.to_string())),
        }
    }

    /// The template these blocks make under `choices`, with `name` and
    /// `summary`, checked against `idls`. Refused, with every reason, when
    /// a choice does not fit its field, a blank field the program needs is
    /// not asked, or the result breaks a rule of the format or the IDLs.
    pub fn template(
        &self,
        name: &str,
        summary: &str,
        choices: &BTreeMap<FieldAt, Choice>,
        idls: &dyn IdlCatalog,
    ) -> Result<Saved, Vec<String>> {
        let mut reasons = Vec::new();
        let name = name.trim();
        if name.is_empty() {
            reasons.push("Give the template a name.".to_string());
        } else if name.chars().count() > MAX_LABEL_CHARS {
            reasons.push(format!(
                "The name is longer than {MAX_LABEL_CHARS} characters."
            ));
        }
        let summary = summary.trim();
        if summary.chars().count() > MAX_SUMMARY_CHARS {
            reasons.push(format!(
                "The summary is longer than {MAX_SUMMARY_CHARS} characters."
            ));
        }
        if self.blocks.len() > MAX_BLOCKS {
            reasons.push(format!(
                "A template holds at most {MAX_BLOCKS} blocks, and Build has {}.",
                self.blocks.len()
            ));
        }
        for field in self.fields() {
            let place = || {
                let block = &self.blocks[field.at.block];
                format!(
                    "Block {} ({}), {}",
                    field.at.block + 1,
                    block.instruction,
                    field.at.field
                )
            };
            if field.locked == Some(Locked::NewKeypair) {
                continue;
            }
            let choice = self.choice(choices, field);
            if field.locked == Some(Locked::MustAsk) {
                if choice != Choice::Ask {
                    reasons.push(format!(
                        "{}: it is blank, and the program needs it. Ask for it each time, or fill it in on Build.",
                        place()
                    ));
                }
            } else if choice != Choice::Keep && !field.offers.contains(&choice) {
                let what = match choice {
                    Choice::Wallet => "the wallet in use",
                    Choice::FeePayer => "the fee payer",
                    _ => "that",
                };
                reasons.push(format!("{}: it does not hold {what}.", place()));
            }
        }

        let inputs = self.inputs(choices);
        if inputs.len() > MAX_INPUTS {
            reasons.push(format!(
                "A template asks at most {MAX_INPUTS} questions, and this one would ask {}.",
                inputs.len()
            ));
        }
        if self.keypairs.len() > MAX_VALUES {
            reasons.push(format!(
                "A template makes at most {MAX_VALUES} keypairs, and these blocks hold {}.",
                self.keypairs.len()
            ));
        }
        if self.aliases.len() > MAX_PROGRAMS {
            reasons.push(format!(
                "A template names at most {MAX_PROGRAMS} programs, and these blocks use {}.",
                self.aliases.len()
            ));
        }
        if !reasons.is_empty() {
            return Err(reasons);
        }

        let mut blocks = Vec::new();
        for block in &self.blocks {
            let alias = self
                .aliases
                .iter()
                .find(|(id, _)| *id == block.program_id)
                .map(|(_, alias)| alias.clone())
                .unwrap_or_default();
            let mut accounts = BTreeMap::new();
            let mut arguments = BTreeMap::new();
            for field in &block.fields {
                let Some(term) = self.term(field, choices, &inputs) else {
                    continue;
                };
                let into = if field.account {
                    &mut accounts
                } else {
                    &mut arguments
                };
                into.insert(field.at.field.clone(), term);
            }
            blocks.push(Block {
                when: None,
                program: alias,
                instruction: block.instruction.clone(),
                accounts,
                arguments,
            });
        }
        let template = Template {
            name: name.to_string(),
            summary: (!summary.is_empty()).then(|| summary.to_string()),
            programs: self
                .aliases
                .iter()
                .map(|(id, alias)| (alias.clone(), *id))
                .collect(),
            options: Vec::new(),
            inputs: inputs
                .iter()
                .map(|input| Input {
                    name: input.name.clone(),
                    label: input.label.clone(),
                    hint: None,
                    optional: input.optional,
                    default: None,
                    option: None,
                })
                .collect(),
            values: self
                .keypairs
                .iter()
                .map(|(_, name)| {
                    (
                        name.clone(),
                        Call {
                            function: Function::NewKeypair,
                            args: Vec::new(),
                        },
                    )
                })
                .collect(),
            blocks,
        };

        match Template::parse(&template.to_toml()) {
            Ok(read) if read == template => {}
            Ok(_) => {
                return Err(vec![
                    "The template would not read back as it was written. Nothing was saved."
                        .to_string(),
                ])
            }
            Err(err) => return Err(vec![err.to_string()]),
        }
        if let Err(problems) = check(&template, idls) {
            return Err(problems.iter().map(ToString::to_string).collect());
        }
        let mut answers = Answers::default();
        for input in &inputs {
            answers
                .inputs
                .insert(input.name.clone(), input.text.clone());
        }
        Ok(Saved {
            template,
            answers,
            keypairs: self
                .keypairs
                .iter()
                .map(|(key, name)| (name.clone(), *key))
                .collect(),
        })
    }
}

/// One field, with what can become of it.
#[allow(clippy::too_many_arguments)]
fn field(
    at: FieldAt,
    account: bool,
    text: String,
    address: Option<Pubkey>,
    unit: Option<String>,
    optional: bool,
    kind: InputKind,
    keypairs: &[Pubkey],
    wallet: Option<Pubkey>,
    fee_payer: Option<Pubkey>,
) -> SaveField {
    let blank = text.trim().is_empty();
    let locked = if address.is_some_and(|address| keypairs.contains(&address)) {
        Some(Locked::NewKeypair)
    } else if blank && !optional {
        Some(Locked::MustAsk)
    } else {
        None
    };
    let mut offers = Vec::new();
    if locked.is_none() {
        offers.push(Choice::Keep);
        offers.push(Choice::Ask);
        if address.is_some() && address == wallet {
            offers.push(Choice::Wallet);
        }
        if address.is_some() && address == fee_payer {
            offers.push(Choice::FeePayer);
        }
    }
    SaveField {
        at,
        account,
        text,
        address,
        unit,
        optional,
        offers,
        locked,
        kind,
    }
}

fn record_name(idls: &dyn IdlCatalog, program_id: &Pubkey) -> String {
    idls.records()
        .iter()
        .find(|record| record.program_id == *program_id)
        .map(|record| record.name.clone())
        .unwrap_or_default()
}

/// `name`, or `name` and a number, whichever is not in `used` yet. Takes it.
fn unique(name: &str, used: &mut BTreeSet<String>) -> String {
    let mut candidate = name.to_string();
    let mut n = 2;
    while used.contains(&candidate) {
        candidate = format!("{name}_{n}");
        n += 1;
    }
    used.insert(candidate.clone());
    candidate
}

/// A field's name as a name in a template: a letter first, then letters,
/// digits, `_` and `-`.
fn name_of(field: &str) -> String {
    let mut name: String = field
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_NAME_CHARS)
        .collect();
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name.insert(0, 'f');
        name.truncate(MAX_NAME_CHARS);
    }
    name
}

/// A program's name as an alias: lower case, words joined by `_`.
fn slug_name(text: &str, fallback: &str) -> String {
    let mut out = String::new();
    for word in split_words(text) {
        if !out.is_empty() {
            out.push('_');
        }
        out.push_str(&word);
    }
    out.truncate(MAX_NAME_CHARS);
    let out = out.trim_end_matches('_').to_string();
    if out.starts_with(|c: char| c.is_ascii_alphabetic()) {
        out
    } else if out.is_empty() {
        fallback.to_string()
    } else {
        name_of(&format!("{fallback}_{out}"))
    }
}

/// The words of a name in camelCase, snake_case or kebab-case, lower case.
fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut previous: Option<char> = None;
    for c in text.chars() {
        if !c.is_ascii_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            previous = None;
            continue;
        }
        let starts = c.is_ascii_uppercase()
            && previous.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit());
        if starts && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.push(c.to_ascii_lowercase());
        previous = Some(c);
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// A field's name in words: "mintAuthority" reads "Mint authority".
pub fn words(field: &str) -> String {
    let mut text = split_words(field).join(" ");
    if let Some(first) = text.get(..1) {
        text = first.to_ascii_uppercase() + &text[1..];
    }
    if text.is_empty() {
        text = field.to_string();
    }
    text.chars().take(MAX_LABEL_CHARS).collect()
}

/// Names Windows keeps for devices, which no file may take.
const DEVICE_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The name a template of the user's is kept by: its name in lower case
/// letters and digits joined by `-`, at most [`MAX_SLUG_CHARS`] long. Never
/// empty, never a built-in template's id, never a path or a device name.
pub fn template_slug(name: &str) -> String {
    let mut slug = plain_slug(name);
    let taken = |slug: &str| {
        BUILTIN_TEMPLATES.iter().any(|builtin| builtin.id == slug) || DEVICE_NAMES.contains(&slug)
    };
    if taken(&slug) {
        slug.truncate(MAX_SLUG_CHARS - 6);
        slug = format!("{}-yours", slug.trim_end_matches('-'));
    }
    slug
}

/// `name` in lower case letters and digits joined by `-`, cut short.
fn plain_slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(MAX_SLUG_CHARS);
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "template".to_string()
    } else {
        slug
    }
}

/// The built-in template `name` would pass for: one with that name, or
/// whose id it is, whatever the case. A template of the user's may not.
pub fn builtin_named(name: &str) -> Option<&'static BuiltinTemplate> {
    let name = name.trim();
    let slug = plain_slug(name);
    BUILTIN_TEMPLATES.iter().find(|builtin| {
        builtin.id == slug
            || builtin
                .template()
                .is_ok_and(|template| template.name.trim().eq_ignore_ascii_case(name))
    })
}

/// Whether `text` is a name [`template_slug`] gives.
pub fn is_template_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_SLUG_CHARS
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !text.starts_with('-')
        && !text.ends_with('-')
        && !text.contains("--")
        && template_slug(text) == text
}
