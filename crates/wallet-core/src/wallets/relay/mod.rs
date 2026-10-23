//! A whole transaction that carries an authorization: a signature made for a
//! wallet that has no key, by the key that controls it.
//!
//! `programmatic` builds and reads one `Submit` instruction. This module works
//! on the transaction around it: which authorities still have to sign, how a
//! signature made on another device finds its place, and how a transaction
//! that names a programmatic signer as an ordinary signer is rebuilt into one
//! that can actually run.
//!
//! An authority's signature lives in instruction data, so adding one changes
//! the message. Signatures over the transaction itself are made after it, and
//! any made before are dropped.
//!
//! What is the same for every kind of authorization is here. What belongs to
//! the Ed25519 signer program's `Submit` is in `submit`, and the Vector
//! program's `Advance` in `vector` and `vector_wrap`.

use crate::error::{BuildError, SignError};
use crate::transport::protocol::Signed;
use crate::tx::builder::{DEFAULT_LOADED_ACCOUNTS_DATA_SIZE, MAX_CU_LIMIT};
use crate::tx::compose::with_margin;
use crate::tx::decode::{decode_v1_transaction, DecodedTx};
use crate::tx::signatures::insert_signature;
use crate::wallets::programmatic::ProgrammaticError;
use solana_hash::Hash;
use solana_message::{v1, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signature::SIGNATURE_BYTES;
use thiserror::Error;

mod submit;
mod vector;
mod vector_sign;
mod vector_wrap;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod vector_sign_tests;
#[cfg(test)]
mod vector_tests;

pub use submit::*;
pub use vector::{
    advance_next_keys, authorizations_for_note, vector_accounts, vector_signature_objects,
    AuthContext, VectorContext,
};
pub use vector_sign::{
    digests_to_sign, insert_personal_sign, insert_vector_signature, personal_sign_digest,
    sign_vector_authorizations, sign_vector_authorizations_in,
};
pub use vector_wrap::{wrap_for_vector, VectorWrap, PASSTHROUGH_CU};

/// Who signs an authorization, as far as the transaction says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuthorityId {
    /// An authority of a `Submit`. Its pubkey is in the instruction.
    Ed25519(Pubkey),
    /// A Vector account listed by an `Advance`. The instruction names the
    /// account and nothing else: the key that controls it, and its scheme,
    /// come from a wallet record or from the account's data.
    Vector(Pubkey),
}

impl AuthorityId {
    /// What the transaction names for this authority: the `Submit` key, or
    /// the Vector account. The key that controls a Vector account is not in
    /// the transaction; the Sign screen shows it from context when known.
    pub fn shown_as(&self) -> Pubkey {
        match self {
            Self::Ed25519(pubkey) | Self::Vector(pubkey) => *pubkey,
        }
    }
}

/// One authority of one authorization in a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthoritySlot {
    /// Which instruction of the transaction carries the authorization.
    pub instruction: usize,
    /// Who signs the authorization.
    pub authority: AuthorityId,
    /// The address the wrapped instructions see signing.
    pub address: Pubkey,
    /// A Vector account's: where its signature is in the instruction's data,
    /// as start and length. `None` for a `Submit`'s authority, and for an
    /// account whose scheme the context does not give when the data's
    /// length alone does not tell.
    pub place: Option<(usize, usize)>,
    /// A Vector account's: bytes of the key it signs with next, which sits
    /// in the data just before its signature. Winternitz and XMSS have one;
    /// zero for every other scheme, and when the place is unknown.
    pub next_key_len: usize,
    /// Something is in the signature's place.
    pub signed: bool,
    /// Whether the signature verifies, when it was checked. A Vector
    /// signature is checked when the context holds the account's nonce and
    /// key. A `Submit`'s is checked when it is written.
    pub verified: Option<bool>,
}

/// Every authority the transaction's authorizations name.
pub fn authority_slots(tx: &DecodedTx) -> Vec<AuthoritySlot> {
    authority_slots_in(tx, &AuthContext::default())
}

/// [`authority_slots`], with what `ctx` says of the Vector accounts: where
/// each one's signature is, and whether it verifies.
pub fn authority_slots_in(tx: &DecodedTx, ctx: &AuthContext) -> Vec<AuthoritySlot> {
    let mut slots = submit::slots(tx);
    slots.extend(vector::slots(tx, ctx));
    slots
}

/// A transaction after authorization signatures were written into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authorized {
    pub wire: Vec<u8>,
    /// Authorization signatures written.
    pub signed: usize,
    /// Signatures over the transaction that no longer match and were removed.
    pub dropped: usize,
}

/// Where a signature that arrived from another device belongs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureUse {
    /// It signs the transaction.
    Transaction,
    /// It signs an authorization inside the transaction. `dropped` earlier
    /// transaction signatures stopped matching and were removed.
    Authorization { dropped: usize },
}

/// Put a scanned or pasted signature where it says it belongs: in the
/// transaction's own signatures, or in an authorization the transaction
/// carries. It is verified there before it is written. The same key can be
/// both a signer and an authority, and its two signatures are two objects.
pub fn accept_signature(
    wire: &[u8],
    signed: &Signed,
) -> Result<(Vec<u8>, SignatureUse), SignError> {
    accept_signature_in(wire, signed, &AuthContext::default())
}

