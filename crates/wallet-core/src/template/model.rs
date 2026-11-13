//! A template as read from its file.

use solana_pubkey::Pubkey;
use std::collections::BTreeMap;

/// The address the selected wallet acts as.
pub const WALLET: &str = "wallet";
/// Build's fee payer field.
pub const FEE_PAYER: &str = "fee-payer";
/// The names every template has without declaring them.
pub const BUILT_IN_NAMES: [&str; 2] = [WALLET, FEE_PAYER];

/// A template: questions, derived values and the IDL blocks they fill in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub summary: Option<String>,
    /// Program aliases, by name.
    pub programs: BTreeMap<String, Pubkey>,
    /// Checkboxes, in the order written.
    pub options: Vec<TemplateOption>,
    /// Questions, in the order written.
    pub inputs: Vec<Input>,
    /// Derived values, by name. They are worked out in the order they need
    /// each other, not the order written.
    pub values: BTreeMap<String, Call>,
    /// Instructions, in the order they run.
    pub blocks: Vec<Block>,
}

/// A checkbox that turns blocks and inputs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateOption {
    pub name: String,
    pub label: String,
    pub hint: Option<String>,
    /// On when the form opens.
    pub default: bool,
}

/// A question the form asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub name: String,
    pub label: String,
    pub hint: Option<String>,
    /// May be left blank, which leaves its fields blank.
    pub optional: bool,
    /// What it holds until it is answered: a literal, a built-in name or a
    /// program alias.
    pub default: Option<Term>,
    /// Asked only while this option is on.
    pub option: Option<String>,
}

/// A string in a template: a reference to a name (`$name`) or a literal,
/// written as it would be typed into the field it goes to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Term {
    Ref(String),
    Literal(String),
}

impl Term {
    /// `$name` is a reference, `$$` a literal dollar, anything else a
    /// literal. `None` for a `$` with no name after it.
    pub fn read(text: &str) -> Option<Self> {
        if let Some(rest) = text.strip_prefix("$$") {
            return Some(Self::Literal(format!("${rest}")));
        }
        match text.strip_prefix('$') {
            Some("") => None,
            Some(name) => Some(Self::Ref(name.to_string())),
            None => Some(Self::Literal(text.to_string())),
        }
    }

    /// The text [`Term::read`] reads back to this term.
    pub fn text(&self) -> String {
        match self {
            Self::Ref(name) => format!("${name}"),
            Self::Literal(text) if text.starts_with('$') => format!("${text}"),
            Self::Literal(text) => text.clone(),
        }
    }

    pub fn reference(&self) -> Option<&str> {
        match self {
            Self::Ref(name) => Some(name),
            Self::Literal(_) => None,
        }
    }
}

/// A function from the fixed list, and what it is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub function: Function,
    pub args: Vec<Term>,
}

/// The functions a template can name. There are no others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Function {
    /// The address of a keypair the caller makes for this form.
    NewKeypair,
    /// `<owner> <mint> <token program>`: the associated token address.
    AssociatedTokenAccount,
    /// `<mint>`: the size to create a Token-2022 mint at.
    Token2022MintSpace,
    /// `<mint>`: the size to fund a Token-2022 mint for, token metadata
    /// included.
    Token2022MintFundedSpace,
    /// `<bytes>`: the node's rent for that size, in lamports.
    RentFor,
    /// `<mint>`: the mint's decimals, read from the node.
    DecimalsOf,
    /// `<mint>`: Token or Token-2022, whichever owns the mint.
    TokenProgramOf,
    /// `<program>`: the program data account the upgradeable loader keeps
    /// the program's code in.
    ProgramDataOf,
    /// `<program>`: the program's canonical IDL account on the
    /// program-metadata program, seed `idl`.
    IdlMetadataOf,
    /// `<mint>`: the mint's metadata account on Metaplex Token Metadata.
    MetadataOf,
    /// `<mint>`: the mint's master edition account on Metaplex Token
    /// Metadata.
    MasterEditionOf,
    /// `<amount> <decimals>`: an amount typed in a token's units, in base
    /// units.
    BaseUnits,
    /// `<address>`: a Metaplex creator list naming only `<address>`, with
    /// the whole share, verified, as JSON.
    SoleCreator,
    /// `<collection>`: a Metaplex collection naming `<collection>`, not
    /// verified, as JSON.
    UnverifiedCollection,
    /// `<stream>`: the token account a Streamflow stream holds its tokens
    /// in.
    StreamEscrowOf,
    /// `<tree>`: the config account Bubblegum keeps for a merkle tree.
    TreeConfigOf,
    /// `<shape>`: the depth of a merkle tree of that shape.
    TreeDepth,
    /// `<shape>`: the buffer size of a merkle tree of that shape.
    TreeBuffer,
    /// `<shape> <canopy>`: the bytes a merkle tree account of that shape
    /// and canopy depth takes on MPL Account Compression.
    MerkleTreeSpace,
    /// `<a> <b>`: whichever address sorts first, byte for byte.
    LesserAddress,
    /// `<a> <b>`: whichever address sorts last, byte for byte.
    GreaterAddress,
    /// `<a> <b> <when a is first> <when b is first>`: one of the two numbers,
    /// chosen by which address sorts first.
    WhenLesser,
}

