//! A change to the multisig itself, stored under a transaction number like a
//! vault transaction and voted on the same way.
//!
//! v4 calls it a *config transaction* (`configTransaction`, a list of
//! `configAction`), the smart account program a *settings transaction*
//! (`settingsTransaction`, a list of `settingsAction`). Both are read here
//! from the IDLs the app ships, into one list of typed actions, and built
//! back from it: storing one (`configTransactionCreate` /
//! `createSettingsTransaction`) and executing it (`configTransactionExecute`
//! / `executeSettingsTransaction`).
//!
//! The actions run in order, on the multisig as it is when executed, and
//! the program then refuses the whole change unless the result is a valid
//! multisig (see `change_words::change_outcome`).
//!
//! The smart account program has actions about policies, which this app
//! reads by name and fields but does not execute, and one that sets an
//! archival authority, which the program itself refuses to run today.

use super::*;

/// How often a spending limit's amount comes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    /// Once, and never again.
    OneTime,
    Day,
    /// Seven days.
    Week,
    /// Thirty days.
    Month,
}

// An enum without fields, such as this one, keeps the IDL's own variant
// names in the dynamic client ("oneTime"); one with fields has them
// capitalised ("AddMember").
impl Period {
    fn of(value: &Value) -> Option<Self> {
        match value {
            Value::Enum { variant, .. } => match variant.as_str() {
                "oneTime" | "OneTime" => Some(Self::OneTime),
                "day" | "Day" => Some(Self::Day),
                "week" | "Week" => Some(Self::Week),
                "month" | "Month" => Some(Self::Month),
                _ => None,
            },
            _ => None,
        }
    }

    fn value(self) -> Value {
        Value::unit_enum(match self {
            Self::OneTime => "oneTime",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        })
    }
}

/// A spending limit a change creates: some members may move up to `amount`
/// of `mint` out of a vault per `period` without a vote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpendingLimit {
    /// The key its account is derived from (v4 `createKey`, smart account
    /// `seed`).
    pub seed: Pubkey,
    pub vault_index: u8,
    /// The default address for SOL.
    pub mint: Pubkey,
    /// In the mint's smallest unit (lamports for SOL).
    pub amount: u64,
    pub period: Period,
    /// Who may use it.
    pub members: Vec<Pubkey>,
    /// Where it may send to. Empty for anywhere.
    pub destinations: Vec<Pubkey>,
    /// Smart account only: when it stops working, in Unix seconds.
    /// `i64::MAX` for never. `None` on v4, whose limits do not expire.
    pub expiration: Option<i64>,
}

/// What a policy action of the smart account program does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyVerb {
    Create,
    Update,
    Remove,
}

/// One action of a change to the multisig.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigAction {
    AddMember(SquadsMember),
    RemoveMember(Pubkey),
    ChangeThreshold(u16),
    /// Seconds.
    SetTimeLock(u32),
    AddSpendingLimit(SpendingLimit),
    /// The spending limit's account.
    RemoveSpendingLimit(Pubkey),
    /// v4 only. `None` turns closing transaction accounts off.
    SetRentCollector(Option<Pubkey>),
    /// Smart account only. The program refuses to execute it today
    /// (`NotImplemented`).
    SetArchivalAuthority(Option<Pubkey>),
    /// Smart account only: a policy created, updated or removed. Read by
    /// name and fields, not executed by this app.
    Policy {
        verb: PolicyVerb,
        /// The policy's account, for an update or a removal.
        policy: Option<Pubkey>,
        /// Every field of the action, as text, in the IDL's order.
        fields: Vec<(String, String)>,
    },
}

/// A change to a multisig, read from its transaction account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigTransaction {
    /// The program it belongs to, which decides a few of the rules.
    pub program: SquadsProgram,
    pub creator: Pubkey,
    pub index: u64,
    /// In order: each runs on what the ones before it left.
    pub actions: Vec<ConfigAction>,
}

impl ConfigTransaction {
    /// Why this app does not build its execution, or `None` when it does.
    /// The program may still refuse it: see `change_outcome`.
    pub fn not_executed_here(&self) -> Option<&'static str> {
        self.actions
            .iter()
            .any(|action| matches!(action, ConfigAction::Policy { .. }))
            .then_some(
                "It creates, changes or removes a policy, which this app reads but does not execute yet.",
            )
    }
}

