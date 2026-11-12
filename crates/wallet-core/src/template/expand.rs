//! Turning a checked template and its answers into IDL blocks.
//!
//! Expansion never fails on a blank or unreadable answer: the field it fills
//! is left blank, so a half-filled form can still be shown as blocks. What
//! is still needed comes back with the blocks: the inputs nobody answered,
//! and the chain facts the node has to supply.

use super::check::{analyze, candidates, InputScale, Problem};
use super::model::*;
use super::size::{metadata_len, mint_len, sizing, Sizing};
use crate::idl::build::{field_decimals, IdlAccountSource, IdlArgScale};
use crate::idl::catalog::IdlCatalog;
use crate::idl::draft::IdlDraft;
use crate::ids::TOKEN_2022_PROGRAM_ID;
use crate::spl::associated_token_address;
use crate::units::format_base_digits;
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;

/// What the form holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers {
    /// The text of each answered input, as typed into its field: in the
    /// field's unit for a scaled amount. An input with no entry takes its
    /// default; one with an empty entry was cleared and stays blank.
    pub inputs: BTreeMap<String, String>,
    /// The options that are on.
    pub options: BTreeSet<String>,
}

impl Answers {
    /// Nothing answered, and the options that are on by default.
    pub fn for_template(template: &Template) -> Self {
        Self {
            inputs: BTreeMap::new(),
            options: template
                .default_options()
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }

    pub fn answer(mut self, input: &str, text: &str) -> Self {
        self.inputs.insert(input.to_string(), text.to_string());
        self
    }

    pub fn turn_on(mut self, option: &str) -> Self {
        self.options.insert(option.to_string());
        self
    }

    pub fn is_on(&self, option: &str) -> bool {
        self.options.contains(option)
    }
}

/// What the caller knows that the template does not. The caller makes and
/// keeps the keypairs: only their addresses come here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Env {
    /// `$wallet`: the address the selected wallet acts as.
    pub wallet: Option<Pubkey>,
    /// `$fee-payer`: Build's fee payer.
    pub fee_payer: Option<Pubkey>,
    /// The address of the keypair made for each `new-keypair` value.
    pub keypairs: BTreeMap<String, Pubkey>,
}

impl Template {
    /// What input `input` holds until it is answered: its default, with a
    /// built-in name or a program alias resolved against `env`. `None` for
    /// an input with no default, or one whose default is not known yet (no
    /// wallet selected for `$wallet`).
    pub fn default_text(&self, input: &str, env: &Env) -> Option<String> {
        let name = match self.input(input)?.default.as_ref()? {
            Term::Literal(text) => return Some(text.clone()),
            Term::Ref(name) => name.as_str(),
        };
        let address = match name {
            WALLET => env.wallet,
            FEE_PAYER => env.fee_payer,
            alias => self.programs.get(alias).copied(),
        };
        address.map(|address| address.to_string())
    }
}

/// Something only the node can say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fact {
    /// The lamports an account of this many bytes has to hold.
    Rent(u64),
    /// A mint's decimals.
    MintDecimals(Pubkey),
    /// The program that owns a mint: Token or Token-2022.
    TokenProgram(Pubkey),
}

impl std::fmt::Display for Fact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rent(bytes) => write!(f, "Rent for {bytes} bytes, in lamports"),
            Self::MintDecimals(mint) => write!(f, "Decimals of the mint {mint}"),
            Self::TokenProgram(mint) => write!(f, "Token program of the mint {mint}"),
        }
    }
}

/// The answers to chain facts, from the node or typed by hand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// Lamports by account size.
    pub rent: BTreeMap<u64, u64>,
    /// Decimals by mint.
    pub decimals: BTreeMap<Pubkey, u8>,
    /// Owning token program by mint.
    pub token_programs: BTreeMap<Pubkey, Pubkey>,
}

impl Facts {
    /// Whether `fact` is answered.
    pub fn knows(&self, fact: &Fact) -> bool {
        match fact {
            Fact::Rent(bytes) => self.rent.contains_key(bytes),
            Fact::MintDecimals(mint) => self.decimals.contains_key(mint),
            Fact::TokenProgram(mint) => self.token_programs.contains_key(mint),
        }
    }

    /// Adds `other`'s answers, which win over ones already here.
    pub fn merge(&mut self, other: Facts) {
        self.rent.extend(other.rent);
        self.decimals.extend(other.decimals);
        self.token_programs.extend(other.token_programs);
    }
}