/// What a value or an argument is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ValueType {
    Address,
    /// A program address: Token or Token-2022, or an alias.
    Program,
    /// An integer in base units.
    Integer,
    /// A number typed in a token's units, as an input: a parameter only.
    Amount,
    /// A value written as JSON into a field made of fields: a result only.
    Json,
    /// One of the merkle tree shapes [`super::tree::TREE_SHAPES`] names, as
    /// an input: a parameter only.
    Shape,
}

impl Function {
    pub const ALL: [Function; 22] = [
        Self::NewKeypair,
        Self::AssociatedTokenAccount,
        Self::Token2022MintSpace,
        Self::Token2022MintFundedSpace,
        Self::RentFor,
        Self::DecimalsOf,
        Self::TokenProgramOf,
        Self::ProgramDataOf,
        Self::IdlMetadataOf,
        Self::MetadataOf,
        Self::MasterEditionOf,
        Self::BaseUnits,
        Self::SoleCreator,
        Self::UnverifiedCollection,
        Self::StreamEscrowOf,
        Self::TreeConfigOf,
        Self::TreeDepth,
        Self::TreeBuffer,
        Self::MerkleTreeSpace,
        Self::LesserAddress,
        Self::GreaterAddress,
        Self::WhenLesser,
    ];

    /// The name a file writes it by.
    pub fn name(self) -> &'static str {
        match self {
            Self::NewKeypair => "new-keypair",
            Self::AssociatedTokenAccount => "associated-token-account",
            Self::Token2022MintSpace => "token-2022-mint-space",
            Self::Token2022MintFundedSpace => "token-2022-mint-funded-space",
            Self::RentFor => "rent-for",
            Self::DecimalsOf => "decimals-of",
            Self::TokenProgramOf => "token-program-of",
            Self::ProgramDataOf => "program-data-of",
            Self::IdlMetadataOf => "idl-metadata-of",
            Self::MetadataOf => "metadata-of",
            Self::MasterEditionOf => "master-edition-of",
            Self::BaseUnits => "base-units",
            Self::SoleCreator => "sole-creator",
            Self::UnverifiedCollection => "unverified-collection",
            Self::StreamEscrowOf => "stream-escrow-of",
            Self::TreeConfigOf => "tree-config-of",
            Self::TreeDepth => "tree-depth",
            Self::TreeBuffer => "tree-buffer",
            Self::MerkleTreeSpace => "merkle-tree-space",
            Self::LesserAddress => "lesser-address",
            Self::GreaterAddress => "greater-address",
            Self::WhenLesser => "when-lesser",
        }
    }

    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|function| function.name() == name)
    }

    /// What each argument is.
    pub(crate) fn params(self) -> &'static [ValueType] {
        use ValueType::*;
        match self {
            Self::NewKeypair => &[],
            Self::AssociatedTokenAccount => &[Address, Address, Address],
            Self::Token2022MintSpace
            | Self::Token2022MintFundedSpace
            | Self::DecimalsOf
            | Self::TokenProgramOf
            | Self::ProgramDataOf
            | Self::IdlMetadataOf
            | Self::MetadataOf
            | Self::MasterEditionOf
            | Self::SoleCreator
            | Self::UnverifiedCollection
            | Self::StreamEscrowOf
            | Self::TreeConfigOf => &[Address],
            Self::RentFor => &[Integer],
            Self::BaseUnits => &[Amount, Integer],
            Self::TreeDepth | Self::TreeBuffer => &[Shape],
            Self::MerkleTreeSpace => &[Shape, Integer],
            Self::LesserAddress | Self::GreaterAddress => &[Address, Address],
            Self::WhenLesser => &[Address, Address, Integer, Integer],
        }
    }

    /// The arguments, as a file writes them after the name.
    pub(crate) fn usage(self) -> &'static str {
        match self {
            Self::NewKeypair => "no arguments",
            Self::AssociatedTokenAccount => "an owner, a mint and a token program",
            Self::Token2022MintSpace | Self::Token2022MintFundedSpace => "a mint",
            Self::RentFor => "a size in bytes",
            Self::DecimalsOf | Self::TokenProgramOf => "a mint",
            Self::ProgramDataOf | Self::IdlMetadataOf => "a program",
            Self::MetadataOf | Self::MasterEditionOf => "a mint",
            Self::BaseUnits => "an amount in a token's units and its decimals",
            Self::SoleCreator => "an address",
            Self::UnverifiedCollection => "a collection",
            Self::StreamEscrowOf => "a stream",
            Self::TreeConfigOf => "a merkle tree",
            Self::TreeDepth | Self::TreeBuffer => "a tree shape",
            Self::MerkleTreeSpace => "a tree shape and a canopy depth",
            Self::LesserAddress | Self::GreaterAddress => "two addresses",
            Self::WhenLesser => "two addresses and the two numbers to choose between",
        }
    }

    pub(crate) fn returns(self) -> ValueType {
        match self {
            Self::NewKeypair
            | Self::AssociatedTokenAccount
            | Self::ProgramDataOf
            | Self::IdlMetadataOf
            | Self::MetadataOf
            | Self::MasterEditionOf
            | Self::StreamEscrowOf
            | Self::TreeConfigOf
            | Self::LesserAddress
            | Self::GreaterAddress => ValueType::Address,
            Self::Token2022MintSpace
            | Self::Token2022MintFundedSpace
            | Self::RentFor
            | Self::DecimalsOf
            | Self::BaseUnits
            | Self::TreeDepth
            | Self::TreeBuffer
            | Self::MerkleTreeSpace
            | Self::WhenLesser => ValueType::Integer,
            Self::TokenProgramOf => ValueType::Program,
            Self::SoleCreator | Self::UnverifiedCollection => ValueType::Json,
        }
    }

    /// The JSON a function of [`ValueType::Json`] writes for `address`.
    pub(crate) fn json_for(self, address: &Pubkey) -> Option<String> {
        match self {
            Self::SoleCreator => Some(format!(
                r#"[{{"address":"{address}","verified":true,"share":100}}]"#
            )),
            Self::UnverifiedCollection => {
                Some(format!(r#"{{"verified":false,"key":"{address}"}}"#))
            }
            _ => None,
        }
    }
}

/// One instruction the template writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The block exists only while this option is on.
    pub when: Option<String>,
    /// A program alias, or a value of `token-program-of`.
    pub program: String,
    pub instruction: String,
    pub accounts: BTreeMap<String, Term>,
    pub arguments: BTreeMap<String, Term>,
}