impl Squad {
    /// Read a config (v4) or settings (smart account) transaction account of
    /// this multisig. `None` when `data` is not one, belongs to another
    /// multisig, or holds an action this app cannot read, which leaves the
    /// whole transaction unread rather than smaller than it is.
    pub fn parse_change(&self, data: &[u8]) -> Option<ConfigTransaction> {
        let (account, owner) = match self.program {
            SquadsProgram::V4 => ("configTransaction", "multisig"),
            SquadsProgram::SmartAccount => ("settingsTransaction", "settings"),
        };
        let stored = self.program.account(account, data)?;
        if key(field(&stored, owner)?)? != self.address {
            return None;
        }
        let actions = list(field(&stored, "actions")?)?
            .iter()
            .map(|action| action_of(self.program, action))
            .collect::<Option<Vec<_>>>()?;
        Some(ConfigTransaction {
            program: self.program,
            creator: key(field(&stored, "creator")?)?,
            index: number(field(&stored, "index")?)?,
            actions,
        })
    }

    /// Store `actions` as change number `index` of this multisig. `creator`
    /// is a member that may propose and signs; `rent_payer` signs and pays
    /// for the account. `None` when an action cannot be stored by this app
    /// (a policy action) or does not belong to this program (a rent
    /// collector on the smart account, an archival authority on v4).
    ///
    /// Both programs refuse it for a multisig with a config or settings
    /// authority (`NotSupportedForControlled`), with no action, and with a
    /// time lock over three months.
    pub fn create_change(
        &self,
        index: u64,
        actions: &[ConfigAction],
        creator: &Pubkey,
        rent_payer: &Pubkey,
    ) -> Option<Instruction> {
        let actions = actions
            .iter()
            .map(|action| action_value(self.program, action))
            .collect::<Option<Vec<_>>>()?;
        let args = record([
            ("actions", Value::Array(actions)),
            ("memo", Value::Option(None)),
        ]);
        let mut accounts = vec![
            (self.change_owner(), self.address),
            ("transaction", self.transaction(index)),
            ("creator", *creator),
            ("rentPayer", *rent_payer),
            ("systemProgram", SYSTEM_PROGRAM_ID),
        ];
        let name = match self.program {
            SquadsProgram::V4 => "configTransactionCreate",
            SquadsProgram::SmartAccount => {
                accounts.push(("program", self.program.program_id()));
                "createSettingsTransaction"
            }
        };
        Some(self.program.instruction(name, &accounts, Some(args)))
    }

    /// Execute approved change `change`. `member` signs, and so does
    /// `rent_payer`, which pays when the multisig account has to grow (a
    /// member added) or for a spending limit's account, and gets the rent
    /// of a removed spending limit back. `None` when this app does not
    /// execute it (see [`ConfigTransaction::not_executed_here`]).
    ///
    /// The spending limits it creates or removes follow the named accounts,
    /// writable: both programs find each by address.
    pub fn execute_change(
        &self,
        change: &ConfigTransaction,
        member: &Pubkey,
        rent_payer: &Pubkey,
    ) -> Option<Instruction> {
        if change.not_executed_here().is_some() {
            return None;
        }
        let index = change.index;
        let mut accounts = vec![
            (self.change_owner(), self.address),
            ("proposal", self.proposal(index)),
            ("transaction", self.transaction(index)),
            ("rentPayer", *rent_payer),
            ("systemProgram", SYSTEM_PROGRAM_ID),
        ];
        let name = match self.program {
            SquadsProgram::V4 => {
                accounts.push(("member", *member));
                "configTransactionExecute"
            }
            SquadsProgram::SmartAccount => {
                accounts.push(("signer", *member));
                accounts.push(("program", self.program.program_id()));
                "executeSettingsTransaction"
            }
        };
        let mut instruction = self.program.instruction(name, &accounts, None);
        for action in &change.actions {
            let limit = match action {
                ConfigAction::AddSpendingLimit(limit) => self.spending_limit(&limit.seed),
                ConfigAction::RemoveSpendingLimit(limit) => *limit,
                _ => continue,
            };
            if !instruction.accounts.iter().any(|meta| meta.pubkey == limit) {
                instruction.accounts.push(AccountMeta::new(limit, false));
            }
        }
        Some(instruction)
    }

    /// The account of the spending limit derived from `seed`.
    pub fn spending_limit(&self, seed: &Pubkey) -> Pubkey {
        self.program.address(&[
            self.program.prefix(),
            self.address.as_ref(),
            b"spending_limit",
            seed.as_ref(),
        ])
    }

    /// What the IDL calls the multisig in the instructions about a change.
    fn change_owner(&self) -> &'static str {
        match self.program {
            SquadsProgram::V4 => "multisig",
            SquadsProgram::SmartAccount => "settings",
        }
    }
}

