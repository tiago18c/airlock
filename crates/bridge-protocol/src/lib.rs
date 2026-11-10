//! Shared framing and message shapes for the browser bridge.
//!
//! Pure: no sockets, no native messaging host. The desktop app and
//! `bridge-host` both speak this. [`data_dir`] is the one part that touches
//! files: it finds the folder both of them keep their data and logs in.

pub mod data_dir;
pub mod extension_id;
pub mod frame;
pub mod message;
pub mod reply;
pub mod request;

pub use request::{request_code, Frame, MAX_PENDING, MAX_PENDING_PER_ORIGIN, MAX_TRANSACTIONS};
