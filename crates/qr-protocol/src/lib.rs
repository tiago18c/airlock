//! QRS1: a small QR transfer protocol for typed objects, the largest being a
//! full-size Solana v1 transaction with its context note.
//!
//! Each QR symbol carries one self-describing frame. The header is wincode,
//! little-endian:
//!
//! ```text
//!  0..4   magic          b"QRS1"
//!  4      version        u8 = 1
//!  5..13  transfer_id    8 bytes (stable for one payload)
//!  13..15 index          u16, 0-based
//!  15..17 count          u16, total frames
//!  17..21 total_len      u32, original payload length
//!  21..25 crc32          u32, IEEE CRC of the original payload
//!  25..   chunk          payload slice for this frame, zero-padded
//! ```
//!
//! Every frame of every transfer carries a chunk of the same size, the last
//! one padded with zeros, so every code on screen is the same QR version. A
//! reader keeps frames until it has `1..=count`, concatenates them in order,
//! cuts the padding at `total_len`, and accepts the result only if the CRC
//! matches.
//!
//! The payload is one or more typed objects (see [`QrObject`]): a
//! transaction (Solana v1 wire), a signature with what it signs, a context
//! note for a transaction, a public key, the keys of wallets another device
//! holds, a key rotation, or the state of an account. A transaction and its
//! note travel together as one payload, or each on its own.
//!
//! As text, a payload is JSON ([`payload_json`]), and a transaction alone is
//! its wire in base64, as other tools print one.
//!
//! This crate knows bytes, not Solana. The sizes it needs are written out
//! below, and the caller says what counts as a transaction.

mod assemble;
mod frame;
mod json;
mod object;
mod text;

#[cfg(test)]
mod tests;

pub use assemble::{Assembler, Collected, Collector, FrameSeen};
pub use frame::{clamp_chunk_size, crc32, encode_frames, hex_id, parse_frame, transfer_id, Frame};
pub use json::{parse_payload_json, payload_json};
pub use object::{
    decode_object, decode_payload, encode_object, encode_payload, encode_wallet_keys, AccountState,
    ContextAccount, ContextAuthorization, ContextPin, KeyRole, KeyRotation, QrObject,
    RotationSignature, Signed, TxContext, WalletKey, MAX_ACCOUNTS, MAX_AUTHORIZATIONS,
    MAX_KEY_LABEL, MAX_OBJECTS, MAX_PINS, MAX_ROTATION_NONCES, MAX_WALLET_KEYS,
};
pub use text::parse_payload_text;

/// The first byte of a Solana v1 transaction.
pub const V1_PREFIX: u8 = 0x81;
/// The largest v1 transaction Solana accepts, in bytes.
pub const MAX_TRANSACTION_SIZE: usize = 4096;
/// Bytes in an address.
pub const PUBKEY_BYTES: usize = 32;
/// Bytes in a signature.
pub const SIGNATURE_BYTES: usize = 64;

pub const MAGIC: &[u8; 4] = b"QRS1";
/// 2: the transfer id is the first 8 bytes of the payload's SHA-256, and
/// the receiver checks it.
pub const VERSION: u8 = 2;
pub const HEADER_LEN: usize = 25;
/// The largest payload: room for a full-size transaction, its context note
/// and anything else sent with it. 43 codes at [`DEFAULT_CHUNK`].
pub const MAX_PAYLOAD: usize = 32 * 1024;
/// QR version 40 at ECC-M holds 2331 data bytes.
pub const MAX_FRAME_BYTES: usize = 2331;
/// The chunk every transfer uses: 793-byte frames, QR version 23 at ECC-M.
pub const DEFAULT_CHUNK: usize = 768;
pub const MIN_CHUNK: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("payload is empty")]
    Empty,
    #[error("payload is {len} bytes; max is {MAX_PAYLOAD}")]
    TooLarge { len: usize },
    #[error("frame is shorter than the QRS1 header")]
    Truncated,
    #[error("not a QRS1 frame")]
    BadMagic,
    #[error("unsupported QRS1 version {0}")]
    UnsupportedVersion(u8),
    #[error("frame header is invalid")]
    BadHeader,
    #[error("frame belongs to a different transfer")]
    TransferMismatch,
    #[error("reassembled payload failed its checks; scan it again")]
    CorruptPayload,
    #[error("two codes of this transfer at one place differ; scan it again")]
    ConflictingFrame,
    #[error(
        "payload is not a transaction, a signature, a context note, a public key, wallet keys, a key rotation or an account state"
    )]
    BadObject,
    #[error("expected one object, and this payload holds {0}")]
    NotOneObject(usize),
}
