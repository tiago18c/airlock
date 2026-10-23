//! Runtime PDA and account address resolution from Codama nodes.
//!
//! Rust counterpart of the TypeScript `@codama/dynamic-address-resolution`
//! package. Given a [`codama_nodes::RootNode`] and an instruction, resolves
//! each account's address — following PDA seeds, account defaults, conditionals
//! and custom resolvers — without code generation. The API is synchronous, as
//! Solana PDA derivation ([`solana_pubkey::Pubkey::find_program_address`]) is.

mod inputs;
mod resolver;

pub use inputs::*;
pub use resolver::*;
pub use solana_pubkey::Pubkey;
