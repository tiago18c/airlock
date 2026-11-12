//! Reading a template file. The format is closed: a key, function or name
//! this module does not know refuses the whole file.

use super::model::*;
use serde::Deserialize;
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

/// The largest file read, in bytes.
pub const MAX_TEMPLATE_BYTES: usize = 64 * 1024;
/// More instructions than a transaction can hold.
pub const MAX_BLOCKS: usize = 32;
pub const MAX_INPUTS: usize = 32;
pub const MAX_OPTIONS: usize = 16;
pub const MAX_VALUES: usize = 32;
pub const MAX_PROGRAMS: usize = 16;
/// The longest label or name, in characters.
const MAX_LABEL_CHARS: usize = 80;
/// The longest summary or hint, in characters.
const MAX_HINT_CHARS: usize = 300;
const MAX_NAME_CHARS: usize = 48;

/// Why a template file was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateError {
    /// Larger than [`MAX_TEMPLATE_BYTES`].
    TooLarge { bytes: usize },
    /// Not TOML, or a key or type the format does not have. `message` is
    /// the TOML reader's, which names the key; `place` is where it points.
    Toml {
        message: String,
        place: Option<TextPlace>,
    },
    /// `key` holds something the format does not allow. Blocks, inputs and
    /// options are numbered from 1 in `key`, as `block[2].accounts.mint`.
    Invalid { key: String, expected: String },
    /// A character that can make the text read as something other than it
    /// is: a control character other than tab, line feed and carriage
    /// return, or a Unicode bidirectional control. `at` is where: "line 3"
    /// when it is in the text itself, or the key whose string holds it when
    /// it was written as an escape (`block[1].arguments.name`).
    /// `place` is where it is when it is in the text itself.
    Hidden {
        at: String,
        character: char,
        place: Option<TextPlace>,
    },
}

/// A place in a template's text, both counted from 1: the line, and the
/// character within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPlace {
    pub line: usize,
    pub column: usize,
}

impl TemplateError {
    /// Where in the text it points, when it points somewhere: a TOML error
    /// and a hidden character in the text do; a key that holds something
    /// the format does not allow names its key instead.
    pub fn place(&self) -> Option<TextPlace> {
        match self {
            Self::Toml { place, .. } | Self::Hidden { place, .. } => *place,
            Self::TooLarge { .. } | Self::Invalid { .. } => None,
        }
    }
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { bytes } => write!(
                f,
                "This template is {bytes} bytes. A template can be at most {MAX_TEMPLATE_BYTES}."
            ),
            Self::Toml {
                message,
                place: Some(place),
            } => write!(
                f,
                "This is not a template file: line {}: {message}",
                place.line
            ),
            Self::Toml {
                message,
                place: None,
            } => write!(f, "This is not a template file: {message}"),
            Self::Invalid { key, expected } => write!(f, "{key}: {expected}"),
            Self::Hidden { at, character, .. } => write!(
                f,
                "{at}: holds U+{:04X}, {}. It can make the text read as something other than what it is, so a template may not hold one.",
                *character as u32,
                hidden_kind(*character).unwrap_or("a hidden character")
            ),
        }
    }
}

impl std::error::Error for TemplateError {}

