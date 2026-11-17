//! The signing schemes, their sizes, and the address of a key.

use super::VectorError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solana_pubkey::Pubkey;

/// First seed of every Vector account's address.
pub const VECTOR_SEED: &[u8] = b"vector";

/// Bytes of the header every Vector account starts with. The scheme's own
/// bytes follow it.
pub const HEADER_LEN: usize = 76;

/// Where the header keeps the scheme's byte, after the nonce, the seed and
/// the slot the account was created in.
pub(super) const SCHEME_OFFSET: usize = 72;

/// The eight Ed25519 points of order dividing 8, in their canonical
/// encodings, which the program refuses as keys: their signatures never
/// verify, or for a non-canonical encoding anyone can make one.
const ED25519_SMALL_ORDER: [[u8; 32]; 8] = {
    let mut points = [[0u8; 32]; 8];
    // Order 1: (0, 1).
    points[0][0] = 1;
    // Order 2: (0, -1).
    points[1] = [
        0xec, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x7f,
    ];
    // Order 4: (+-sqrt(-1), 0). points[2] stays all zeros.
    points[3][31] = 0x80;
    // Order 8.
    points[4] = [
        0xc7, 0x17, 0x6a, 0x70, 0x3d, 0x4d, 0xd8, 0x4f, 0xba, 0x3c, 0x0b, 0x76, 0x0d, 0x10, 0x67,
        0x0f, 0x2a, 0x20, 0x53, 0xfa, 0x2c, 0x39, 0xcc, 0xc6, 0x4e, 0xc7, 0xfd, 0x77, 0x92, 0xac,
        0x03, 0x7a,
    ];
    points[5] = points[4];
    points[5][31] = 0xfa;
    points[6] = [
        0x26, 0xe8, 0x95, 0x8f, 0xc2, 0xb2, 0x27, 0xb0, 0x45, 0xc3, 0xf4, 0x89, 0xf2, 0xef, 0x98,
        0xf0, 0xd5, 0xdf, 0xac, 0x05, 0xd3, 0xc6, 0x33, 0x39, 0xb1, 0x38, 0x02, 0x88, 0x6d, 0x53,
        0xfc, 0x05,
    ];
    points[7] = points[6];
    points[7][31] = 0x85;
    points
};

/// Whether an Ed25519 encoding's y (the low 255 bits) is at least
/// p = 2^255 - 19: a second encoding of a point with a small y.
fn ed25519_y_not_reduced(key: &[u8; 32]) -> bool {
    key[31] & 0x7f == 0x7f && key[1..31].iter().all(|&byte| byte == 0xff) && key[0] >= 0xed
}

/// A signing scheme this app signs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SchemeId {
    Ed25519,
    /// Secp256k1 behind Ethereum's `personal_sign` envelope. The key is an
    /// Ethereum address.
    Eip191,
    Secp256k1,
    Falcon512,
    MlDsa44,
    /// Hash-based, one signature per key; every wrap must rotate.
    Winternitz,
    /// Hash-based, 256 signatures per key, and each leaf signs once.
    Xmss,
}

impl SchemeId {
    pub const ALL: [Self; 7] = [
        Self::Ed25519,
        Self::Eip191,
        Self::Secp256k1,
        Self::Falcon512,
        Self::MlDsa44,
        Self::Winternitz,
        Self::Xmss,
    ];

    /// The scheme byte: a field of the account's header and a seed of its
    /// address.
    pub const fn byte(self) -> u8 {
        match self {
            Self::Ed25519 => 1,
            Self::Eip191 => 2,
            Self::Secp256k1 => 3,
            Self::Falcon512 => 4,
            Self::MlDsa44 => 5,
            Self::Winternitz => 6,
            Self::Xmss => 7,
        }
    }