/// Something the blocks cannot be built without.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    /// A required input with no answer.
    Input(String),
    /// `$wallet` is used and no wallet is selected.
    Wallet,
    /// `$fee-payer` is used and Build has no fee payer.
    FeePayer,
    /// The caller has not made the keypair of this `new-keypair` value.
    Keypair(String),
}

/// A field the template filled itself, with what it put there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The account or argument.
    pub field: String,
    /// As written into the field. Empty while it cannot be worked out.
    pub text: String,
    /// The value, built-in name or program alias it came from. `None` for a
    /// literal.
    pub from: Option<String>,
}

/// One block of the expansion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandedBlock {
    /// The block's number in the template, from 1.
    pub number: usize,
    /// Its program is `None` while it is a `token-program-of` that is not
    /// known yet; its fields are then laid out as Token's.
    pub draft: IdlDraft,
    /// Fields the template sets to a literal or a program alias.
    pub fixed: Vec<Row>,
    /// Fields the template works out from a value or a built-in name.
    pub worked_out: Vec<Row>,
}

/// A template written out as blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    /// The blocks whose option is on, or that have none, in order.
    pub blocks: Vec<ExpandedBlock>,
    pub missing: Vec<Missing>,
    /// Chain facts still needed. A fact whose argument is not known yet is
    /// not needed yet.
    pub needs: Vec<Fact>,
    /// Every chain fact the blocks rest on, known or not: `needs` and the
    /// facts already answered. A form that asks for facts by hand keeps a
    /// field for each of these, so a field does not go once it is filled.
    pub uses: Vec<Fact>,
}

impl Expansion {
    /// Nothing missing and no fact needed: the blocks build.
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.needs.is_empty()
    }

    /// The decimal places an input of this scale is typed with, once known.
    pub fn decimals_of(&self, scale: &InputScale) -> Option<u8> {
        match scale {
            InputScale::None => None,
            InputScale::Fixed { decimals, .. } => Some(*decimals),
            InputScale::FromArgument {
                block, argument, ..
            } => self
                .blocks
                .iter()
                .find(|expanded| expanded.number == *block)?
                .draft
                .argument(argument)
                .trim()
                .parse()
                .ok(),
        }
    }
}

/// A value as the template works it out.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Val {
    Address(Pubkey),
    /// In base units.
    Integer(u128),
    /// An answer or a literal, as typed.
    Text(String),
}

impl Val {
    fn address(&self) -> Option<Pubkey> {
        match self {
            Self::Address(address) => Some(*address),
            Self::Text(text) => Pubkey::from_str(text.trim()).ok(),
            Self::Integer(_) => None,
        }
    }

    fn integer(&self) -> Option<u128> {
        match self {
            Self::Integer(n) => Some(*n),
            Self::Text(text) => text.trim().parse().ok(),
            Self::Address(_) => None,
        }
    }
}

struct Run<'a> {
    template: &'a Template,
    answers: &'a Answers,
    env: &'a Env,
    facts: &'a Facts,
    values: BTreeMap<&'a str, Option<Val>>,
    missing: Vec<Missing>,
    needs: Vec<Fact>,
    uses: Vec<Fact>,
}

