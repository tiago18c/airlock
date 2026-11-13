//! Checking a template against the IDLs of the network in use, and the form
//! it asks for.
//!
//! Every block's program needs an IDL, its instruction and every field it
//! binds must be in that IDL, and every field the program needs is bound or
//! left to a default the IDL fills. An input takes the kind of the fields it
//! is bound to, so they must agree. A block whose program is
//! `token-program-of` is checked against Token and Token-2022 both.

use super::model::*;
use super::size::{sizing, Sizing};
use crate::idl::build::{
    idl_instruction_form, parse_field, IdlAccountSource, IdlArgField, IdlArgKind, IdlArgScale,
    IdlBuildInput, IdlForm,
};
use crate::idl::catalog::IdlCatalog;
use crate::ids::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use codama_dynamic_client::ProgramClient;
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;
use std::sync::Arc;

/// Something about a template the IDLs do not allow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The block, numbered from 1 in the order written.
    pub block: Option<usize>,
    pub instruction: Option<String>,
    /// The account or argument of that instruction.
    pub field: Option<String>,
    pub message: String,
    /// The program this network has no IDL for, when that is the problem.
    pub missing_program: Option<Pubkey>,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.block, &self.instruction, &self.field) {
            (Some(block), Some(instruction), Some(field)) => {
                write!(f, "Block {block} ({instruction}), {field}: ")?
            }
            (Some(block), Some(instruction), None) => write!(f, "Block {block} ({instruction}): ")?,
            (Some(block), None, _) => write!(f, "Block {block}: ")?,
            _ => {}
        }
        f.write_str(&self.message)
    }
}

/// What the form of a checked template shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateForm {
    /// The questions, in the order written.
    pub inputs: Vec<FormInput>,
    /// The checkboxes this network offers, in the order written.
    pub options: Vec<TemplateOption>,
    /// The options this network does not offer, by name, each with why: a
    /// block under it needs a program this network has no IDL for.
    pub unavailable: Vec<(String, String)>,
}

/// One question, with the widget its fields call for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormInput {
    pub name: String,
    pub label: String,
    pub hint: Option<String>,
    pub optional: bool,
    pub default: Option<Term>,
    /// Asked only while this option is on.
    pub option: Option<String>,
    pub kind: InputKind,
}

/// The widget an input takes from the fields it is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputKind {
    /// An account, or a public key argument.
    Address,
    /// Any other argument. `kind` is never an option: an optional field
    /// left blank is an optional input left blank.
    Argument { kind: IdlArgKind, scale: InputScale },
}

/// How an input's number is typed, from the scale of its fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputScale {
    /// In base units.
    None,
    Fixed {
        decimals: u8,
        unit: Option<String>,
    },
    /// With as many decimal places as argument `argument` of block `block`
    /// (numbered from 1) holds, once that is known: see
    /// [`super::Expansion::decimals_of`].
    FromArgument {
        block: usize,
        argument: String,
        unit: Option<String>,
    },
    /// With as many decimal places as the value `value` holds once it is
    /// worked out: an input given to `base-units` with a value as its
    /// decimals. See [`super::Expansion::decimals_of`].
    FromValue {
        value: String,
        unit: Option<String>,
    },
    /// With as many decimal places as the input `input` holds.
    FromInput {
        input: String,
        unit: Option<String>,
    },
}

/// The IDL a block is built with, for one program it can run on.
#[derive(Clone)]
pub(crate) struct BlockIdl {
    pub program_id: Pubkey,
    pub program_name: String,
    pub client: Arc<ProgramClient>,
    pub form: IdlForm,
}

/// A checked template: its form, and each block's IDL for each program it
/// can run on (Token first when it is `token-program-of`).
pub(crate) struct Checked {
    pub form: TemplateForm,
    pub blocks: Vec<Vec<BlockIdl>>,
}

/// Checks `template` against `idls`, the catalog of the network in use, and
/// returns the form it asks for. Every problem found is listed.
pub fn check(template: &Template, idls: &dyn IdlCatalog) -> Result<TemplateForm, Vec<Problem>> {
    analyze(template, idls).map(|checked| checked.form)
}

/// The programs a block can run on.
pub(crate) fn candidates(template: &Template, block: &Block) -> Vec<Pubkey> {
    match template.programs.get(&block.program) {
        Some(address) => vec![*address],
        None => vec![TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID],
    }
}