fn invalid(key: impl Into<String>, expected: impl Into<String>) -> TemplateError {
    TemplateError::Invalid {
        key: key.into(),
        expected: expected.into(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTemplate {
    #[allow(dead_code)]
    format: i64,
    name: String,
    summary: Option<String>,
    #[serde(default)]
    programs: BTreeMap<String, String>,
    #[serde(default, rename = "option")]
    options: Vec<RawOption>,
    #[serde(default, rename = "input")]
    inputs: Vec<RawInput>,
    #[serde(default)]
    values: BTreeMap<String, String>,
    #[serde(default, rename = "block")]
    blocks: Vec<RawBlock>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOption {
    name: String,
    label: String,
    hint: Option<String>,
    #[serde(default)]
    default: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInput {
    name: String,
    label: String,
    hint: Option<String>,
    #[serde(default)]
    optional: bool,
    default: Option<String>,
    option: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlock {
    when: Option<String>,
    program: Option<String>,
    instruction: Option<String>,
    #[serde(default)]
    accounts: BTreeMap<String, String>,
    #[serde(default)]
    arguments: BTreeMap<String, String>,
}

impl Template {
    /// Reads a template file, refusing anything the format does not allow.
    pub fn parse(text: &str) -> Result<Template, TemplateError> {
        if text.len() > MAX_TEMPLATE_BYTES {
            return Err(TemplateError::TooLarge { bytes: text.len() });
        }
        hidden_in_text(text)?;
        let table: toml::Table = toml::from_str(text).map_err(|err| toml_error(text, &err))?;
        hidden_in_table(&table, "")?;
        match table.get("format") {
            Some(toml::Value::Integer(1)) => {}
            Some(toml::Value::Integer(other)) => {
                return Err(invalid(
                    "format",
                    format!("format {other} is not one this app reads. Expected format = 1."),
                ))
            }
            Some(_) => return Err(invalid("format", "Expected format = 1.")),
            None => return Err(invalid("format", "Missing. Expected format = 1.")),
        }
        let raw: RawTemplate = toml::from_str(text).map_err(|err| toml_error(text, &err))?;
        let template = read(raw)?;
        validate(&template)?;
        Ok(template)
    }
}

/// What `c` is, when it is a character a template may not hold: a control
/// character other than tab, line feed and carriage return, or a Unicode
/// bidirectional control (embeddings, overrides, isolates and marks). Each
/// can make a line show something other than what it holds.
fn hidden_kind(c: char) -> Option<&'static str> {
    match c {
        '\t' | '\n' | '\r' => None,
        '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}'
        | '\u{200E}'
        | '\u{200F}'
        | '\u{061C}' => Some("a bidirectional control"),
        c if c.is_control() => Some("a control character"),
        _ => None,
    }
}

/// Refuses text that holds a hidden character, naming its line.
fn hidden_in_text(text: &str) -> Result<(), TemplateError> {
    for (index, line) in text.split('\n').enumerate() {
        if let Some((column, character)) = line
            .chars()
            .enumerate()
            .find(|(_, c)| hidden_kind(*c).is_some())
        {
            return Err(TemplateError::Hidden {
                at: format!("line {}", index + 1),
                character,
                place: Some(TextPlace {
                    line: index + 1,
                    column: column + 1,
                }),
            });
        }
    }
    Ok(())
}

/// Refuses a key or a string that holds a hidden character written as an
/// escape (a backslash, `u` and its code), which the text does not show,
/// naming the key.
fn hidden_in_table(table: &toml::Table, prefix: &str) -> Result<(), TemplateError> {
    for (key, value) in table {
        let at = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if let Some(character) = key.chars().find(|c| hidden_kind(*c).is_some()) {
            return Err(TemplateError::Hidden {
                at,
                character,
                place: None,
            });
        }
        hidden_in_value(value, &at)?;
    }
    Ok(())
}

fn hidden_in_value(value: &toml::Value, at: &str) -> Result<(), TemplateError> {
    match value {
        toml::Value::String(text) => match text.chars().find(|c| hidden_kind(*c).is_some()) {
            Some(character) => Err(TemplateError::Hidden {
                at: at.to_string(),
                character,
                place: None,
            }),
            None => Ok(()),
        },
        toml::Value::Array(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(index, item)| hidden_in_value(item, &format!("{at}[{}]", index + 1))),
        toml::Value::Table(table) => hidden_in_table(table, at),
        _ => Ok(()),
    }
}

/// The TOML reader's message, with the place it points at.
fn toml_error(text: &str, err: &toml::de::Error) -> TemplateError {
    TemplateError::Toml {
        message: err.message().trim().to_string(),
        place: err.span().map(|span| place_of(text, span.start)),
    }
}

/// The line and character of byte `at` in `text`, both from 1.
fn place_of(text: &str, at: usize) -> TextPlace {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let before = &text[..at];
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    TextPlace {
        line: before.matches('\n').count() + 1,
        column: before[line_start..].chars().count() + 1,
    }
}

fn count(key: &str, len: usize, max: usize) -> Result<(), TemplateError> {
    if len > max {
        return Err(invalid(key, format!("{len} entries. At most {max}.")));
    }
    Ok(())
}

fn label(key: &str, text: String, max: usize) -> Result<String, TemplateError> {
    if text.trim().is_empty() {
        return Err(invalid(key, "Empty. Expected some text."));
    }
    if text.chars().count() > max {
        return Err(invalid(key, format!("Longer than {max} characters.")));
    }
    Ok(text)
}

fn hint(key: &str, text: Option<String>) -> Result<Option<String>, TemplateError> {
    text.map(|text| label(key, text, MAX_HINT_CHARS))
        .transpose()
}

fn name(key: &str, text: &str) -> Result<String, TemplateError> {
    let mut chars = text.chars();
    let starts = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !starts || !rest || text.len() > MAX_NAME_CHARS {
        return Err(invalid(
            key,
            format!(
                "`{text}` is not a name. Expected a letter, then letters, digits, `_` or `-`, at most {MAX_NAME_CHARS} in all."
            ),
        ));
    }
    Ok(text.to_string())
}

fn term(key: &str, text: &str) -> Result<Term, TemplateError> {
    Term::read(text).ok_or_else(|| {
        invalid(
            key,
            "`$` with no name after it. Write `$$` for a dollar sign.",
        )
    })
}

fn read(raw: RawTemplate) -> Result<Template, TemplateError> {
    count("programs", raw.programs.len(), MAX_PROGRAMS)?;
    count("option", raw.options.len(), MAX_OPTIONS)?;
    count("input", raw.inputs.len(), MAX_INPUTS)?;
    count("values", raw.values.len(), MAX_VALUES)?;
    count("block", raw.blocks.len(), MAX_BLOCKS)?;

    let mut programs = BTreeMap::new();
    for (alias, address) in raw.programs {
        let key = format!("programs.{alias}");
        let alias = name(&key, &alias)?;
        let address = Pubkey::from_str(address.trim())
            .map_err(|_| invalid(&key, format!("`{address}` is not an address.")))?;
        programs.insert(alias, address);
    }

    let mut options = Vec::new();
    for (index, option) in raw.options.into_iter().enumerate() {
        let key = format!("option[{}]", index + 1);
        options.push(TemplateOption {
            name: name(&format!("{key}.name"), &option.name)?,
            label: label(&format!("{key}.label"), option.label, MAX_LABEL_CHARS)?,
            hint: hint(&format!("{key}.hint"), option.hint)?,
            default: option.default,
        });
    }

    let mut inputs = Vec::new();
    for (index, input) in raw.inputs.into_iter().enumerate() {
        let key = format!("input[{}]", index + 1);
        inputs.push(Input {
            name: name(&format!("{key}.name"), &input.name)?,
            label: label(&format!("{key}.label"), input.label, MAX_LABEL_CHARS)?,
            hint: hint(&format!("{key}.hint"), input.hint)?,
            optional: input.optional,
            default: input
                .default
                .map(|text| term(&format!("{key}.default"), &text))
                .transpose()?,
            option: input.option,
        });
    }

    let mut values = BTreeMap::new();
    for (value, text) in raw.values {
        let key = format!("values.{value}");
        let value = name(&key, &value)?;
        let mut words = text.split_whitespace();
        let Some(function) = words.next() else {
            return Err(invalid(
                &key,
                "Empty. Expected a function and its arguments.",
            ));
        };
        let function = Function::by_name(function).ok_or_else(|| {
            invalid(
                &key,
                format!(
                    "`{function}` is not a function. Expected one of: {}.",
                    Function::ALL.map(Function::name).join(", ")
                ),
            )
        })?;
        let args = words
            .map(|word| term(&key, word))
            .collect::<Result<Vec<_>, _>>()?;
        if args.len() != function.params().len() {
            return Err(invalid(
                &key,
                format!(
                    "{} takes {}, not {} argument{}.",
                    function.name(),
                    function.usage(),
                    args.len(),
                    if args.len() == 1 { "" } else { "s" }
                ),
            ));
        }
        values.insert(value, Call { function, args });
    }

    let mut blocks = Vec::new();
    for (index, block) in raw.blocks.into_iter().enumerate() {
        let key = format!("block[{}]", index + 1);
        let program = match block.program {
            Some(text) => match term(&format!("{key}.program"), &text)? {
                Term::Ref(name) => name,
                Term::Literal(text) => {
                    return Err(invalid(
                        format!("{key}.program"),
                        format!("`{text}` is not a reference. Expected `$` and a program alias from [programs]."),
                    ))
                }
            },
            None => return Err(invalid(&key, "No program. Expected program = \"$alias\".")),
        };
        let instruction = match block.instruction {
            Some(text) if !text.trim().is_empty() => text,
            _ => return Err(invalid(
                &key,
                "No instruction. Expected instruction = \"name\", as the program's IDL names it.",
            )),
        };
        let mut accounts = BTreeMap::new();
        for (field, text) in block.accounts {
            let term = term(&format!("{key}.accounts.{field}"), &text)?;
            accounts.insert(field, term);
        }
        let mut arguments = BTreeMap::new();
        for (field, text) in block.arguments {
            if accounts.contains_key(&field) {
                return Err(invalid(
                    format!("{key}.arguments.{field}"),
                    "Also bound under accounts. Bind a field once.",
                ));
            }
            let term = term(&format!("{key}.arguments.{field}"), &text)?;
            arguments.insert(field, term);
        }
        blocks.push(Block {
            when: block.when,
            program,
            instruction,
            accounts,
            arguments,
        });
    }

    Ok(Template {
        name: label("name", raw.name, MAX_LABEL_CHARS)?,
        summary: hint("summary", raw.summary)?,
        programs,
        options,
        inputs,
        values,
        blocks,
    })
}

/// Names, references, types of function arguments and loops.
fn validate(template: &Template) -> Result<(), TemplateError> {
    // One namespace.
    let mut seen: BTreeMap<&str, String> = BUILT_IN_NAMES
        .iter()
        .map(|name| (*name, "a built-in name".to_string()))
        .collect();
    let declared = template
        .programs
        .keys()
        .map(|name| (name.as_str(), format!("programs.{name}")))
        .chain(
            template
                .inputs
                .iter()
                .enumerate()
                .map(|(i, input)| (input.name.as_str(), format!("input[{}].name", i + 1))),
        )
        .chain(
            template
                .values
                .keys()
                .map(|name| (name.as_str(), format!("values.{name}"))),
        )
        .chain(
            template
                .options
                .iter()
                .enumerate()
                .map(|(i, option)| (option.name.as_str(), format!("option[{}].name", i + 1))),
        );
    for (name, key) in declared {
        if let Some(first) = seen.get(name) {
            return Err(invalid(
                key,
                format!(
                    "`{name}` is already used by {first}. Every name in a template is different."
                ),
            ));
        }
        seen.insert(name, key);
    }

    let option_named = |key: String, name: &str| -> Result<(), TemplateError> {
        if template.option(name).is_none() {
            return Err(invalid(
                key,
                format!("`{name}` is not an option of this template."),
            ));
        }
        Ok(())
    };

    let mut used: BTreeSet<&str> = BTreeSet::new();
    for (index, input) in template.inputs.iter().enumerate() {
        let key = format!("input[{}]", index + 1);
        if let Some(option) = &input.option {
            option_named(format!("{key}.option"), option)?;
        }
        if let Some(Term::Ref(name)) = &input.default {
            if !BUILT_IN_NAMES.contains(&name.as_str()) && !template.programs.contains_key(name) {
                return Err(invalid(
                    format!("{key}.default"),
                    format!("`${name}` cannot be a default. Expected a literal, $wallet, $fee-payer or a program alias."),
                ));
            }
        }
    }

    for (value, call) in &template.values {
        let key = format!("values.{value}");
        for (arg, param) in call.args.iter().zip(call.function.params()) {
            match arg {
                Term::Ref(name) => {
                    reference(template, &key, name)?;
                    used.insert(name);
                    if let Some(other) = template.values.get(name) {
                        let fits = match param {
                            ValueType::Integer => other.function.returns() == ValueType::Integer,
                            _ => other.function.returns() != ValueType::Integer,
                        };
                        if !fits {
                            return Err(invalid(
                                &key,
                                format!(
                                    "`${name}` is not what {} takes: {}.",
                                    call.function.name(),
                                    call.function.usage()
                                ),
                            ));
                        }
                    }
                }
                Term::Literal(text) => {
                    let fits = match param {
                        ValueType::Integer => text.parse::<u64>().is_ok(),
                        _ => Pubkey::from_str(text).is_ok(),
                    };
                    if !fits {
                        return Err(invalid(
                            &key,
                            format!(
                                "`{text}` is not what {} takes: {}.",
                                call.function.name(),
                                call.function.usage()
                            ),
                        ));
                    }
                }
            }
        }
    }
    loops(template)?;

    for (index, block) in template.blocks.iter().enumerate() {
        let key = format!("block[{}]", index + 1);
        if let Some(option) = &block.when {
            option_named(format!("{key}.when"), option)?;
        }
        let program = &block.program;
        let is_program = template.programs.contains_key(program)
            || template
                .values
                .get(program)
                .is_some_and(|call| call.function == Function::TokenProgramOf);
        if !is_program {
            return Err(invalid(
                format!("{key}.program"),
                format!("`${program}` is not a program. Expected a program alias from [programs], or a value of token-program-of."),
            ));
        }
        used.insert(program);
        for (field, term) in block
            .accounts
            .iter()
            .map(|(f, t)| (format!("accounts.{f}"), t))
            .chain(
                block
                    .arguments
                    .iter()
                    .map(|(f, t)| (format!("arguments.{f}"), t)),
            )
        {
            if let Term::Ref(name) = term {
                reference(template, &format!("{key}.{field}"), name)?;
                used.insert(name);
            }
        }
    }

    for (index, input) in template.inputs.iter().enumerate() {
        if !used.contains(input.name.as_str()) {
            return Err(invalid(
                format!("input[{}].name", index + 1),
                format!("`{}` is not used by any block or value.", input.name),
            ));
        }
    }
    for value in template.values.keys() {
        if !used.contains(value.as_str()) {
            return Err(invalid(
                format!("values.{value}"),
                "Not used by any block or other value.",
            ));
        }
    }
    Ok(())
}

fn reference(template: &Template, key: &str, name: &str) -> Result<(), TemplateError> {
    if template.names_value(name) {
        return Ok(());
    }
    if template.option(name).is_some() {
        return Err(invalid(
            key,
            format!(
                "`${name}` is an option. An option turns blocks on with `when`; it holds no value."
            ),
        ));
    }
    Err(invalid(
        key,
        format!("`${name}` names nothing. Expected an input, a value, a program alias, $wallet or $fee-payer."),
    ))
}

/// A value that names itself, directly or through others, has no order to
/// be worked out in.
fn loops(template: &Template) -> Result<(), TemplateError> {
    fn walk<'a>(
        template: &'a Template,
        name: &'a str,
        path: &mut Vec<&'a str>,
        cleared: &mut BTreeSet<&'a str>,
    ) -> Result<(), TemplateError> {
        if cleared.contains(name) {
            return Ok(());
        }
        if let Some(at) = path.iter().position(|step| *step == name) {
            let mut cycle: Vec<String> = path[at..].iter().map(|step| format!("${step}")).collect();
            cycle.push(format!("${name}"));
            return Err(invalid(
                format!("values.{name}"),
                format!("These values need each other: {}.", cycle.join(" → ")),
            ));
        }
        let Some(call) = template.values.get(name) else {
            return Ok(());
        };
        path.push(name);
        for arg in &call.args {
            if let Some(next) = arg.reference() {
                walk(template, next, path, cleared)?;
            }
        }
        path.pop();
        cleared.insert(name);
        Ok(())
    }
    let mut cleared = BTreeSet::new();
    for name in template.values.keys() {
        walk(template, name, &mut Vec::new(), &mut cleared)?;
    }
    Ok(())
}
