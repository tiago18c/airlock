//! Templates on Build: a short TOML file that asks a few questions and
//! writes IDL blocks from the answers.
//!
//! ```text
//! template ─► check (form: inputs, options) ─► answers ─► expand ─► IdlDrafts
//! ```
//!
//! A template only binds answers, literals and derived values to the fields
//! of IDL instructions; it never writes bytes. [`check`] refuses one whose
//! programs, instructions or fields the IDLs of the network in use do not
//! have. [`expand`] writes the blocks, which are built and read on Review
//! and Sign as blocks made by hand are. Chain facts, such as rent, come back
//! from [`expand`] as [`Fact`]s for the caller to read from a node or ask for.
//!
//! Nothing here touches the network or a secret: the caller makes the
//! keypair of a `new-keypair` value and passes its address in [`Env`].
//! See `docs/templates.md` for the format.

mod builtin;
mod check;
mod expand;
mod model;
mod parse;
mod save;
mod size;
mod tree;
mod write;

#[cfg(test)]
mod expand_tests;
#[cfg(test)]
mod save_tests;
#[cfg(test)]
mod tests;

pub use builtin::{builtin_template, BuiltinTemplate, BUILTIN_TEMPLATES, STARTER_TEMPLATE};
pub use check::{check, FormInput, InputKind, InputScale, Problem, TemplateForm};
pub use expand::{expand, Answers, Env, ExpandedBlock, Expansion, Fact, Facts, Missing, Row};
pub use model::{
    Block, Call, Function, Input, Template, TemplateOption, Term, BUILT_IN_NAMES, FEE_PAYER, WALLET,
};
pub use parse::{
    TemplateError, TextPlace, MAX_BLOCKS, MAX_INPUTS, MAX_OPTIONS, MAX_PROGRAMS,
    MAX_TEMPLATE_BYTES, MAX_VALUES,
};
pub use save::{
    builtin_named, is_template_slug, template_slug, words, Choice, FieldAt, Locked, PlannedInput,
    SaveBlock, SaveField, SavePlan, Saved, MAX_SLUG_CHARS,
};
pub use size::{MINT_EXTENSIONS, TOKEN_METADATA};