/// [`accept_signature`], with what `ctx` says of the Vector accounts. A
/// Vector account's signature needs it to be placed unless the data's
/// length tells, and to be checked before it is written.
pub fn accept_signature_in(
    wire: &[u8],
    signed: &Signed,
    ctx: &AuthContext,
) -> Result<(Vec<u8>, SignatureUse), SignError> {
    match signed {
        Signed::Transaction { pubkey, signature } => {
            let wire = insert_signature(wire, &Pubkey::new_from_array(*pubkey), signature)?;
            Ok((wire, SignatureUse::Transaction))
        }
        Signed::Authorization {
            authority,
            signature,
        } => {
            let authorized = insert_authority_signature_in(
                wire,
                &Pubkey::new_from_array(*authority),
                signature,
                ctx,
            )?;
            Ok((
                authorized.wire,
                SignatureUse::Authorization {
                    dropped: authorized.dropped,
                },
            ))
        }
        Signed::AccountAuthorization {
            account,
            nonce,
            signature,
        } => {
            let authorized = insert_vector_signature(
                wire,
                &Pubkey::new_from_array(*account),
                nonce,
                signature,
                ctx,
            )?;
            Ok((
                authorized.wire,
                SignatureUse::Authorization {
                    dropped: authorized.dropped,
                },
            ))
        }
    }
}

/// The same transaction on a newer blockhash. Its signatures are removed,
/// because they covered the old one. Authorization signatures are untouched:
/// they never covered a blockhash.
pub fn with_blockhash(wire: &[u8], blockhash: Hash) -> Result<Vec<u8>, SignError> {
    let tx = decode(wire)?;
    if tx.uses_durable_nonce {
        return Err(SignError::Message(
            "this transaction lives on a durable nonce, not on a blockhash".into(),
        ));
    }
    let (wire, _) = rebuild(&tx, |message| {
        message.lifetime_specifier = blockhash;
        Ok(())
    })?;
    Ok(wire)
}

/// The same transaction with its loaded accounts data size limit set to
/// `loaded` bytes plus the margin Build adds. `loaded` is what a simulation
/// of this transaction reported. Signatures over the transaction are
/// removed. Authorization signatures are untouched: they do not cover the
/// limit.
///
/// A rebuilt transaction starts at the largest limit there is, because the
/// limit of the transaction it was made from does not fit it. On devnet a
/// transfer loads 206 bytes, and the same transfer inside an authorization
/// loads 497,297: the signer, executor and nonce programs are loaded whole.
/// With the first number as its limit, the second fails with
/// `MaxLoadedAccountsDataSizeExceeded` before it runs. A node reports what
/// was loaded even when the simulation fails later, such as on an
/// authorization nobody has signed yet.
pub fn fit_loaded_accounts(wire: &[u8], loaded: u32) -> Result<Vec<u8>, SignError> {
    let tx = decode(wire)?;
    let limit = with_margin(u64::from(loaded), DEFAULT_LOADED_ACCOUNTS_DATA_SIZE);
    let (wire, _) = rebuild(&tx, |message| {
        message.config.loaded_accounts_data_size_limit = Some(limit);
        Ok(())
    })?;
    Ok(wire)
}

/// The same transaction with its compute unit limit set to `limit`.
/// Signatures over the transaction are removed. Authorization signatures are
/// untouched: they do not cover the limit.
pub fn with_compute_unit_limit(wire: &[u8], limit: u32) -> Result<Vec<u8>, SignError> {
    let tx = decode(wire)?;
    let (wire, _) = rebuild(&tx, |message| {
        message.config.compute_unit_limit = Some(limit.min(MAX_CU_LIMIT));
        Ok(())
    })?;
    Ok(wire)
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WrapError {
    #[error("No signer of this transaction is a programmatic signer this device knows.")]
    NothingToWrap,
    #[error("This transaction already carries an authorization.")]
    AlreadyWrapped,
    #[error("A transaction that advances a durable nonce cannot be wrapped. Build it on a recent blockhash.")]
    DurableNonce,
    #[error("{0} is a programmatic signer and cannot pay the fee. Choose a hot wallet to pay.")]
    PayerCannotSign(Pubkey),
    #[error("Instruction {} is a programmatic authorization that {account} would have to sign. A Vector account cannot sign inside another authorization, so sign its instructions outside it.", instruction + 1)]
    VectorInsideSubmit { instruction: usize, account: Pubkey },
    #[error("Instruction {} asks the System program to take lamports from {account}. A Vector account can only send lamports as a plain transfer.", instruction + 1)]
    VectorSystemDebit { instruction: usize, account: Pubkey },
    #[error(transparent)]
    Vector(#[from] crate::wallets::vector::VectorError),
    #[error(transparent)]
    Programmatic(#[from] ProgrammaticError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("{0}")]
    Message(String),
}

fn decode(wire: &[u8]) -> Result<DecodedTx, SignError> {
    decode_v1_transaction(wire).map_err(|err| SignError::Message(err.to_string()))
}

/// Apply `change` to the message of `tx` and lay the wire out again. When the
/// message did change, every signature over it is void: the new wire has none,
/// and the count of those that were there is returned.
fn rebuild(
    tx: &DecodedTx,
    change: impl FnOnce(&mut v1::Message) -> Result<(), SignError>,
) -> Result<(Vec<u8>, usize), SignError> {
    let versioned: VersionedMessage = wincode::deserialize(&tx.message)
        .map_err(|_| SignError::Message("the message could not be read".into()))?;
    let VersionedMessage::V1(mut message) = versioned else {
        return Err(SignError::Message("this is not a v1 transaction".into()));
    };
    change(&mut message)?;
    let bytes = VersionedMessage::V1(message).serialize();
    if bytes == tx.message {
        return Ok((tx.wire.clone(), 0));
    }
    let dropped = tx.signers.iter().filter(|slot| slot.filled).count();
    let mut wire = bytes;
    wire.extend(std::iter::repeat_n(
        0u8,
        tx.signers.len().saturating_mul(SIGNATURE_BYTES),
    ));
    Ok((wire, dropped))
}