/// The actions an instruction that stores a change to a multisig carries
/// (`configTransactionCreate` / `createSettingsTransaction`), in order, read
/// from its own bytes with the IDL the app ships, so Review and Sign can say
/// them in words. `None` for any other instruction, and for one holding an
/// action this app cannot read.
pub fn stored_change_actions(program_id: &Pubkey, data: &[u8]) -> Option<Vec<ConfigAction>> {
    let program = SquadsProgram::from_program_id(program_id)?;
    let parsed = program.client().parse_instruction_data(data).ok()??;
    let name = match parsed.path.last()? {
        codama_nodes::Node::Instruction(node) => node.name.to_string(),
        _ => return None,
    };
    let stores = match program {
        SquadsProgram::V4 => "configTransactionCreate",
        SquadsProgram::SmartAccount => "createSettingsTransaction",
    };
    if name != stores {
        return None;
    }
    list(field(field(&parsed.data, "args")?, "actions")?)?
        .iter()
        .map(|action| action_of(program, action))
        .collect()
}

fn keys(value: &Value) -> Option<Vec<Pubkey>> {
    list(value)?.iter().map(key).collect()
}

fn maybe_key(value: &Value) -> Option<Option<Pubkey>> {
    match value {
        Value::Option(None) => Some(None),
        Value::Option(Some(inner)) => Some(Some(key(inner)?)),
        _ => None,
    }
}

fn member_of(value: &Value) -> Option<SquadsMember> {
    Some(SquadsMember {
        key: key(field(value, "key")?)?,
        permissions: number(field(field(value, "permissions")?, "mask")?)?,
    })
}

fn action_of(program: SquadsProgram, action: &Value) -> Option<ConfigAction> {
    let Value::Enum { variant, fields } = action else {
        return None;
    };
    let f = |name: &str| field(fields, name);
    Some(match (program, variant.as_str()) {
        (SquadsProgram::V4, "AddMember") => ConfigAction::AddMember(member_of(f("newMember")?)?),
        (SquadsProgram::SmartAccount, "AddSigner") => {
            ConfigAction::AddMember(member_of(f("newSigner")?)?)
        }
        (SquadsProgram::V4, "RemoveMember") => ConfigAction::RemoveMember(key(f("oldMember")?)?),
        (SquadsProgram::SmartAccount, "RemoveSigner") => {
            ConfigAction::RemoveMember(key(f("oldSigner")?)?)
        }
        (_, "ChangeThreshold") => ConfigAction::ChangeThreshold(number(f("newThreshold")?)?),
        (_, "SetTimeLock") => ConfigAction::SetTimeLock(number(f("newTimeLock")?)?),
        (_, "AddSpendingLimit") => {
            let (seed, index, members, expiration) = match program {
                SquadsProgram::V4 => ("createKey", "vaultIndex", "members", None),
                SquadsProgram::SmartAccount => (
                    "seed",
                    "accountIndex",
                    "signers",
                    Some(number(f("expiration")?)?),
                ),
            };
            ConfigAction::AddSpendingLimit(SpendingLimit {
                seed: key(f(seed)?)?,
                vault_index: number(f(index)?)?,
                mint: key(f("mint")?)?,
                amount: number(f("amount")?)?,
                period: Period::of(f("period")?)?,
                members: keys(f(members)?)?,
                destinations: keys(f("destinations")?)?,
                expiration,
            })
        }
        (_, "RemoveSpendingLimit") => ConfigAction::RemoveSpendingLimit(key(f("spendingLimit")?)?),
        (SquadsProgram::V4, "SetRentCollector") => {
            ConfigAction::SetRentCollector(maybe_key(f("newRentCollector")?)?)
        }
        (SquadsProgram::SmartAccount, "SetArchivalAuthority") => {
            ConfigAction::SetArchivalAuthority(maybe_key(f("newArchivalAuthority")?)?)
        }
        (SquadsProgram::SmartAccount, "PolicyCreate" | "PolicyUpdate" | "PolicyRemove") => {
            let verb = match variant.as_str() {
                "PolicyCreate" => PolicyVerb::Create,
                "PolicyUpdate" => PolicyVerb::Update,
                _ => PolicyVerb::Remove,
            };
            let Value::Struct(all) = fields.as_ref() else {
                return None;
            };
            let policy = match verb {
                PolicyVerb::Create => None,
                _ => Some(key(f("policy")?)?),
            };
            ConfigAction::Policy {
                verb,
                policy,
                fields: all
                    .iter()
                    .filter(|(name, _)| name.as_str() != "policy")
                    .map(|(name, value)| (name.clone(), value_text(value)))
                    .collect(),
            }
        }
        _ => return None,
    })
}

