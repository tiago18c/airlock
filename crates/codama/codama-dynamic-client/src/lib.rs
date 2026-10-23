//! Runtime dynamic Solana program client from a Codama IDL.
//!
//! Rust counterpart of the TypeScript `@codama/dynamic-client` package — the
//! facade that ties together [`codama_dynamic_codecs`],
//! [`codama_dynamic_resolution`], [`codama_dynamic_instructions`] and
//! [`codama_dynamic_parsers`]. Parse an IDL into a [`ProgramClient`], then build
//! instructions, derive PDAs and parse on-chain data — all without code
//! generation, and fully synchronously.

mod client;
mod collect_pdas;

pub use client::ProgramClient;
pub use codama_visitors_core::{DynamicError, DynamicResult};
pub use collect_pdas::collect_pda_nodes;

// Re-exports of the building blocks, so consumers need only this crate.
pub use codama_dynamic_codecs::{get_type_codec, Codec, NumberValue, Value};
pub use codama_dynamic_instructions::{
    get_instruction_display, get_instruction_display_from_dictionary,
    required_accounts_for_display, AccountMeta, AccountsInput, ArgumentResolverFn, ArgumentsInput,
    DictionaryAccount, DictionaryName, DisplayDictionary, DisplayField, Instruction,
    InstructionBuilder, InstructionDisplay, IntentBody, IntentSpan, IntentValue, ResolverFn,
    ScaleSource,
};
pub use codama_dynamic_parsers::{
    DataFilter, ParsableKind, ParsedData, ParsedInstruction, ParsedInstructionAccount,
};
pub use codama_dynamic_resolution::{encode_constant_seed, encode_seed_literal, value_to_pubkey};
pub use solana_pubkey::Pubkey;
