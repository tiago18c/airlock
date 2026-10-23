//! Native messaging host shared with the desktop app's socket listener.
//!
//! Framing and message shapes live in `bridge-protocol`. This crate owns the
//! socket, the install step, and the host binary.

pub mod endpoint;
pub mod install;
pub mod log;
pub mod relay;

pub use bridge_protocol::{extension_id, frame, message};