/// Writes `template` out as blocks from `answers`. Fails only when the
/// template does not check against `idls`.
pub fn expand(
    template: &Template,
    answers: &Answers,
    env: &Env,
    facts: &Facts,
    idls: &dyn IdlCatalog,
) -> Result<Expansion, Vec<Problem>> {
    let checked = analyze(template, idls)?;
    let mut run = Run {
        template,
        answers,
        env,
        facts,
        values: BTreeMap::new(),
        missing: Vec::new(),
        needs: Vec::new(),
        uses: Vec::new(),
    };

    for input in &template.inputs {
        if !input.optional && run.active(input) && run.input_text(input).is_none() {
            run.miss(Missing::Input(input.name.clone()));
        }
    }

    let enabled: Vec<(usize, &Block)> = template
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| run.enabled(block))
        .collect();
    let live = live_values(template, enabled.iter().map(|(_, block)| *block));
    for name in template.value_order() {
        if live.contains(name) {
            let value = run.evaluate(name);
            run.values.insert(name, value);
        }
    }

    let mut blocks = Vec::new();
    for (index, block) in enabled {
        let program = run
            .term(&Term::Ref(block.program.clone()))
            .and_then(|v| v.address());
        let idls = &checked.blocks[index];
        let Some(idl) = idls
            .iter()
            .find(|idl| Some(idl.program_id) == program)
            .or_else(|| idls.first())
        else {
            continue;
        };
        let mut draft = IdlDraft::for_instruction(
            idl.program_id,
            &idl.program_name,
            &idl.client,
            Some(&block.instruction),
        );
        if program != Some(idl.program_id) {
            draft.program_id = None;
            draft.program_name = String::new();
        }
        let mut fixed = Vec::new();
        let mut worked_out = Vec::new();
        let mut row = |field: &str, term: &Term, text: &str| {
            let row = |from| Row {
                field: field.to_string(),
                text: text.to_string(),
                from,
            };
            match term {
                Term::Literal(_) => fixed.push(row(None)),
                Term::Ref(name) if template.input(name).is_some() => {}
                Term::Ref(name) if template.programs.contains_key(name) => {
                    fixed.push(row(Some(name.clone())))
                }
                Term::Ref(name) => worked_out.push(row(Some(name.clone()))),
            }
        };

        for (name, term) in &block.accounts {
            let address = run.term(term).and_then(|value| value.address());
            let derived = idl
                .form
                .accounts
                .iter()
                .find(|field| field.name == *name)
                .is_some_and(|field| field.source == IdlAccountSource::Derived);
            if derived {
                // As Build's "Change": the form asks for it instead of
                // showing the address the IDL would derive. An account the
                // IDL cannot fill is asked for anyway.
                draft.set_manual(name, true);
            }
            draft.set_account(name, address);
            row(
                name,
                term,
                &address.map(|a| a.to_string()).unwrap_or_default(),
            );
        }

        // An amount scaled by another argument is written once that
        // argument is.
        let fields = &idl.form.arguments;
        let scaled_later = |name: &str| {
            fields.iter().any(|field| {
                field.name == name && matches!(field.scale, IdlArgScale::FromArgument { .. })
            })
        };
        let order = block
            .arguments
            .iter()
            .filter(|(name, _)| !scaled_later(name))
            .chain(
                block
                    .arguments
                    .iter()
                    .filter(|(name, _)| scaled_later(name)),
            );
        for (name, term) in order {
            let Some(field) = fields.iter().find(|field| field.name == *name) else {
                continue;
            };
            let text = match (term, run.term(term)) {
                (_, None) => String::new(),
                (Term::Literal(text), _) => text.clone(),
                (_, Some(Val::Text(text))) => text,
                (_, Some(Val::Address(address))) => address.to_string(),
                (_, Some(Val::Integer(n))) => {
                    let typed: HashMap<String, String> = draft.arguments.iter().cloned().collect();
                    match field_decimals(field, &typed) {
                        Ok(None) => n.to_string(),
                        Ok(Some(decimals)) => format_base_digits(&n.to_string(), decimals),
                        Err(_) => String::new(),
                    }
                }
            };
            draft.set_argument(name, text.clone());
            row(name, term, &text);
        }

        blocks.push(ExpandedBlock {
            number: index + 1,
            draft,
            fixed,
            worked_out,
        });
    }

    Ok(Expansion {
        blocks,
        missing: run.missing,
        needs: run.needs,
        uses: run.uses,
    })
}

/// The values the enabled blocks reach, directly or through other values.
fn live_values<'a>(
    template: &'a Template,
    blocks: impl Iterator<Item = &'a Block>,
) -> BTreeSet<&'a str> {
    let mut stack: Vec<&str> = Vec::new();
    for block in blocks {
        stack.push(&block.program);
        stack.extend(
            block
                .accounts
                .values()
                .chain(block.arguments.values())
                .filter_map(Term::reference),
        );
    }
    let mut live = BTreeSet::new();
    while let Some(name) = stack.pop() {
        if let Some(call) = template.values.get(name) {
            if live.insert(name) {
                stack.extend(call.args.iter().filter_map(Term::reference));
            }
        }
    }
    live
}

impl<'a> Run<'a> {
    fn miss(&mut self, missing: Missing) {
        if !self.missing.contains(&missing) {
            self.missing.push(missing);
        }
    }

    /// Notes that the blocks rest on `fact`, and that it is still needed
    /// when it is not known.
    fn rest_on(&mut self, fact: Fact) {
        if !self.uses.contains(&fact) {
            self.uses.push(fact);
        }
        if !self.facts.knows(&fact) && !self.needs.contains(&fact) {
            self.needs.push(fact);
        }
    }

    fn enabled(&self, block: &Block) -> bool {
        block
            .when
            .as_ref()
            .is_none_or(|option| self.answers.is_on(option))
    }

    fn active(&self, input: &Input) -> bool {
        input
            .option
            .as_ref()
            .is_none_or(|option| self.answers.is_on(option))
    }

