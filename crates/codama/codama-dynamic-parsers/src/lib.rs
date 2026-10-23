//! Runtime identification and parsing of Solana data from Codama nodes.
//!
//! Rust counterpart of the TypeScript `@codama/dynamic-parsers` package. Given a
//! [`codama_nodes::RootNode`] and a byte buffer, identifies which account,
//! event or instruction the bytes belong to (by discriminator) and decodes them
//! into a runtime [`codama_dynamic_codecs::Value`] — without code generation.

mod discriminators;
mod identify;
mod parsers;

pub use discriminators::{account_data_filters, DataFilter};
pub use identify::{
    identify_account_data, identify_data, identify_event_data, identify_instruction_data,
    ParsableKind, ALL_PARSABLE_KINDS,
};
pub use parsers::{
    parse_account_data, parse_account_data_as, parse_data, parse_event_data, parse_instruction,
    parse_instruction_data, ParsedData, ParsedInstruction, ParsedInstructionAccount,
};

pub use solana_instruction::{AccountMeta, Instruction};