fn action_value(program: SquadsProgram, action: &ConfigAction) -> Option<Value> {
    let pubkey = |key: &Pubkey| Value::PublicKey(key.to_bytes());
    let maybe = |key: &Option<Pubkey>| Value::Option(key.map(|key| Box::new(pubkey(&key))));
    let pubkeys = |keys: &[Pubkey]| Value::Array(keys.iter().map(pubkey).collect());
    let variant = |name: &str, fields: Value| Value::Enum {
        variant: name.into(),
        fields: Box::new(fields),
    };
    let v4 = program == SquadsProgram::V4;
    Some(match action {
        ConfigAction::AddMember(member) => {
            let value = record([
                ("key", pubkey(&member.key)),
                ("permissions", record([("mask", member.permissions.into())])),
            ]);
            if v4 {
                variant("AddMember", record([("newMember", value)]))
            } else {
                variant("AddSigner", record([("newSigner", value)]))
            }
        }
        ConfigAction::RemoveMember(old) => {
            if v4 {
                variant("RemoveMember", record([("oldMember", pubkey(old))]))
            } else {
                variant("RemoveSigner", record([("oldSigner", pubkey(old))]))
            }
        }
        ConfigAction::ChangeThreshold(threshold) => variant(
            "ChangeThreshold",
            record([("newThreshold", (*threshold).into())]),
        ),
        ConfigAction::SetTimeLock(seconds) => {
            variant("SetTimeLock", record([("newTimeLock", (*seconds).into())]))
        }
        ConfigAction::AddSpendingLimit(limit) => {
            let common = |seed: &str, index: &str, members: &str| {
                vec![
                    (seed.to_string(), pubkey(&limit.seed)),
                    (index.to_string(), limit.vault_index.into()),
                    ("mint".to_string(), pubkey(&limit.mint)),
                    ("amount".to_string(), limit.amount.into()),
                    ("period".to_string(), limit.period.value()),
                    (members.to_string(), pubkeys(&limit.members)),
                    ("destinations".to_string(), pubkeys(&limit.destinations)),
                ]
            };
            let fields = if v4 {
                common("createKey", "vaultIndex", "members")
            } else {
                let mut fields = common("seed", "accountIndex", "signers");
                fields.push((
                    "expiration".to_string(),
                    limit.expiration.unwrap_or(i64::MAX).into(),
                ));
                fields
            };
            variant("AddSpendingLimit", Value::struct_from(fields))
        }
        ConfigAction::RemoveSpendingLimit(limit) => variant(
            "RemoveSpendingLimit",
            record([("spendingLimit", pubkey(limit))]),
        ),
        ConfigAction::SetRentCollector(collector) if v4 => variant(
            "SetRentCollector",
            record([("newRentCollector", maybe(collector))]),
        ),
        ConfigAction::SetArchivalAuthority(authority) if !v4 => variant(
            "SetArchivalAuthority",
            record([("newArchivalAuthority", maybe(authority))]),
        ),
        ConfigAction::SetRentCollector(_)
        | ConfigAction::SetArchivalAuthority(_)
        | ConfigAction::Policy { .. } => return None,
    })
}

/// A decoded value as short text, for a field this app does not word:
/// numbers as they are, addresses in base58, bytes in hex, a struct as
/// `name: value` pairs, a variant by its name.
pub(super) fn value_text(value: &Value) -> String {
    let join = |items: &[Value]| items.iter().map(value_text).collect::<Vec<_>>().join(", ");
    match value {
        Value::Number(number) => match number {
            codama_dynamic_client::NumberValue::Unsigned(n) => n.to_string(),
            codama_dynamic_client::NumberValue::Signed(n) => n.to_string(),
            codama_dynamic_client::NumberValue::Float(n) => n.to_string(),
        },
        Value::Bool(b) => b.to_string(),
        Value::String(s) => format!("\"{s}\""),
        Value::Bytes(bytes) => format!("0x{}", hex::encode(bytes)),
        Value::PublicKey(bytes) => Pubkey::new_from_array(*bytes).to_string(),
        Value::Array(items) | Value::Set(items) => format!("[{}]", join(items)),
        Value::Tuple(items) => format!("({})", join(items)),
        Value::Map(pairs) => format!(
            "{{{}}}",
            pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", value_text(k), value_text(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Struct(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(name, value)| format!("{name}: {}", value_text(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Enum { variant, fields } => match fields.as_ref() {
            Value::Struct(inner) if inner.is_empty() => variant.clone(),
            Value::Struct(inner) if inner.len() == 1 && inner.contains_key("fields") => {
                format!("{variant}{}", value_text(&inner["fields"]))
            }
            other => format!("{variant} {}", value_text(other)),
        },
        Value::Option(None) => "none".into(),
        Value::Option(Some(inner)) => value_text(inner),
    }
}