    pub fn from_byte(byte: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|scheme| scheme.byte() == byte)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ed25519 => "Ed25519",
            Self::Eip191 => "EIP-191",
            Self::Secp256k1 => "secp256k1",
            Self::Falcon512 => "Falcon-512",
            Self::MlDsa44 => "ML-DSA-44",
            Self::Winternitz => "Winternitz",
            Self::Xmss => "XMSS",
        }
    }

    /// Bytes of the key an account is created with: a public key, or for
    /// EIP-191 an Ethereum address.
    pub const fn key_len(self) -> usize {
        match self {
            Self::Ed25519 => 32,
            Self::Eip191 => 20,
            Self::Secp256k1 => 33,
            Self::Falcon512 => 897,
            Self::MlDsa44 => 1312,
            Self::Winternitz | Self::Xmss => 41,
        }
    }

    /// Bytes of one signature in an `Advance`.
    pub const fn signature_len(self) -> usize {
        match self {
            Self::Ed25519 | Self::Secp256k1 => 64,
            Self::Eip191 => 65,
            Self::Falcon512 => 666,
            Self::MlDsa44 => 2420,
            Self::Winternitz => 849,
            Self::Xmss => 1037,
        }
    }

    /// Bytes of the key an `Advance` names for the account to sign with next:
    /// a key of the scheme for Winternitz and XMSS, none for the others.
    pub const fn next_key_len(self) -> usize {
        if self.is_stateful() {
            self.key_len()
        } else {
            0
        }
    }

    /// Bytes an account takes in an `Advance`: its next key, then its
    /// signature.
    pub const fn advance_len(self) -> usize {
        self.next_key_len() + self.signature_len()
    }

    /// Compute units an `Advance` spends on one signature of this scheme,
    /// with a fifth more for margin. Measured against our deployment (a wrap
    /// that lands a `Withdraw`).
    pub const fn advance_cu(self) -> u32 {
        match self {
            Self::Ed25519 => 16_000,
            Self::Eip191 => 34_000,
            Self::Secp256k1 => 90_000,
            Self::Falcon512 => 220_800,
            Self::MlDsa44 => 316_800,
            // Measured ~38k on a rebuilt transfer; a fifth more for margin.
            Self::Winternitz => 46_000,
            Self::Xmss => 48_000,
        }
    }

    /// Bytes the account holds after its header.
    pub(super) const fn stored_len(self) -> usize {
        match self {
            Self::Ed25519 => 32,
            Self::Eip191 => 20,
            Self::Secp256k1 => 33,
            // The key with its transform done. The key itself is not kept.
            Self::Falcon512 => 1024,
            // The key, then its matrix expanded.
            Self::MlDsa44 => 1312 + 20_544,
            // The key that signs next.
            Self::Winternitz => 41,
            // The key that signs now, then the lowest leaf that still may.
            Self::Xmss => 41 + 4,
        }
    }

    /// Bytes of a complete account.
    pub const fn account_len(self) -> usize {
        HEADER_LEN + self.stored_len()
    }

    /// The program client's description of this scheme. Nothing outside
    /// `wallets/vector` sees it.
    pub(super) const fn core(self) -> vector_core::Scheme {
        match self {
            Self::Ed25519 => vector_core::ED25519,
            Self::Eip191 => vector_core::EIP191,
            Self::Secp256k1 => vector_core::SECP256K1,
            Self::Falcon512 => vector_core::FALCON512,
            Self::MlDsa44 => vector_core::MLDSA44,
            Self::Winternitz => vector_core::WINTERNITZ,
            Self::Xmss => vector_core::XMSS,
        }
    }

    /// A key of this scheme signs a limited number of times, and a device
    /// has to remember how many it has used (or rotate every time).
    pub const fn is_stateful(self) -> bool {
        matches!(self, Self::Winternitz | Self::Xmss)
    }

    /// Every `Advance` must hand the account to a new key (Winternitz). XMSS
    /// must past the force-rotate threshold, which is checked separately.
    pub const fn always_rotates(self) -> bool {
        matches!(self, Self::Winternitz)
    }

    /// The first eight hex characters of a nonce, for the Sign screen and
    /// the wallet row. Both devices of a pair show the same digits so a
    /// wrong note is obvious.
    pub fn show_nonce(nonce: &[u8; 32]) -> String {
        hex::encode(&nonce[..4])
    }

    /// `key` as a person reads it: base58 for an Ed25519 key, an Ethereum
    /// address for EIP-191, hex for secp256k1. A longer key is shown by its
    /// length and the start of its hash, which is also what its address is
    /// made from.
    pub fn show_key(self, key: &[u8]) -> String {
        match self {
            Self::Ed25519 => bs58::encode(key).into_string(),
            Self::Eip191 => format!("0x{}", hex::encode(key)),
            Self::Secp256k1 => hex::encode(key),
            Self::Falcon512 | Self::MlDsa44 | Self::Winternitz | Self::Xmss => format!(
                "{}-byte key, hash {}…",
                key.len(),
                hex::encode(&seed(key)[..8])
            ),
        }
    }

    pub(super) fn check_key(self, key: &[u8]) -> Result<(), VectorError> {
        if key.len() != self.key_len() {
            return Err(self.wrong_key(key));
        }
        Ok(())
    }

    /// `key` is one the program creates an account for, as its
    /// `Initialize` checks: an Ed25519 key is a point of large order in its
    /// one canonical encoding, and a secp256k1 key's x coordinate is on the
    /// curve. An account made for any other could never sign, or anyone
    /// could sign for it.
    pub(super) fn check_creatable(self, key: &[u8]) -> Result<(), VectorError> {
        self.check_key(key)?;
        let refused = VectorError::KeyRefused {
            scheme: self.label(),
        };
        match self {
            Self::Ed25519 => {
                let key: [u8; 32] = key.try_into().map_err(|_| self.wrong_key(key))?;
                if !Pubkey::new_from_array(key).is_on_curve()
                    || ed25519_y_not_reduced(&key)
                    || ED25519_SMALL_ORDER.contains(&key)
                {
                    return Err(refused);
                }
            }
            Self::Secp256k1 => {
                if !matches!(key[0], 0x02 | 0x03) || k256::PublicKey::from_sec1_bytes(key).is_err()
                {
                    return Err(refused);
                }
            }
            Self::Eip191 if key.iter().all(|byte| *byte == 0) => return Err(refused),
            Self::Eip191 | Self::Falcon512 | Self::MlDsa44 | Self::Winternitz | Self::Xmss => {}
        }
        Ok(())
    }

    pub(super) fn wrong_key(self, key: &[u8]) -> VectorError {
        VectorError::KeyLength {
            scheme: self.label(),
            expected: self.key_len(),
            found: key.len(),
        }
    }

    /// `next_key` is what an `Advance` can name for an account of this
    /// scheme: nothing, or a key of the scheme.
    pub(super) fn check_next_key(self, next_key: &[u8]) -> Result<(), VectorError> {
        match (self.is_stateful(), next_key.is_empty()) {
            (false, true) => Ok(()),
            (false, false) => Err(VectorError::KeepsItsKey {
                scheme: self.label(),
            }),
            (true, true) => Err(VectorError::NeedsNextKey {
                scheme: self.label(),
            }),
            (true, false) => self.check_key(next_key),
        }
    }

    pub(super) fn check_signature(self, signature: &[u8]) -> Result<(), VectorError> {
        if signature.len() != self.signature_len() {
            return Err(VectorError::SignatureLength {
                scheme: self.label(),
                expected: self.signature_len(),
                found: signature.len(),
            });
        }
        Ok(())
    }
}

