//! Core node-tree visitor framework for the Codama standard.
//!
//! This crate is the Rust counterpart of the TypeScript `@codama/visitors-core`
//! package. It provides the foundational traversal machinery the dynamic client
//! crates build on:
//!
//! - [`Visitor`] / [`visit`] — dispatch a node to a per-kind handler.
//! - [`NodePath`] — an owned ancestor→target chain of nodes.
//! - [`NodeStack`] — a stack of paths maintained during traversal.
//! - [`LinkableDictionary`] — resolves [`codama_nodes::LinkNode`]s to their target.

mod error;
mod linkable;
mod node_path;
mod node_stack;
mod visitor;

pub use error::{DynamicError, DynamicResult};
pub use linkable::*;
pub use node_path::*;
pub use node_stack::*;
pub use visitor::*;
