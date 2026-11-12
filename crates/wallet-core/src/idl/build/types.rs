//! What a build form asks for, and how a build can fail.

use super::*;

/// Values the build form has collected. Missing accounts are `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdlBuildInput {
    pub accounts: Vec<(String, Option<Pubkey>)>,
    pub arguments: Vec<(String, String)>,
}

/// How an instruction account is collected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdlAccountSource {
    /// Address-book picker. Optional accounts may stay empty.
    Picker,
    /// `InstructionBuilder` fills this. The user does not type it.
    Derived,
    /// A default the encoder cannot resolve offline. The user types the address.
    Fill,
}

/// One account row on the IDL build form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlAccountField {
    pub name: String,
    pub signer: bool,
    pub optional: bool,
    pub source: IdlAccountSource,
    /// Concrete seed labels for a derived account.
    pub seeds: Vec<String>,
    /// Derived address, once every seed it depends on is filled.
    pub address: Option<Pubkey>,
    /// Why a [`IdlAccountSource::Fill`] account cannot be derived.
    pub reason: String,
}

/// The input widget for one argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdlArgKind {
    /// A number of `bits` bits. A value the field cannot hold is refused.
    Integer {
        signed: bool,
        float: bool,
        bits: u8,
    },
    Pubkey,
    String,
    Bool,
    Enum {
        variants: Vec<String>,
    },
    Option(Box<IdlArgKind>),
    Bytes,
    /// A Codama type this form does not have a widget for. The text is rejected
    /// at build time with `note`.
    Raw {
        note: String,
    },
}

/// How the text of an integer argument reads, from the IDL's amount display.
/// A scaled field is typed in its unit ("1.5" SOL) and encoded as the integer
/// (1500000000 lamports).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum IdlArgScale {
    /// Typed in base units, as the program reads it.
    #[default]
    None,
    /// Typed with up to `decimals` decimal places. A unit with no decimals,
    /// such as bytes, is a scale of 0.
    Fixed { decimals: u8, unit: Option<String> },
    /// Typed with as many decimal places as the argument `argument` of the
    /// same instruction holds, as `transferChecked.amount` takes `decimals`.
    FromArgument {
        argument: String,
        unit: Option<String>,
    },
}

impl IdlArgScale {
    /// The unit the IDL names, if any.
    pub fn unit(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Fixed { unit, .. } | Self::FromArgument { unit, .. } => unit.as_deref(),
        }
    }
}

/// One argument row. Omitted defaults are absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlArgField {
    pub name: String,
    pub kind: IdlArgKind,
    /// An empty value keeps the IDL default instead of being encoded.
    pub blank_uses_default: bool,
    /// How an integer is typed. [`IdlArgScale::None`] for anything else.
    pub scale: IdlArgScale,
}

/// The form for one IDL instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlForm {
    pub program_id: Pubkey,
    pub instruction: String,
    pub accounts: Vec<IdlAccountField>,
    pub arguments: Vec<IdlArgField>,
}

/// Why an IDL instruction was not staged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdlBuildFailure {
    Message(String),
    /// Derivation failed for this account. Other accounts stay as they are.
    FillAddress {
        account: String,
        reason: String,
    },
}

impl std::fmt::Display for IdlBuildFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::FillAddress { account, reason } => {
                write!(f, "{account}: fill this address · {reason}")
            }
        }
    }
}

impl std::error::Error for IdlBuildFailure {}
