//! Runtime instruction building from Codama nodes.
//!
//! Rust counterpart of the TypeScript `@codama/dynamic-instructions` package.
//! Given a [`codama_nodes::RootNode`] and an [`codama_nodes::InstructionNode`],
//! [`InstructionBuilder`] resolves account addresses, encodes arguments and
//! assembles a native [`solana_instruction::Instruction`] — without code
//! generation. The API is synchronous, building on the synchronous
//! [`codama_dynamic_resolution`] and [`codama_dynamic_codecs`] crates.

mod accounts;
mod arguments;
mod builder;
mod display;

pub use accounts::create_account_metas;
pub use arguments::{encode_instruction_arguments, resolve_argument_defaults, ArgumentResolverFn};
pub use builder::InstructionBuilder;
pub use codama_dynamic_parsers::{ParsedInstruction, ParsedInstructionAccount};
pub use display::{
    get_instruction_display, get_instruction_display_from_dictionary,
    required_accounts_for_display, DictionaryAccount, DictionaryName, DisplayDictionary,
    DisplayField, InstructionDisplay, IntentBody, IntentSpan, IntentValue, ScaleSource,
};

// Re-exports of the inputs shared across the dynamic-client crates.
pub use codama_dynamic_resolution::{value_to_pubkey, AccountsInput, ArgumentsInput, ResolverFn};
pub use solana_instruction::{AccountMeta, Instruction};
pub use solana_pubkey::Pubkey;
