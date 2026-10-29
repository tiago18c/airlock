use solana_pubkey::Pubkey;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BuildError {
    #[error("offline builds need a durable nonce (recent blockhash is online-only)")]
    BlockhashOffline,
    #[error("compiled transaction is {len} bytes; a v1 transaction holds at most {max}", max = solana_message::v1::MAX_TRANSACTION_SIZE)]
    TooLarge { len: usize },
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecodeError {
    #[error("payload is not a Solana transaction v1 (expected 0x81 prefix)")]
    NotV1,
    #[error("transaction is truncated or corrupt")]
    Truncated,
    #[error("instruction references an account that is not in this message")]
    BadAccountIndex,
    #[error("address lookup table {0} is missing")]
    MissingLookupTable(Pubkey),
    #[error("an address lookup table index is past the end of the table")]
    LookupIndexOutOfRange,
    #[error("{0}")]
    Message(String),
}

pub use wallet_vault::VaultError;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RpcError {
    #[error("RPC is disabled in offline mode")]
    Offline,
    #[error("RPC timed out")]
    Timeout,
    /// No connection to the node could be made.
    #[error("The node could not be reached: {0}")]
    Unreachable(String),
    /// The node has not reached the slot the read asked for yet.
    #[error("The node has not caught up with the chain yet")]
    Behind,
    #[error("{0}")]
    Message(String),
}

impl RpcError {
    /// The node did not answer at all, so asking it again right away would
    /// not answer either. A node that answered with an error is not down.
    pub fn is_down(&self) -> bool {
        matches!(self, Self::Offline | Self::Timeout | Self::Unreachable(_))
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SignError {
    #[error("this wallet cannot sign")]
    CannotSign,
    #[error("signer does not match required pubkey")]
    WrongPubkey,
    #[error("none of the required signers are unlocked locally")]
    NoLocalSigner,
    #[error("The nonce of the Vector account {0} is not known here. Scan the transaction's context note, or read the account while online.")]
    NoNonce(Pubkey),
    #[error(transparent)]
    Vector(#[from] VectorError),
    #[error("{0}")]
    Message(String),
}

/// A seed phrase, derivation path or pasted private key that cannot be used.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DeriveError {
    #[error("Derivation path is required.")]
    PathRequired,
    #[error("This path has no * to vary.")]
    NoWildcard,
    #[error("Use a single * for the account index.")]
    ManyWildcards,
    #[error("Derivation path: {0}")]
    Path(String),
    #[error("Seed phrase must be 12, 15, 18, 21, or 24 words.")]
    PhraseLength,
    #[error("Seed phrase is not a valid English mnemonic.")]
    PhraseInvalid,
    #[error("Private key is required.")]
    KeyRequired,
    #[error("JSON keypair: {0}")]
    KeyJson(String),
    #[error("Not a base58 private key.")]
    KeyBase58,
    #[error("Invalid hex.")]
    KeyHex,
    #[error("Expected a 32-byte seed or a 64-byte keypair.")]
    KeyLength,
    #[error("{0}")]
    Key(String),
}

/// An address-book file that cannot be written or read.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BookFileError {
    #[error("could not write the address book: {0}")]
    Encode(String),
    #[error("that is not an address book file: {0}")]
    NotABook(String),
    #[error("that file has no addresses to import")]
    Empty,
}

/// What goes wrong with a Vector key, account, signature or transaction.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum VectorError {
    #[error("A {scheme} key is {expected} bytes, and this one is {found}.")]
    KeyLength {
        scheme: &'static str,
        expected: usize,
        found: usize,
    },
    #[error("A {scheme} signature is {expected} bytes, and this one is {found}.")]
    SignatureLength {
        scheme: &'static str,
        expected: usize,
        found: usize,
    },
    #[error("That is not a {scheme} key. Paste an Ed25519 key in base58, and any other as hex.")]
    BadKeyText { scheme: &'static str },
    #[error("No address can be derived for this key.")]
    NoAddress,
    #[error("That is not a recovery phrase: it takes 12, 15, 18, 21 or 24 English words with a valid checksum.")]
    BadPhrase,
    #[error(
        "No key this recovery phrase makes, up to number {searched}, is the key the account holds."
    )]
    NotFromPhrase { searched: u32 },
    #[error("This account belongs to {owner}, not to the Vector program {program}.")]
    ForeignOwner { owner: Pubkey, program: Pubkey },
    #[error("This account is too short to be a Vector account.")]
    Truncated,
    #[error("This Vector account is of scheme {0}, which this build does not sign for.")]
    UnsupportedScheme(u8),
    #[error("This {scheme} account is {found} bytes of the {expected} it needs. It is still being set up: its Initialize has to run again before it can sign.")]
    Incomplete {
        scheme: &'static str,
        expected: usize,
        found: usize,
    },
    #[error("A {scheme} account keeps its key, so its Advance names no next one.")]
    KeepsItsKey { scheme: &'static str },
    #[error("A {scheme} account names the key it signs with next in every Advance.")]
    NeedsNextKey { scheme: &'static str },
    #[error("This Advance hands the Winternitz account {0} to the key that just signed for it. A Winternitz key signs once.")]
    KeyReused(Pubkey),
    #[error("An Advance needs at least one account to sign.")]
    NoSigners,
    #[error("An Advance lists at most 254 accounts, and this one would list {0}.")]
    TooManySigners(usize),
    #[error("{0} is listed twice in one Advance. An account signs once per transaction.")]
    SignerTwice(Pubkey),
    #[error("A Passthrough carries at most 255 instructions, and this one would carry {0}.")]
    TooManyInstructions(usize),
    #[error("Instruction {} of this Passthrough has {found} accounts, and at most 255 fit.", index + 1)]
    InnerAccounts { index: usize, found: usize },
    #[error("Instruction {} of this Passthrough has {found} bytes of data, and at most 65,535 fit.", index + 1)]
    InnerData { index: usize, found: usize },
    #[error("This Passthrough would list {0} accounts, and one instruction lists at most 255. Split what it runs into more than one transaction.")]
    PassthroughAccounts(usize),
    #[error("A Passthrough runs as at most 16 accounts, and this one would run as {0}.")]
    TooManyPassthroughSigners(usize),
    #[error("This transaction has no Advance of the Vector program.")]
    NoAdvance,
    #[error("This transaction has two Advances of the Vector program. Each would have to sign the other's signatures, so neither can be signed.")]
    TwoAdvances,
    #[error("The Advance in this transaction lists other accounts or schemes than the ones it is being signed for.")]
    AdvanceMismatch,
    #[error("The Advance in this transaction is malformed: it lists no accounts, or more accounts or data than one instruction holds.")]
    MalformedAdvance,
    #[error("That wallet is not a Vector wallet in this vault.")]
    NotVector,
    #[error("The key kept for {wallet} cannot be read.")]
    BadHeldKey { wallet: String },
    #[error("The key of {0} is not on this device. Sign on the device that holds it.")]
    SignedElsewhere(String),
    #[error("This build cannot sign with a {scheme} key yet.")]
    CannotSign { scheme: &'static str },
    #[error("The key that signs for {wallet} here is not the key its account was created with.")]
    NotTheKey { wallet: String },
    #[error("The {scheme} key could not sign.")]
    SigningFailed { scheme: &'static str },
    #[error("This {scheme} signature does not verify: it was made over other instructions, at another nonce, or by another key.")]
    DoesNotVerify { scheme: &'static str },
    #[error("That is not a 65-byte EIP-191 signature (hex, with or without 0x).")]
    BadPersonalSign,
    #[error("That signature was made over the digest's hex text, not the 32 bytes. Sign the bytes with personal_sign.")]
    SignedDigestAsText,
    #[error("{0} is marked writable in this transaction, and the runtime will not let it be written. Its flag would differ from what was signed, so the signature could not land. Remove the write.")]
    DemotedWritable(Pubkey),
    #[error(
        "The context note says {0} uses another scheme than its wallet here does. It was not used."
    )]
    NoteScheme(Pubkey),
    #[error("This transaction hands {0} to a key this device did not make for it. Whoever holds that key would control the account, so it is not signed.")]
    RotatesElsewhere(Pubkey),
    #[error("The key that signs for {0} here is not the one the account holds now. If a transaction that replaces its key has landed, install the next key on the Wallets screen.")]
    NotCurrentKey(Pubkey),
    #[error("What the hash-based key of {0} signs has to be recorded before it signs.")]
    NotReserved(Pubkey),
    #[error("This is not a Passthrough instruction.")]
    NotPassthrough,
    #[error("This Passthrough instruction is malformed: {0}.")]
    MalformedPassthrough(&'static str),
}