fn lookup(idls: &dyn IdlCatalog, program_id: &Pubkey) -> Option<(String, Arc<ProgramClient>)> {
    let client = idls.program_client(program_id)?;
    let name = idls
        .records()
        .iter()
        .find(|record| record.program_id == *program_id)
        .map(|record| record.name.clone())
        .unwrap_or_default();
    Some((name, client))
}

/// The inputs a term reads, directly or through values.
pub(crate) fn inputs_behind<'a>(template: &'a Template, term: &'a Term) -> BTreeSet<&'a str> {
    let mut found = BTreeSet::new();
    let mut stack: Vec<&str> = term.reference().into_iter().collect();
    let mut seen = BTreeSet::new();
    while let Some(name) = stack.pop() {
        if !seen.insert(name) {
            continue;
        }
        if template.input(name).is_some() {
            found.insert(name);
        } else if let Some(call) = template.values.get(name) {
            stack.extend(call.args.iter().filter_map(Term::reference));
        }
    }
    found
}

/// What a reference holds, when the template says: everything but an input.
fn term_type(template: &Template, term: &Term) -> Option<ValueType> {
    let name = term.reference()?;
    if BUILT_IN_NAMES.contains(&name) {
        return Some(ValueType::Address);
    }
    if template.programs.contains_key(name) {
        return Some(ValueType::Program);
    }
    template
        .values
        .get(name)
        .map(|call| call.function.returns())
}

/// An option's kind is its item's: blank is how an option is left out.
pub(crate) fn strip_option(kind: &IdlArgKind) -> &IdlArgKind {
    match kind {
        IdlArgKind::Option(inner) => strip_option(inner),
        other => other,
    }
}

pub(crate) fn field_kind(block: usize, field: &IdlArgField) -> InputKind {
    let kind = strip_option(&field.kind);
    if *kind == IdlArgKind::Pubkey {
        return InputKind::Address;
    }
    let scale = match &field.scale {
        IdlArgScale::None => InputScale::None,
        IdlArgScale::Fixed { decimals, unit } => InputScale::Fixed {
            decimals: *decimals,
            unit: unit.clone(),
        },
        IdlArgScale::FromArgument { argument, unit } => InputScale::FromArgument {
            block,
            argument: argument.clone(),
            unit: unit.clone(),
        },
    };
    InputKind::Argument {
        kind: kind.clone(),
        scale,
    }
}

/// One field an input is bound to.
struct Seen {
    kind: InputKind,
    /// What two kinds are compared by: a scale from another argument is the
    /// same wherever that argument holds the same thing.
    key: String,
    place: String,
}

struct Pass<'a> {
    template: &'a Template,
    problems: Vec<Problem>,
    seen: BTreeMap<String, Vec<Seen>>,
}

