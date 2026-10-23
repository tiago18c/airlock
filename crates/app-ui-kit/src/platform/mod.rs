//! Where the UI touches the page and the device: eval, QR, camera, IDL files.
//! The browser bridge socket and Windows camera permission live in `app-desktop`;
//! its queue is in the core.

pub mod camera;
pub mod eval;
#[cfg(not(target_arch = "wasm32"))]
pub mod idl_files;
pub mod qr;
pub mod storage;

pub use eval::{copy_text, eval};