impl Block {
    /// What the template binds to `field`, an account or an argument.
    pub fn binding(&self, field: &str) -> Option<&Term> {
        self.accounts
            .get(field)
            .or_else(|| self.arguments.get(field))
    }
}

impl Template {
    pub fn input(&self, name: &str) -> Option<&Input> {
        self.inputs.iter().find(|input| input.name == name)
    }

    pub fn option(&self, name: &str) -> Option<&TemplateOption> {
        self.options.iter().find(|option| option.name == name)
    }

    /// The values the caller makes a keypair for, by name.
    pub fn keypair_values(&self) -> Vec<&str> {
        self.values
            .iter()
            .filter(|(_, call)| call.function == Function::NewKeypair)
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// The options on when the form opens.
    pub fn default_options(&self) -> Vec<&str> {
        self.options
            .iter()
            .filter(|option| option.default)
            .map(|option| option.name.as_str())
            .collect()
    }

    /// Every name a reference can reach, so `$name` is one of these.
    pub(crate) fn names_value(&self, name: &str) -> bool {
        BUILT_IN_NAMES.contains(&name)
            || self.programs.contains_key(name)
            || self.values.contains_key(name)
            || self.input(name).is_some()
    }

    /// The values in an order where each comes after those it names.
    pub(crate) fn value_order(&self) -> Vec<&str> {
        let mut order = Vec::new();
        let mut done = std::collections::BTreeSet::new();
        fn visit<'a>(
            template: &'a Template,
            name: &'a str,
            done: &mut std::collections::BTreeSet<&'a str>,
            order: &mut Vec<&'a str>,
        ) {
            if !done.insert(name) {
                return;
            }
            if let Some(call) = template.values.get(name) {
                for arg in &call.args {
                    if let Some(next) = arg.reference() {
                        if template.values.contains_key(next) {
                            visit(template, next, done, order);
                        }
                    }
                }
            }
            order.push(name);
        }
        for name in self.values.keys() {
            visit(self, name, &mut done, &mut order);
        }
        order
    }
}
