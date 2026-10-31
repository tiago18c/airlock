use codama_errors::CodamaError as UpstreamError;
use thiserror::Error;

/// Errors raised while walking a Codama IDL and building or parsing instructions.
///
/// Upstream [`codama_errors::CodamaError`] stays the error type for turning Rust
/// source into nodes. This enum is the runtime client's error type, and it
/// wraps that one so `?` still works on node conversions.
#[derive(Debug, Error)]
pub enum DynamicError {
    #[error(transparent)]
    Codama(#[from] UpstreamError),

    #[error("Unrecognized node kind: `{0}`")]
    UnrecognizedNodeKind(String),

    #[error("Unsupported node kind for this visitor: `{0}`")]
    UnsupportedNodeKind(String),

    #[error("Linked `{kind}` node not found: `{name}`")]
    LinkedNodeNotFound { kind: String, name: String },

    #[error("A type nests more than {0} levels deep, or links to itself")]
    TypeTooDeep(usize),

    #[error("A type expands to more than {0} nodes")]
    TypeTooLarge(usize),

    #[error("Cannot remove the last path in the node stack")]
    CannotRemoveLastPathInNodeStack,

    #[error("Missing required account: `{0}`")]
    AccountMissing(String),

    #[error("Missing required argument: `{0}`")]
    ArgumentMissing(String),

    #[error("injected value `{0}` was not provided and has no fallback")]
    InjectedValueMissing(String),

    #[error("Account `{name}` not found. Available accounts: {available}")]
    AccountNotFound { name: String, available: String },

    #[error("Circular account dependency detected: {chain}")]
    CircularAccountDependency { chain: String },

    #[error("Failed to derive PDA: {0}")]
    FailedToDerivePda(String),

    #[error("Missing resolver for account value: `{0}`")]
    AccountResolverMissing(String),

    #[error("Value cannot be converted to an address: {0}")]
    UnexpectedAddressType(String),

    #[error("Omitted argument `{argument}` of instruction `{instruction}` has no default value")]
    DefaultValueMissing {
        argument: String,
        instruction: String,
    },

    #[error("Failed to encode argument `{argument}`: {message}")]
    FailedToEncodeArgument { argument: String, message: String },

    #[error("Invalid input for argument `{argument}`: expected {expected}")]
    InvalidArgumentInput { argument: String, expected: String },

    #[error("Failed to execute resolver `{0}`")]
    FailedToExecuteResolver(String),

    #[error("Discriminator field `{0}` not found")]
    DiscriminatorFieldNotFound(String),

    #[error("Discriminator field `{0}` has no default value")]
    DiscriminatorFieldHasNoDefaultValue(String),

    #[error("Instruction `{name}` not found. Available instructions: {available}")]
    InstructionNotFound { name: String, available: String },

    #[error("PDA `{name}` not found. Available PDAs: {available}")]
    PdaNotFound { name: String, available: String },

    #[error("Node not found")]
    NodeNotFound,

    #[error("Invalid encoding: {0}")]
    InvalidBytesEncoding(String),

    #[error("Unexpected node: expected `{expected}`, found `{actual}`")]
    UnexpectedNode { expected: String, actual: String },
}

pub type DynamicResult<T> = Result<T, DynamicError>;