/// What names an account for life: the hash of the key it was created with.
/// It is in the account's header and in its address.
pub fn seed(key: &[u8]) -> [u8; 32] {
    Sha256::digest(key).into()
}

/// The account of `key` under `scheme`, in the Vector program at `program`.
/// `key` is what the account is created with. For XMSS and Winternitz that
/// is the first key, also after the account has moved on to another.
pub fn address(program: &Pubkey, scheme: SchemeId, key: &[u8]) -> Result<Pubkey, VectorError> {
    scheme.check_key(key)?;
    Pubkey::try_find_program_address(&[VECTOR_SEED, &[scheme.byte()], &seed(key)], program)
        .map(|(address, _)| address)
        .ok_or(VectorError::NoAddress)
}

/// The address a Vector wallet of `key` under `scheme` is known by: the
/// system PDA of its account. It belongs to the System program, so it holds
/// the wallet's SOL, pays rent and owns its token accounts like any wallet,
/// and every `Passthrough` the app builds signs as it. The account itself
/// keeps only the key and the nonce.
pub fn wallet_address(
    program: &Pubkey,
    scheme: SchemeId,
    key: &[u8],
) -> Result<Pubkey, VectorError> {
    let account = address(program, scheme, key)?;
    Ok(super::passthrough::system_pda_under(program, &account))
}
