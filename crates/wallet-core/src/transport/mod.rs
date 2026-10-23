//! Moving a transaction between devices.

pub(crate) mod handoff;
pub mod protocol;

pub use handoff::{signature_objects, SignatureObject};