    /// An input's text: its answer, or its default when it has none. `None`
    /// when blank, or asked under an option that is off.
    fn input_text(&mut self, input: &Input) -> Option<String> {
        if !self.active(input) {
            return None;
        }
        let text = match self.answers.inputs.get(&input.name) {
            Some(text) => Some(text.clone()),
            None => match &input.default {
                None => None,
                Some(Term::Literal(text)) => Some(text.clone()),
                Some(term) => self
                    .term(term)
                    .and_then(|v| v.address())
                    .map(|a| a.to_string()),
            },
        };
        text.filter(|text| !text.trim().is_empty())
    }

    fn term(&mut self, term: &Term) -> Option<Val> {
        let name = match term {
            Term::Literal(text) => return Some(Val::Text(text.clone())),
            Term::Ref(name) => name.as_str(),
        };
        if name == WALLET {
            let wallet = self.env.wallet;
            if wallet.is_none() {
                self.miss(Missing::Wallet);
            }
            return wallet.map(Val::Address);
        }
        if name == FEE_PAYER {
            let payer = self.env.fee_payer;
            if payer.is_none() {
                self.miss(Missing::FeePayer);
            }
            return payer.map(Val::Address);
        }
        if let Some(address) = self.template.programs.get(name) {
            return Some(Val::Address(*address));
        }
        if let Some(input) = self.template.input(name) {
            return self.input_text(input).map(Val::Text);
        }
        self.values.get(name).cloned().flatten()
    }

    fn evaluate(&mut self, name: &'a str) -> Option<Val> {
        let call = self.template.values.get(name)?;
        let args: Vec<Option<Val>> = call.args.iter().map(|arg| self.term(arg)).collect();
        let address = |at: usize| args.get(at)?.as_ref()?.address();
        match call.function {
            Function::NewKeypair => {
                let address = self.env.keypairs.get(name).copied();
                if address.is_none() {
                    self.miss(Missing::Keypair(name.to_string()));
                }
                address.map(Val::Address)
            }
            Function::AssociatedTokenAccount => {
                let (owner, mint, program) = (address(0)?, address(1)?, address(2)?);
                Some(Val::Address(associated_token_address(
                    &owner, &mint, &program,
                )))
            }
            Function::Token2022MintSpace => self.mint_size(&call.args[0], false),
            Function::Token2022MintFundedSpace => self.mint_size(&call.args[0], true),
            Function::RentFor => {
                let bytes = u64::try_from(args.first()?.as_ref()?.integer()?).ok()?;
                self.rest_on(Fact::Rent(bytes));
                let lamports = self.facts.rent.get(&bytes)?;
                Some(Val::Integer(u128::from(*lamports)))
            }
            Function::DecimalsOf => {
                let mint = address(0)?;
                self.rest_on(Fact::MintDecimals(mint));
                let decimals = self.facts.decimals.get(&mint)?;
                Some(Val::Integer(u128::from(*decimals)))
            }
            Function::TokenProgramOf => {
                let mint = address(0)?;
                self.rest_on(Fact::TokenProgram(mint));
                let program = self.facts.token_programs.get(&mint)?;
                Some(Val::Address(*program))
            }
        }
    }

    /// The size of the Token-2022 mint bound as `mint`: the extensions the
    /// enabled Token-2022 blocks of this template set up on it, each once,
    /// and with `funded`, the token metadata written to the mint itself.
    /// `None` while the metadata's text is not known.
    fn mint_size(&mut self, mint: &Term, funded: bool) -> Option<Val> {
        let template = self.template;
        let mut seen = BTreeSet::new();
        let mut extensions = Vec::new();
        for block in &template.blocks {
            let on_token_2022 = candidates(template, block) == [TOKEN_2022_PROGRAM_ID];
            if !self.enabled(block)
                || !on_token_2022
                || block.accounts.get("mint") != Some(mint)
                || !seen.insert(block.instruction.as_str())
            {
                continue;
            }
            match sizing(&block.instruction) {
                Sizing::Fixed(len) => extensions.push(len),
                Sizing::Metadata if funded && block.accounts.get("metadata") == Some(mint) => {
                    let mut text = |field: &str| {
                        let term = block.arguments.get(field)?;
                        match self.term(term)? {
                            Val::Text(text) => Some(text.trim().to_string()),
                            _ => None,
                        }
                    };
                    let (name, symbol, uri) = (text("name")?, text("symbol")?, text("uri")?);
                    extensions.push(metadata_len(&name, &symbol, &uri));
                }
                _ => {}
            }
        }
        Some(Val::Integer(mint_len(&extensions) as u128))
    }
}
