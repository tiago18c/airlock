//! Converts Anchor IDL JSON into a Codama [`RootNode`].
//!
//! Port of the TypeScript `@codama/nodes-from-anchor` package (raw converters —
//! the equivalent of `rootNodeFromAnchorWithoutDefaultVisitor`). The TS
//! `defaultVisitor` post-processing pipeline (PDA extraction, fixed account
//! sizes, argument flattening, …) is codegen ergonomics and is intentionally
//! not ported: the raw tree already carries every discriminator, type and
//! default value required to encode, decode and build instructions at runtime.
//!
//! Two Anchor IDL specs exist and both are supported:
//! - **v01** (Anchor 0.30+, `metadata.spec == "0.1.0"`): explicit discriminator
//!   byte arrays, `pubkey` type leaf, object-form `defined`, generics.
//! - **v00** (older Anchor and Shank): computed sha256 discriminators
//!   (`global:`/`account:`/`event:` preimages), `publicKey` leaf, string-form
//!   `defined`, map/set collection types.

mod discriminators;
mod helpers;
mod v00;
mod v01;

use codama_nodes::RootNode;
pub use discriminators::{
    anchor_account_discriminator, anchor_event_discriminator, anchor_instruction_discriminator,
};
use serde_json::Value;

/// Errors produced while converting an Anchor IDL.
#[derive(Debug, thiserror::Error)]
pub enum AnchorIdlError {
    #[error("the IDL is not valid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("the IDL is not a JSON object")]
    NotAnObject,
    #[error("missing or invalid `{0}` field in the Anchor IDL")]
    MissingField(String),
    #[error("unrecognized Anchor IDL type: {0}")]
    UnrecognizedType(String),
    #[error("instruction argument `{0}` referenced by a PDA seed was not found")]
    ArgumentMissing(String),
    #[error("account `{0}` has no matching type definition in the IDL")]
    AccountTypeMissing(String),
    #[error("event `{0}` has no matching type definition in the IDL")]
    EventTypeMissing(String),
    #[error("generic type `{0}` is not defined in the IDL")]
    GenericTypeMissing(String),
    #[error("PDA seed kind `{0}` is not supported")]
    SeedKindUnimplemented(String),
    #[error("account `{0}` data must be a struct type")]
    AccountDataNotAStruct(String),
}

pub type Result<T> = std::result::Result<T, AnchorIdlError>;

/// Whether a parsed JSON document looks like an Anchor IDL (as opposed to a
/// Codama root node, which always carries `"kind": "rootNode"`).
pub fn is_anchor_idl(idl: &Value) -> bool {
    idl.get("kind").and_then(Value::as_str) != Some("rootNode")
        && (idl.get("instructions").is_some() || idl.get("metadata").is_some())
}

/// Converts an Anchor IDL JSON string into a Codama [`RootNode`].
pub fn root_node_from_anchor(json: &str) -> Result<RootNode> {
    root_node_from_anchor_value(&serde_json::from_str(json)?)
}

/// Converts a parsed Anchor IDL into a Codama [`RootNode`], dispatching on the
/// IDL spec version (`metadata.spec == "0.1.0"` → v01, anything else → v00).
pub fn root_node_from_anchor_value(idl: &Value) -> Result<RootNode> {
    if !idl.is_object() {
        return Err(AnchorIdlError::NotAnObject);
    }
    let spec = idl
        .get("metadata")
        .and_then(|m| m.get("spec"))
        .and_then(Value::as_str);
    let program = if spec == Some("0.1.0") {
        v01::program_node_from_anchor(idl)?
    } else {
        v00::program_node_from_anchor(idl)?
    };
    Ok(RootNode::new(program))
}
