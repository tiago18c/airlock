//! A Vector account's data.
//!
//! ```text
//!  0..32   nonce     what the next signature has to be made over
//! 32..64   seed      sha256 of the key the account was created with
//! 64..72   created   the slot it was created in (u64)
//! 72       scheme
//! 73       bump
//! 74       system bump   of the account's system PDA
//! 75       pad
//! 76..     what the scheme stores
//! ```

use super::scheme::{seed, SchemeId, HEADER_LEN, SCHEME_OFFSET};
use super::VectorError;
use crate::chain::RawAccount;
use solana_pubkey::Pubkey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorAccount {
    pub scheme: SchemeId,
    /// What the account's next signature has to be made over. It changes
    /// with every signature that lands.
    pub nonce: [u8; 32],
    /// The hash of the key the account was created with.
    pub seed: [u8; 32],
    /// The slot the account was created in. The program refuses to close it
    /// in that slot.
    pub created: u64,
    /// What the scheme stores after the header.
    stored: Vec<u8>,
}

impl VectorAccount {
    /// Read an account that belongs to the Vector program at `program` and
    /// is complete. One that is still being grown cannot sign, and is an
    /// error that says so. A zeroed account names scheme 0, which is none.
    pub fn parse(program: &Pubkey, account: &RawAccount) -> Result<Self, VectorError> {
        if account.owner != *program {
            return Err(VectorError::ForeignOwner {
                owner: account.owner,
                program: *program,
            });
        }
        let (header, stored) = account
            .data
            .split_at_checked(HEADER_LEN)
            .ok_or(VectorError::Truncated)?;
        let byte = header[SCHEME_OFFSET];
        let scheme = SchemeId::from_byte(byte).ok_or(VectorError::UnsupportedScheme(byte))?;
        if account.data.len() != scheme.account_len() {
            return Err(VectorError::Incomplete {
                scheme: scheme.label(),
                expected: scheme.account_len(),
                found: account.data.len(),
            });
        }
        let field = |start: usize| -> [u8; 32] {
            let mut out = [0u8; 32];
            out.copy_from_slice(&header[start..start + 32]);
            out
        };
        Ok(Self {
            scheme,
            nonce: field(0),
            seed: field(32),
            created: u64::from_le_bytes(header[64..72].try_into().expect("8 bytes")),
            stored: stored.to_vec(),
        })
    }

    /// This is the account that was created with `key`.
    pub fn created_with(&self, key: &[u8]) -> bool {
        self.seed == seed(key)
    }

    /// The key that verifies the account's next signature, where the account
    /// keeps it. Falcon-512 keeps only a transformed key, so that one has to
    /// come from whoever holds the key it was created with.
    pub fn current_key(&self) -> Option<&[u8]> {
        match self.scheme {
            SchemeId::Falcon512 => None,
            scheme => self.stored.get(..scheme.key_len()),
        }
    }

    /// The lowest leaf the chain still accepts a signature from, for a
    /// scheme whose key signs once per leaf.
    pub fn next_leaf(&self) -> Option<u32> {
        if self.scheme != SchemeId::Xmss {
            return None;
        }
        let bytes = self.stored.get(SchemeId::Xmss.key_len()..)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }
}
