//! What hardware wallets say, as bytes. No I/O: `hw-transport` opens the
//! devices and moves these bytes, and `wallet-flows` decides what to ask.
//!
//! - [`ledger`]: the Solana app's APDUs, the dashboard's, the HID framing
//!   and the status words.
//! - [`trezor`]: Codec v1 framing, the session and Solana messages, written
//!   by hand with a small protobuf codec. Trezor's `.proto` files are
//!   LGPL-3.0, so they are not copied or compiled here.
//! - [`keystone`]: the UR types it reads and writes over QR.
//! - [`link`]: the connection a transport hands over, and exchanges on it.
//!
//! This crate knows bytes and paths, not Solana transactions.

pub mod keystone;
pub mod ledger;
pub mod link;
mod path;
mod proto;
pub mod trezor;

pub use path::{format_path, parse_path, PathError, HARDENED};
