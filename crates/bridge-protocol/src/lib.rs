//! Shared framing and message shapes for the browser bridge.
//!
//! Pure: no sockets, no native messaging host. The desktop app and
//! `bridge-host` both speak this.

pub mod extension_id;
pub mod frame;
pub mod message;
pub mod reply;
pub mod request;

pub use request::{request_code, MAX_TRANSACTIONS};
