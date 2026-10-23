//! QR codes, written to be read and audited: an encoder, and a decoder for
//! camera frames. No dependencies and no `unsafe`.
//!
//! The scope is what Airlock needs. Model 2 symbols, versions 1 to 40, all
//! four ECC levels and all eight masks. The encoder writes one byte-mode
//! segment. The decoder reads numeric, alphanumeric and byte segments and
//! names every other mode (ECI, Kanji, structured append, FNC1) as
//! unsupported. Micro QR, rMQR and Model 1 are not recognised.
//!
//! See `README.md` for how each step maps to its file and to the spec.

#![forbid(unsafe_code)]

mod bch;
mod bitstream;
mod decode;
mod encode;
mod gf256;
mod image;
mod matrix;
mod rs;
mod scanner;
mod tables;

pub use decode::{decode_matrix, Decoded};
pub use encode::{encode, encode_with, Symbol};
pub use matrix::BitMatrix;
pub use scanner::{ScanError, Scanner, MAX_SIDE, MIN_SIDE};

/// Error correction level: roughly 7, 15, 25 and 30 % of codewords
/// recoverable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EcLevel {
    L,
    M,
    Q,
    H,
}

impl EcLevel {
    pub const ALL: [EcLevel; 4] = [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H];

    /// The two bits the format information stores.
    pub(crate) fn format_bits(self) -> u8 {
        match self {
            EcLevel::L => 0b01,
            EcLevel::M => 0b00,
            EcLevel::Q => 0b11,
            EcLevel::H => 0b10,
        }
    }

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// More bytes than version 40 holds at this level.
    TooLong { len: usize, max: usize },
    /// A version outside 1..=40.
    BadVersion,
    /// A mask outside 0..=7.
    BadMask,
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EncodeError::TooLong { len, max } => {
                write!(f, "{len} bytes do not fit in a QR code; the most is {max}")
            }
            EncodeError::BadVersion => f.write_str("QR version must be 1 to 40"),
            EncodeError::BadMask => f.write_str("QR mask must be 0 to 7"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Why a grid of modules did not decode, by the stage that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not 21 to 177 modules, in steps of 4.
    Size,
    /// The version information names another size.
    Version,
    /// Neither copy of the format information is readable.
    Format,
    /// A block has more errors than its ECC corrects.
    Ecc,
    /// The data bits do not parse.
    Bitstream,
    /// A mode this decoder does not read, by its 4-bit indicator.
    UnsupportedMode(u8),
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::Size => f.write_str("not a QR symbol size"),
            DecodeError::Version => f.write_str("version information disagrees with the size"),
            DecodeError::Format => f.write_str("format information is unreadable"),
            DecodeError::Ecc => f.write_str("too many errors to correct"),
            DecodeError::Bitstream => f.write_str("data does not parse"),
            DecodeError::UnsupportedMode(m) => write!(f, "unsupported QR mode {m:#06b}"),
        }
    }
}

impl std::error::Error for DecodeError {}
