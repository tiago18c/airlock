//! Uniform Resources (BC-UR, BCR-2020-005): how Keystone and other airgapped
//! wallets pass CBOR through QR codes. A message is CBOR, written as
//! bytewords in `ur:<type>/...` text, and cut into fountain-coded parts when
//! it does not fit one code.
//!
//! No dependencies, as `qr-codec` has none: the hashes, the generator and
//! the CBOR subset are here, each checked against the reference vectors.
//! What the types mean (`sol-sign-request`, `crypto-multi-accounts`) is for
//! the caller: this crate knows bytes and CBOR, not Solana.

mod bytewords;
pub mod cbor;
pub mod fountain;
mod hash;
mod random;
mod ur;
mod words;

pub use bytewords::BytewordsError;
pub use cbor::{CborError, Value};
pub use fountain::{FountainError, Part, MAX_FRAGMENTS, MAX_MESSAGE};
pub use hash::{crc32, sha256};
pub use ur::{encode_single, is_ur, parse, Scanned, UrDecoder, UrEncoder, UrError};

/// Bytewords, for callers that print or read them on their own.
pub mod words_minimal {
    pub use crate::bytewords::{decode, encode};
}
