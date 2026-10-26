//! Program interfaces: the catalog of IDLs, the ones that ship with the app,
//! and building an instruction from one.

pub(crate) mod build;
pub(crate) mod builtin;
pub(crate) mod catalog;
pub(crate) mod draft;
pub(crate) mod network;
pub(crate) mod pack;
#[cfg(test)]
mod system_idl;

pub use build::{
    build_idl_instruction, idl_instruction_form, IdlAccountField, IdlAccountSource, IdlArgField,
    IdlArgKind, IdlBuildFailure, IdlBuildInput, IdlForm,
};
pub use builtin::{
    builtin_idl, pinned_program, BuiltinIdl, EmbeddedIdl, IdlSource, PinnedProgram, BUILTIN_IDLS,
    ED25519_SIGNER_PROGRAM_ID, PINNED_PROGRAMS,
};
pub use catalog::sha256_bytes;
#[cfg(not(target_arch = "wasm32"))]
pub use catalog::DirIdlBodies;
pub use catalog::{
    anchor_idl_account_data, anchor_idl_address, idl_bytes_from_anchor_account,
    idl_bytes_from_program_metadata, outline_idl, pretty_idl, program_metadata_account_data,
    program_metadata_idl_address, IdlAccountUse, IdlArgOutline, IdlBodies, IdlBundleReport,
    IdlCatalog, IdlChainSource, IdlCheck, IdlError, IdlErrorOutline, IdlIxOutline, IdlNamed,
    IdlOrigin, IdlOutline, IdlPdaOutline, IdlRecord, IdlSpec, IdlStore, MemoryBodies,
    MAX_IDL_BYTES,
};
pub use draft::IdlDraft;
pub use network::IdlNetwork;
pub use pack::{idls_from_pack, IdlPackError};
