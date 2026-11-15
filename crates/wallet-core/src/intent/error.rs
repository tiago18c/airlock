//! Why a request is refused: a note that fails a check (`bad_notes`), or
//! something the account cannot do (`unsupported`).

use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use thiserror::Error;

/// A note that does not hold for the transactions it is about. Each one
/// names the address or the `data` entry at fault, by its place in the
/// request's `data`. Every one is `bad_notes` on the wire.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NoteError {
    #[error(
        "{address} has a note, but no transaction of the request lists it among its own accounts."
    )]
    NotInTransactions { address: Pubkey },
    #[error("Transaction {transaction} reads {address} from a lookup table, so it cannot carry a note. List it among the transaction's own accounts.")]
    FromLookupTable { transaction: usize, address: Pubkey },
    #[error("{address} carries more than one note. Give each address one.")]
    TwoNotes { address: Pubkey },
    #[error("{address} is the account the request acts as, which carries no note. Leave it out of accounts: when it pays the fee, who pays is the wallet's to choose.")]
    OnAccount { address: Pubkey },
    #[error("The note on {address} derives {derived}, not {address}.")]
    WrongDerivation { address: Pubkey, derived: Pubkey },
    #[error("The note on {address} cannot be derived: {reason}")]
    NotDerivable { address: Pubkey, reason: String },
    #[error("{address} is marked new, but it signs no transaction of the request. A new account has to sign the transaction that creates it.")]
    NewDoesNotSign { address: Pubkey },
    #[error("{address} is marked new, but the chain has seen it before.")]
    SeenOnChain { address: Pubkey },
    #[error("data[{entry}] names transaction {transaction}, and the request has {count}.")]
    NoSuchTransaction {
        entry: usize,
        transaction: usize,
        count: usize,
    },
    #[error("data[{entry}] names instruction {instruction} of transaction {transaction}, which has {count}.")]
    NoSuchInstruction {
        entry: usize,
        transaction: usize,
        instruction: usize,
        count: usize,
    },
    #[error("data[{entry}] names {len} bytes at offset {offset}, past the end of the instruction's {data_len} bytes of data.")]
    OutOfRange {
        entry: usize,
        offset: usize,
        len: usize,
        data_len: usize,
    },
    #[error("data[{entry}] says the bytes at offset {offset} are {address}, and they are not.")]
    NotThatAddress {
        entry: usize,
        offset: usize,
        address: Pubkey,
    },
    #[error("data[{entry}] says the byte at offset {offset} is the bump of {address}, {bump}, and it is not.")]
    NotThatBump {
        entry: usize,
        offset: usize,
        address: Pubkey,
        bump: u8,
    },
    #[error("data[{entry}] names the bump of {address}, which carries no pda note.")]
    BumpOfNotPda { entry: usize, address: Pubkey },
    #[error("{address} may be replaced, and transaction {transaction}, instruction {instruction}, holds it at offset {offset} with no data entry for it. Declare it in data, or mark the account not replaceable.")]
    UndeclaredUse {
        address: Pubkey,
        transaction: usize,
        instruction: usize,
        offset: usize,
    },
    #[error("{address} has no note that lets the wallet replace it.")]
    NotReplaceable { address: Pubkey },
    #[error("The derivations of {address} rest on each other in a loop.")]
    Loop { address: Pubkey },
    #[error("With the replacements, transaction {transaction} would list {address} twice.")]
    SameAddress { transaction: usize, address: Pubkey },
    #[error("Transaction {transaction} could not be read: {reason}")]
    Unreadable { transaction: usize, reason: String },
}

impl NoteError {
    /// The error code on the wire.
    pub fn code(&self) -> &'static str {
        "bad_notes"
    }
}

/// What a request needs that an account cannot do, as `error.requirement`
/// names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Requirement {
    Delivery,
    FeePayer,
    NewAccounts,
    Signer,
    Order,
    Messages,
    Chain,
}

impl Requirement {
    /// The name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Self::Delivery => "delivery",
            Self::FeePayer => "feePayer",
            Self::NewAccounts => "newAccounts",
            Self::Signer => "signer",
            Self::Order => "order",
            Self::Messages => "messages",
            Self::Chain => "chain",
        }
    }
}

/// A request the account cannot resolve: `unsupported`, naming what it
/// would need.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{message}")]
pub struct Unsupported {
    pub requirement: Requirement,
    /// What it is, for a person.
    pub message: String,
}

impl Unsupported {
    pub fn new(requirement: Requirement, message: impl Into<String>) -> Self {
        Self {
            requirement,
            message: message.into(),
        }
    }

    /// The error code on the wire.
    pub fn code(&self) -> &'static str {
        "unsupported"
    }
}
