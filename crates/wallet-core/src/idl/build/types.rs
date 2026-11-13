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
    /// A list of any length, typed as a JSON array of its items: strings
    /// for text and addresses, numbers or strings for integers, `true` and
    /// `false`. Blank is the empty list. Only text, integers, addresses and
    /// booleans are items.
    List(Box<IdlArgKind>),
    Bytes,
    /// A fixed number of bytes, such as a name kept in `[u8; 64]`: typed as
    /// text, kept as its UTF-8 bytes followed by zeros, or as hex after
    /// `0x`, followed by zeros too. Blank is all zeros.
    FixedBytes {
        len: usize,
    },
    /// A struct inside a list, an option or another struct: typed as a JSON
    /// object with a key for each field, each written as the item of a list
    /// of its kind would be. Every field is needed.
    Struct {
        fields: Vec<(String, IdlArgKind)>,
    },
    /// An enum some of whose variants hold fields. A variant that holds
    /// nothing is typed by its name, as a choice is; one that holds fields
    /// as a JSON object with the variant as its one key: `{"limited": 5}`
    /// for a tuple of one item, `{"pair": [1, 2]}` for more, `{"v1":
    /// {"size": 0}}` for a struct.
    Variants {
        variants: Vec<(String, IdlVariant)>,
    },
    /// A Codama type this form does not have a widget for. The text is rejected
    /// at build time with `note`.
    Raw {
        note: String,
    },
}

/// What one variant of [`IdlArgKind::Variants`] holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdlVariant {
    Empty,
    Tuple(Vec<IdlArgKind>),
    Struct(Vec<(String, IdlArgKind)>),
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

impl IdlArgField {
    /// Whether the field may be left blank: it then takes the IDL default,
    /// none for an option, or the empty list.
    pub fn may_be_blank(&self) -> bool {
        self.blank_uses_default
            || matches!(
                self.kind,
                IdlArgKind::Option(_) | IdlArgKind::List(_) | IdlArgKind::FixedBytes { .. }
            )
    }
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
///
/// An argument whose type is a struct is one row per field of the struct,
/// named `argument.field` ([`STRUCT_FIELD_SEPARATOR`]), and the build puts
/// the struct back together from them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlArgField {
    pub name: String,
    pub kind: IdlArgKind,
    /// An empty value keeps the IDL default instead of being encoded.
    pub blank_uses_default: bool,
    /// How an integer is typed. [`IdlArgScale::None`] for anything else.
    pub scale: IdlArgScale,
    /// An option the program reads only if bytes are left (Codama's
    /// `remainderOptionTypeNode`): none is written as nothing at all. Of
    /// several such options at the end of an instruction, one left blank
    /// before one that is filled would have the later one's bytes read as
    /// its own, so the build refuses that ([`trailing_option_gap`]).
    pub remainder_option: bool,
}

/// Of `form`'s trailing options ([`IdlArgField::remainder_option`]), the
/// first one left blank while a later one holds a value, and that later
/// one: `(blank, filled)`. The program reads such options in order and
/// stops at the end of the bytes, so the later value would be read as the
/// blank one. `None` when every filled one follows only filled ones.
pub fn trailing_option_gap<'a>(
    arguments: &'a [IdlArgField],
    input: &IdlBuildInput,
) -> Option<(&'a str, &'a str)> {
    let text = |name: &str| {
        input
            .arguments
            .iter()
            .find(|(entry, _)| entry == name)
            .map_or("", |(_, value)| value.trim())
    };
    let mut blank: Option<&str> = None;
    for field in arguments.iter().filter(|field| field.remainder_option) {
        match (text(&field.name).is_empty(), blank) {
            (true, None) => blank = Some(&field.name),
            (false, Some(first)) => return Some((first, &field.name)),
            _ => {}
        }
    }
    None
}

/// What the build says when [`trailing_option_gap`] finds one: the field
/// to fill first, and why, in words a person can act on.
pub fn trailing_option_message(blank: &str, filled: &str) -> String {
    format!(
        "Fill {blank} first, or clear {filled}. The program reads these values in order and stops where the data ends, so with {blank} blank it would read {filled} as {blank}."
    )
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
