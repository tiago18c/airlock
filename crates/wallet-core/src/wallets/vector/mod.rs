//! Vector: an account controlled by a key of one of several signing schemes
//! (`github.com/blueshift-gg/vector`, as of its single-program layout).
//!
//! One program serves every scheme. An account is a program-derived address
//! made from its scheme and the hash of the key it was created with. Its
//! owner signs a digest of a whole transaction off chain. In that
//! transaction, `Advance` verifies the signature and moves the account's
//! nonce, and `Passthrough` then runs instructions with the account as their
//! signer:
//!
//! ```text
//! relay transaction      fee payer signs; has a recent blockhash
//! ├─ Advance             per account it lists: its next key, its signature
//! └─ Passthrough         for one of those accounts
//!    └─ instructions     signed for by the account
//! ```
//!
//! A Winternitz key signs once and an XMSS key once per leaf, so the
//! `Advance` names the key each of those accounts signs with next. The
//! signature covers it.
//!
//! What is here needs no node: the schemes and their sizes, the address of a
//! key, an account's data, a `Passthrough` read back into the instructions it
//! carries, and whether the app offers Vector wallets at all. Building
//! instructions and the digest come from the program's client crate,
//! `vector-core` of our fork, which compiles in [`PROGRAM_ID`].

use solana_pubkey::Pubkey;

mod account;
mod digest;
mod instructions;
mod keys;
mod offer;
mod passthrough;
mod personal_sign;
mod phrase;
mod scheme;
mod sign;
mod xmss;

#[cfg(test)]
mod core_tests;
#[cfg(test)]
mod digest_tests;
#[cfg(test)]
mod instruction_tests;
#[cfg(test)]
mod tests;

pub use crate::error::VectorError;
pub use account::VectorAccount;
pub use digest::{advance_accounts, advance_message, demoted_writable, signer_digest};
pub use instructions::{
    advance, close, initialize, multi_passthrough, passthrough, passthrough_fits, rotation,
    rotation_message, withdraw, AdvanceEntry, MAX_INSTRUCTION_ACCOUNTS, MAX_PASSTHROUGH_SIGNERS,
};
pub(crate) use keys::{eip191_identity, secp256k1_identity};
pub use keys::{
    new_eip191_authority, new_falcon512_authority, new_held_key, new_mldsa44_authority,
    new_rotating_key, new_secp256k1_authority, new_winternitz_authority, new_xmss_authority,
    parse_public_key, winternitz_from_seed, xmss_from_seed,
};
pub use offer::{scheme_offered, signs_here};
pub use passthrough::{
    parse_passthrough, rewrite_passthrough, system_pda, Passthrough, PassthroughSigner,
    PASSTHROUGH_DISCRIMINATOR,
};
pub use personal_sign::parse_personal_sign;
pub use phrase::{
    find_phrase_key, new_key_phrase, normalize_key_phrase, phrase_key, phrase_seed,
    PHRASE_SEARCH_AHEAD,
};
pub use scheme::{address, seed, SchemeId, HEADER_LEN, VECTOR_SEED};
pub use sign::{verify, VectorKey, VectorSigner};
pub use xmss::{
    leaf_status_note, leaves_left, leaves_used, must_rotate, salt_of_signature, warn_leaves,
    XmssRetry, XmssRetryCache, XMSS_FORCE_ROTATE_USED, XMSS_LEAVES, XMSS_SALT_LEN,
    XMSS_SALT_OFFSET, XMSS_WARN_USED,
};

// What a wallet of this device does with its Vector account, named under this
// module too, so callers keep one path for both.
pub use crate::wallets::vector_wallets::{account_state, adopt_account_state, Adopted};
pub use crate::wallets::vector_wallets::{
    held_vector_accounts, install_landed_rotations, keys_to_sign_with, local_vector_keys,
    next_key_target, offered, reserve, reserve_leaf, sign_rotation, LeafError, LeafFor, LeafMarks,
    Reserved, RotationError, LEAVES_SKIPPED_WITHOUT_MARK,
};

/// Our deployment of the Vector program, on devnet. A wallet's record keeps
/// the program it was made under, so this is only what a new one is made
/// under.
pub const PROGRAM_ID: Pubkey = vector_core::PROGRAM_ID;

/// The first byte of an `Advance`'s data.
pub const ADVANCE_DISCRIMINATOR: u8 = vector_core::ADVANCE_DISCRIMINATOR;
