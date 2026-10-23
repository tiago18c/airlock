//! Runtime codec generation from Codama type nodes.
//!
//! Rust counterpart of the TypeScript `@codama/dynamic-codecs` package. Builds
//! [`Codec`]s (encode/decode) and [`Value`]s dynamically from the Codama node
//! tree, without code generation.

mod codec;
mod codec_visitor;
mod encoding;
mod struct_from_args;
mod value;
mod value_visitor;

pub use codec::*;
pub use codec_visitor::*;
pub use encoding::*;
pub use struct_from_args::*;
pub use value::*;
pub use value_visitor::*;