impl Pass<'_> {
    fn problem(&mut self, block: usize, instruction: &str, field: Option<&str>, message: String) {
        let problem = Problem {
            block: Some(block),
            instruction: Some(instruction.to_string()),
            field: field.map(str::to_string),
            message,
            missing_program: None,
        };
        if !self.problems.contains(&problem) {
            self.problems.push(problem);
        }
    }

    fn observe(&mut self, number: usize, block: &Block, field: &str, term: &Term, kind: InputKind) {
        let Some(name) = term.reference() else {
            return;
        };
        if self.template.input(name).is_none() {
            return;
        }
        let key = match &kind {
            InputKind::Argument {
                kind: arg,
                scale: InputScale::FromArgument { argument, unit, .. },
            } => format!(
                "{arg:?} scaled by {:?} {unit:?}",
                block.arguments.get(argument)
            ),
            other => format!("{other:?}"),
        };
        let place = format!("block {number} ({}) {field}", block.instruction);
        let list = self.seen.entry(name.to_string()).or_default();
        if !list
            .iter()
            .any(|seen| seen.place == place && seen.key == key)
        {
            list.push(Seen { kind, key, place });
        }
    }

    /// A field the program needs cannot rest on an answer that may be
    /// blank: an optional input, or one asked only under another option.
    fn required(&mut self, number: usize, block: &Block, field: &str, term: &Term) {
        for name in inputs_behind(self.template, term) {
            let Some(input) = self.template.input(name) else {
                continue;
            };
            if input.optional {
                self.problem(
                    number,
                    &block.instruction,
                    Some(field),
                    format!("The program needs this field, but `${name}` is optional. Make the input required, or bind something else."),
                );
            }
            if let Some(option) = &input.option {
                if block.when.as_deref() != Some(option.as_str()) {
                    self.problem(
                        number,
                        &block.instruction,
                        Some(field),
                        format!("The program needs this field, but `${name}` is asked only while `{option}` is on, and this block runs without it."),
                    );
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn value_fits(
        &mut self,
        number: usize,
        block: &Block,
        field: &str,
        term: &Term,
        address: bool,
        integer: bool,
        json: Option<&IdlArgField>,
    ) {
        let Some(ty) = term_type(self.template, term) else {
            return;
        };
        let fits = match ty {
            ValueType::Address | ValueType::Program => address,
            ValueType::Integer => integer,
            ValueType::Amount | ValueType::Shape => false,
            ValueType::Json => json.is_some(),
        };
        if fits && ty == ValueType::Json {
            // What the function writes must read in this field.
            let function = term
                .reference()
                .and_then(|name| self.template.values.get(name))
                .map(|call| call.function);
            let sample = function.and_then(|function| function.json_for(&Pubkey::default()));
            if let (Some(field_kind), Some(sample)) = (json, sample) {
                let text: HashMap<String, String> = [(field_kind.name.clone(), sample)].into();
                if let Err(err) = parse_field(field_kind, &text) {
                    self.problem(
                        number,
                        &block.instruction,
                        Some(field),
                        format!(
                            "`{}` writes what this field does not take: {err}",
                            term.text()
                        ),
                    );
                }
            }
            return;
        }
        if !fits {
            let what = match ty {
                ValueType::Address => "an address",
                ValueType::Program => "a program",
                ValueType::Integer | ValueType::Amount => "a number",
                ValueType::Json => "JSON",
                ValueType::Shape => "a tree shape",
            };
            self.problem(
                number,
                &block.instruction,
                Some(field),
                format!(
                    "`{}` holds {what}, which this field does not take.",
                    term.text()
                ),
            );
        }
    }

    fn block(&mut self, number: usize, block: &Block, form: &IdlForm) {
        let instruction = block.instruction.as_str();
        for (name, term) in &block.accounts {
            let Some(field) = form.accounts.iter().find(|field| field.name == *name) else {
                let message = if form.arguments.iter().any(|field| field.name == *name) {
                    format!("`{name}` is an argument of {instruction}. Bind it under arguments.")
                } else {
                    format!("{instruction} has no account `{name}`.")
                };
                self.problem(number, instruction, Some(name), message);
                continue;
            };
            if let Term::Literal(text) = term {
                if Pubkey::from_str(text).is_err() {
                    self.problem(
                        number,
                        instruction,
                        Some(name),
                        format!("`{text}` is not an address."),
                    );
                }
            }
            self.value_fits(number, block, name, term, true, false, None);
            self.observe(number, block, name, term, InputKind::Address);
            if !field.optional && field.source != IdlAccountSource::Derived {
                self.required(number, block, name, term);
            }
        }

        for (name, term) in &block.arguments {
            let Some(field) = form.arguments.iter().find(|field| field.name == *name) else {
                let message = if form.accounts.iter().any(|field| field.name == *name) {
                    format!("`{name}` is an account of {instruction}. Bind it under accounts.")
                } else {
                    format!("{instruction} has no argument `{name}` to set.")
                };
                self.problem(number, instruction, Some(name), message);
                continue;
            };
            let kind = field_kind(number, field);
            if let IdlArgScale::FromArgument { argument, .. } = &field.scale {
                if !block.arguments.contains_key(argument) {
                    self.problem(
                        number,
                        instruction,
                        Some(name),
                        format!("This amount is counted in the units `{argument}` sets. Bind `{argument}` in this block too."),
                    );
                }
            } else if let Term::Literal(text) = term {
                let text: HashMap<String, String> = [(name.clone(), text.clone())].into();
                if let Err(err) = parse_field(field, &text) {
                    self.problem(number, instruction, Some(name), err);
                }
            }
            let (address, integer) = match &kind {
                InputKind::Address => (true, false),
                InputKind::Argument { kind, .. } => (
                    false,
                    matches!(kind, IdlArgKind::Integer { float: false, .. }),
                ),
            };
            let json = matches!(
                strip_option(&field.kind),
                IdlArgKind::List(_) | IdlArgKind::Struct { .. } | IdlArgKind::Variants { .. }
            )
            .then_some(field);
            self.value_fits(number, block, name, term, address, integer, json);
            self.observe(number, block, name, term, kind);
            if !field.may_be_blank() {
                self.required(number, block, name, term);
            }
        }

        for field in &form.accounts {
            if !field.optional
                && field.source != IdlAccountSource::Derived
                && !block.accounts.contains_key(&field.name)
            {
                self.problem(
                    number,
                    instruction,
                    Some(&field.name),
                    "The program needs this account and the IDL does not fill it. Bind it."
                        .to_string(),
                );
            }
        }
        for field in &form.arguments {
            if !field.may_be_blank() && !block.arguments.contains_key(&field.name) {
                self.problem(
                    number,
                    instruction,
                    Some(&field.name),
                    "The program needs this argument and the IDL has no default for it. Bind it."
                        .to_string(),
                );
            }
        }
    }
}

pub(crate) fn analyze(template: &Template, idls: &dyn IdlCatalog) -> Result<Checked, Vec<Problem>> {
    let mut pass = Pass {
        template,
        problems: Vec::new(),
        seen: BTreeMap::new(),
    };
    let mut blocks = Vec::new();
    let mut unavailable: Vec<(String, String)> = Vec::new();
    for (index, block) in template.blocks.iter().enumerate() {
        let number = index + 1;
        let mut idl_forms = Vec::new();
        for program_id in candidates(template, block) {
            let Some((program_name, client)) = lookup(idls, &program_id) else {
                let named = match crate::book::program_display_name(&program_id) {
                    Some(name) => format!("the {name}"),
                    None => format!("the program `${}`", block.program),
                };
                let message =
                    format!("Airlock has no IDL for {named} ({program_id}) on this network.");
                // An option whose block this network cannot build is not
                // offered here; the rest of the template is.
                if let Some(option) = &block.when {
                    if !unavailable.iter().any(|(name, _)| name == option) {
                        unavailable.push((option.clone(), message));
                    }
                    continue;
                }
                pass.problems.push(Problem {
                    block: Some(number),
                    instruction: Some(block.instruction.clone()),
                    field: None,
                    message,
                    missing_program: Some(program_id),
                });
                continue;
            };
            let Ok(form) =
                idl_instruction_form(&client, &block.instruction, &IdlBuildInput::default())
            else {
                let name = if program_name.is_empty() {
                    program_id.to_string()
                } else {
                    program_name.clone()
                };
                pass.problem(
                    number,
                    &block.instruction,
                    None,
                    format!(
                        "The IDL of {name} has no instruction `{}`.",
                        block.instruction
                    ),
                );
                continue;
            };
            pass.block(number, block, &form);
            idl_forms.push(BlockIdl {
                program_id,
                program_name,
                client,
                form,
            });
        }
        blocks.push(idl_forms);
    }
    sizes(&mut pass);

    let mut inputs = Vec::new();
    for input in &template.inputs {
        let seen = pass.seen.remove(&input.name).unwrap_or_default();
        let kind = match seen.split_first() {
            Some((first, rest)) => {
                if let Some(other) = rest.iter().find(|other| other.key != first.key) {
                    pass.problems.push(Problem {
                        block: None,
                        instruction: None,
                        field: None,
                        message: format!(
                            "`${}` is bound to fields that take different things: {} and {}.",
                            input.name, first.place, other.place
                        ),
                        missing_program: None,
                    });
                }
                first.kind.clone()
            }
            None => kind_from_parameters(template, &input.name),
        };
        if let Some(problem) = parameter_problem(template, &input.name, &kind) {
            pass.problems.push(problem);
        }
        inputs.push(FormInput {
            name: input.name.clone(),
            label: input.label.clone(),
            hint: input.hint.clone(),
            optional: input.optional,
            default: input.default.clone(),
            option: input.option.clone(),
            kind,
        });
    }

    if !pass.problems.is_empty() {
        return Err(pass.problems);
    }
    Ok(Checked {
        form: TemplateForm {
            inputs,
            options: template
                .options
                .iter()
                .filter(|option| !unavailable.iter().any(|(name, _)| *name == option.name))
                .cloned()
                .collect(),
            unavailable,
        },
        blocks,
    })
}

/// An input given to a function must hold what the function takes.
fn parameter_problem(template: &Template, input: &str, kind: &InputKind) -> Option<Problem> {
    for (value, call) in &template.values {
        for (arg, param) in call.args.iter().zip(call.function.params()) {
            if arg.reference() != Some(input) {
                continue;
            }
            let fits = match (param, kind) {
                (ValueType::Integer | ValueType::Amount, InputKind::Argument { kind, .. }) => {
                    matches!(kind, IdlArgKind::Integer { float: false, .. })
                }
                (ValueType::Integer | ValueType::Amount, InputKind::Address) => false,
                (ValueType::Shape, InputKind::Argument { kind, .. }) => {
                    *kind == super::tree::tree_shape_kind()
                }
                (ValueType::Shape | ValueType::Json, _) => false,
                (_, InputKind::Address) => true,
                (_, InputKind::Argument { .. }) => false,
            };
            if !fits {
                return Some(Problem {
                    block: None,
                    instruction: None,
                    field: None,
                    message: format!(
                        "`${input}` is given to {} in `{value}`, which takes {}, but its fields take something else.",
                        call.function.name(),
                        call.function.usage()
                    ),
                    missing_program: None,
                });
            }
        }
    }
    None
}

/// The kind of an input bound to no field: what the functions it is given
/// to take. An amount given to `base-units` is typed with the decimals it
/// is given with.
fn kind_from_parameters(template: &Template, input: &str) -> InputKind {
    let amount = template.values.values().find_map(|call| {
        (call.function == Function::BaseUnits && call.args[0].reference() == Some(input))
            .then(|| call.args[1].clone())
    });
    let shape = template.values.values().any(|call| {
        call.args
            .iter()
            .zip(call.function.params())
            .any(|(arg, param)| arg.reference() == Some(input) && *param == ValueType::Shape)
    });
    if shape {
        return InputKind::Argument {
            kind: super::tree::tree_shape_kind(),
            scale: InputScale::None,
        };
    }
    if let Some(decimals) = amount {
        let scale = match decimals {
            Term::Literal(text) => match text.parse() {
                Ok(decimals) => InputScale::Fixed {
                    decimals,
                    unit: None,
                },
                Err(_) => InputScale::None,
            },
            Term::Ref(name) if template.input(&name).is_some() => InputScale::FromInput {
                input: name,
                unit: None,
            },
            Term::Ref(value) => InputScale::FromValue { value, unit: None },
        };
        return InputKind::Argument {
            kind: IdlArgKind::Integer {
                signed: false,
                float: false,
                bits: 64,
            },
            scale,
        };
    }
    let integer = template.values.values().any(|call| {
        call.args
            .iter()
            .zip(call.function.params())
            .any(|(arg, param)| arg.reference() == Some(input) && *param == ValueType::Integer)
    });
    if integer {
        InputKind::Argument {
            kind: IdlArgKind::Integer {
                signed: false,
                float: false,
                bits: 64,
            },
            scale: InputScale::None,
        }
    } else {
        InputKind::Address
    }
}

/// A size function counts the extensions of the template's own blocks, so
/// each Token-2022 `initialize…` block on that mint must be one it knows.
fn sizes(pass: &mut Pass<'_>) {
    let template = pass.template;
    let mints: Vec<&Term> = template
        .values
        .values()
        .filter(|call| {
            matches!(
                call.function,
                Function::Token2022MintSpace | Function::Token2022MintFundedSpace
            )
        })
        .filter_map(|call| call.args.first())
        .collect();
    for (index, block) in template.blocks.iter().enumerate() {
        if !candidates(template, block).contains(&TOKEN_2022_PROGRAM_ID) {
            continue;
        }
        let on_mint = block
            .accounts
            .get("mint")
            .is_some_and(|term| mints.contains(&term));
        if on_mint && sizing(&block.instruction) == Sizing::Unknown {
            pass.problem(
                index + 1,
                &block.instruction,
                None,
                format!(
                    "The size functions do not know the extension {} sets up, so they cannot size this mint.",
                    block.instruction
                ),
            );
        }
    }
}
