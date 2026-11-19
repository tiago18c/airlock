//! What a multisig, a proposal and a stored transaction hold on chain.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SquadsMember {
    pub key: Pubkey,
    /// A sum of the `PERMISSION_*` values.
    pub permissions: u8,
}

/// What a new multisig starts with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SquadsSetup {
    pub members: Vec<SquadsMember>,
    pub threshold: u16,
    /// Seconds between approval and execution.
    pub time_lock: u32,
    /// A key that changes the members, threshold and time lock by itself,
    /// without a vote. `None` for a multisig whose members decide.
    pub config_authority: Option<Pubkey>,
    /// v4 only: where the rent of closed transaction accounts goes. `None`
    /// leaves them open for good.
    pub rent_collector: Option<Pubkey>,
}

/// What a multisig account says about itself on chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SquadState {
    pub threshold: u16,
    /// Seconds an approved proposal waits before it can be executed.
    pub time_lock: u32,
    /// Number of the newest transaction. The next one is one higher.
    pub transaction_index: u64,
    /// Proposals up to this number were made before the members, threshold
    /// or time lock last changed, and can no longer be voted on.
    pub stale_transaction_index: u64,
    pub members: Vec<SquadsMember>,
    /// The config authority (v4) or settings authority (smart account): a
    /// key that changes the members, threshold and time lock by itself. Such
    /// a multisig is "controlled", and its members cannot change it by
    /// proposal. `None` when it is the members who decide (the program
    /// keeps the default address then).
    pub config_authority: Option<Pubkey>,
    /// v4: the address the rent of an executed, rejected or cancelled
    /// transaction's accounts can be closed to. `None` turns closing off.
    /// The smart account program keeps no such setting: it returns the rent
    /// to whoever paid it, as each account records.
    pub rent_collector: Option<Pubkey>,
}

/// Where a proposal stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalStatus {
    Draft,
    /// Open for votes.
    Active,
    Rejected,
    /// The threshold is met. It can be executed once the time lock allows.
    Approved,
    Executing,
    Executed,
    Cancelled,
}

/// A proposal account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalState {
    /// The number of the transaction it is about.
    pub index: u64,
    pub status: ProposalStatus,
    /// When it got that status, in Unix seconds. Zero for `Executing`.
    pub since: i64,
    pub approved: Vec<Pubkey>,
    pub rejected: Vec<Pubkey>,
    pub cancelled: Vec<Pubkey>,
}

/// A transaction stored for a vault, read from its account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultTransaction {
    pub creator: Pubkey,
    pub index: u64,
    pub vault_index: u8,
    /// The vault that runs the instructions and signs for them.
    pub vault: Pubkey,
    /// How many ephemeral signers the transaction declares: addresses the
    /// program derives for it and signs as when it runs.
    pub ephemeral_signers: u8,
    pub instructions: Vec<Instruction>,
    /// The address lookup tables its message reads accounts from, in its
    /// order. Empty for most.
    pub lookup_tables: Vec<Pubkey>,
    /// Those of [`Self::lookup_tables`] their authority has deactivated. The
    /// programs still read such a table, but once it is closed this
    /// transaction can never be executed.
    pub deactivated_tables: Vec<Pubkey>,
    /// Keys other than the vault and the ephemeral signers that the stored
    /// message has sign: each has to sign the transaction that executes it.
    pub other_signers: Vec<Pubkey>,
    /// Every account the execution has to list after its own, in the order
    /// the program reads them.
    pub(super) execute_accounts: Vec<AccountMeta>,
    /// What each of [`Self::lookup_tables`] held when it was read, so it can
    /// be simulated reading the same tables (`simulate.rs`).
    pub(super) tables: Vec<(Pubkey, Vec<Pubkey>)>,
}

impl VaultTransaction {
    #[cfg(test)]
    pub(crate) fn for_test(
        squad: &Squad,
        index: u64,
        vault_index: u8,
        instructions: &[Instruction],
    ) -> Self {
        let vault = squad.vault(vault_index);
        let message = vault_message(&vault, instructions);
        Self {
            creator: Pubkey::default(),
            index,
            vault_index,
            vault,
            ephemeral_signers: 0,
            instructions: message.instructions().expect("a well-formed message"),
            lookup_tables: Vec::new(),
            deactivated_tables: Vec::new(),
            other_signers: Vec::new(),
            execute_accounts: message.execute_accounts(),
            tables: Vec::new(),
        }
    }
}

/// What a transaction account of a multisig holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredAccount {
    Vault(VaultTransaction),
    /// A vault transaction whose accounts come partly from address lookup
    /// tables. It is read once those tables are: see [`TableBound::resolve`].
    LookupTables(TableBound),
    /// A change to the multisig itself: its members, threshold, time lock,
    /// spending limits and the like.
    Config(ConfigTransaction),
    /// A batch, a policy payload, or a change to the multisig holding an
    /// action this app cannot read.
    Other,
}

/// A stored vault transaction that reads accounts from address lookup
/// tables, before those are read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableBound {
    pub index: u64,
    creator: Pubkey,
    vault_index: u8,
    vault: Pubkey,
    /// The ephemeral signers' addresses, which the program signs for.
    ephemeral: Vec<Pubkey>,
    message: VaultMessage,
}

/// Why a stored transaction that reads accounts from lookup tables cannot
/// be read.
///
/// A table's entries never change once written, and its address cannot be
/// used again once it is closed. So a transaction whose tables all exist and
/// hold every entry it names reads now as it will run. One whose table is
/// gone can never run. One that names an entry its table does not hold yet
/// cannot be known: the table's authority may still add any address there.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum TableProblem {
    #[error("It reads accounts from lookup table {table}, which does not exist on this cluster: it was closed, or never made. It cannot be read or executed.")]
    Missing { table: Pubkey },
    #[error("It reads entry {index} of lookup table {table}, which holds {holds} addresses. What that entry will hold is not known yet, so it cannot be read, approved or executed.")]
    Short {
        table: Pubkey,
        index: u8,
        holds: usize,
    },
}

/// What a lookup table account holds, from its data as the lookup table
/// program writes it: its addresses, and whether its authority deactivated
/// it. `None` when it is not a table.
pub fn lookup_table_of(data: &[u8]) -> Option<(Vec<Pubkey>, bool)> {
    // `ProgramState::LookupTable` is 1, then the deactivation slot, which is
    // the largest number while the table is active.
    if data.get(..4)? != 1u32.to_le_bytes() {
        return None;
    }
    let deactivation = u64::from_le_bytes(data.get(4..12)?.try_into().ok()?);
    let addresses = crate::tx::parse_lookup_table_addresses(data).ok()?;
    Some((addresses, deactivation != u64::MAX))
}

impl TableBound {
    /// The tables it reads from, in its order.
    pub fn tables(&self) -> Vec<Pubkey> {
        self.message.tables()
    }

    /// Read it with the lookup tables in `accounts`: the data of each
    /// table's account, by address, from accounts the lookup table program
    /// owns. Whether they come from a node or another device decides what
    /// the result is worth.
    pub fn resolve(
        &self,
        accounts: &std::collections::HashMap<Pubkey, Vec<u8>>,
    ) -> Result<VaultTransaction, TableProblem> {
        let mut tables = std::collections::HashMap::new();
        let mut deactivated = Vec::new();
        for table in self.tables() {
            if let Some((addresses, off)) =
                accounts.get(&table).and_then(|data| lookup_table_of(data))
            {
                if off && !deactivated.contains(&table) {
                    deactivated.push(table);
                }
                tables.insert(table, addresses);
            }
        }
        let loaded = self.message.load(&tables)?;
        let instructions =
            self.message
                .instructions_with(&loaded)
                .ok_or(TableProblem::Missing {
                    table: self.tables().first().copied().unwrap_or_default(),
                })?;
        let held = self
            .tables()
            .into_iter()
            .filter_map(|table| Some((table, tables.get(&table)?.clone())))
            .collect();
        Ok(self.finish(instructions, &loaded, deactivated, held))
    }

    fn finish(
        &self,
        instructions: Vec<Instruction>,
        loaded: &super::message::Loaded,
        deactivated_tables: Vec<Pubkey>,
        tables: Vec<(Pubkey, Vec<Pubkey>)>,
    ) -> VaultTransaction {
        let program_signs: Vec<Pubkey> = std::iter::once(self.vault)
            .chain(self.ephemeral.iter().copied())
            .collect();
        let signers = usize::from(self.message.num_signers);
        let other_signers = self.message.account_keys
            [..signers.min(self.message.account_keys.len())]
            .iter()
            .copied()
            .filter(|key| !program_signs.contains(key))
            .collect();
        VaultTransaction {
            creator: self.creator,
            index: self.index,
            vault_index: self.vault_index,
            vault: self.vault,
            ephemeral_signers: u8::try_from(self.ephemeral.len()).unwrap_or(u8::MAX),
            instructions,
            lookup_tables: self.tables(),
            deactivated_tables,
            other_signers,
            execute_accounts: self.message.execute_listing(loaded, &program_signs),
            tables,
        }
    }
}

impl Squad {
    /// Read the multisig (v4) or settings (smart account) account's data.
    pub fn parse_state(&self, data: &[u8]) -> Option<SquadState> {
        let (account, members, authority) = match self.program {
            SquadsProgram::V4 => ("multisig", "members", "configAuthority"),
            SquadsProgram::SmartAccount => ("settings", "signers", "settingsAuthority"),
        };
        let state = self.program.account(account, data)?;
        let members = list(field(&state, members)?)?
            .iter()
            .map(|member| {
                Some(SquadsMember {
                    key: key(field(member, "key")?)?,
                    permissions: number(field(field(member, "permissions")?, "mask")?)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let config_authority =
            Some(key(field(&state, authority)?)?).filter(|key| *key != Pubkey::default());
        let rent_collector = match self.program {
            SquadsProgram::V4 => match field(&state, "rentCollector")? {
                Value::Option(None) => None,
                Value::Option(Some(collector)) => Some(key(collector)?),
                _ => return None,
            },
            SquadsProgram::SmartAccount => None,
        };
        Some(SquadState {
            threshold: number(field(&state, "threshold")?)?,
            time_lock: number(field(&state, "timeLock")?)?,
            transaction_index: number(field(&state, "transactionIndex")?)?,
            stale_transaction_index: number(field(&state, "staleTransactionIndex")?)?,
            members,
            config_authority,
            rent_collector,
        })
    }

    /// Read a proposal account of this multisig. `None` when `data` is not
    /// one.
    pub fn parse_proposal(&self, data: &[u8]) -> Option<ProposalState> {
        let owner = match self.program {
            SquadsProgram::V4 => "multisig",
            SquadsProgram::SmartAccount => "settings",
        };
        let proposal = self.program.account("proposal", data)?;
        if key(field(&proposal, owner)?)? != self.address {
            return None;
        }
        let Value::Enum { variant, fields } = field(&proposal, "status")? else {
            return None;
        };
        let status = match variant.as_str() {
            "Draft" => ProposalStatus::Draft,
            "Active" => ProposalStatus::Active,
            "Rejected" => ProposalStatus::Rejected,
            "Approved" => ProposalStatus::Approved,
            "Executing" => ProposalStatus::Executing,
            "Executed" => ProposalStatus::Executed,
            "Cancelled" => ProposalStatus::Cancelled,
            _ => return None,
        };
        // Every status but `Executing` carries the time it was reached.
        let since = match field(fields, "timestamp") {
            Some(timestamp) => number(timestamp)?,
            None => 0,
        };
        let keys = |name: &str| {
            list(field(&proposal, name)?)?
                .iter()
                .map(key)
                .collect::<Option<Vec<Pubkey>>>()
        };
        Some(ProposalState {
            index: number(field(&proposal, "transactionIndex")?)?,
            status,
            since,
            approved: keys("approved")?,
            rejected: keys("rejected")?,
            cancelled: keys("cancelled")?,
        })
    }

    /// Read a transaction account of this multisig. `None` when `data` is
    /// not an account of the program, or belongs to another multisig.
    pub fn parse_transaction(&self, data: &[u8]) -> Option<StoredAccount> {
        let (account, owner) = match self.program {
            SquadsProgram::V4 => ("vaultTransaction", "multisig"),
            SquadsProgram::SmartAccount => ("transaction", "consensusAccount"),
        };
        let Some(stored) = self.program.account(account, data) else {
            let (change, change_owner) = match self.program {
                SquadsProgram::V4 => ("configTransaction", "multisig"),
                SquadsProgram::SmartAccount => ("settingsTransaction", "settings"),
            };
            if let Some(change) = self.program.account(change, data) {
                if key(field(&change, change_owner)?)? != self.address {
                    return None;
                }
                // One action this app cannot read leaves it all unread.
                return Some(
                    self.parse_change(data)
                        .map_or(StoredAccount::Other, StoredAccount::Config),
                );
            }
            // Some other account of the program, or nothing it knows.
            let known = self.program.client().parse_account_data(data).ok()?;
            return known.map(|_| StoredAccount::Other);
        };
        if key(field(&stored, owner)?)? != self.address {
            return None;
        }
        // v4 keeps the vault index and the message beside each other. The
        // smart account program keeps them inside the payload it stores.
        let (details, index_name) = match self.program {
            SquadsProgram::V4 => (&stored, "vaultIndex"),
            SquadsProgram::SmartAccount => {
                let Value::Enum { variant, fields } = field(&stored, "payload")? else {
                    return None;
                };
                if variant != "TransactionPayload" {
                    return Some(StoredAccount::Other);
                }
                let Value::Tuple(inner) = field(fields, "fields")? else {
                    return None;
                };
                (inner.first()?, "accountIndex")
            }
        };
        let vault_index: u8 = number(field(details, index_name)?)?;
        let ephemeral_signers =
            u8::try_from(bytes(field(details, "ephemeralSignerBumps")?)?.len()).ok()?;
        let message = VaultMessage::from_account(field(details, "message")?)?;
        let index: u64 = number(field(&stored, "index")?)?;
        let bound = TableBound {
            index,
            creator: key(field(&stored, "creator")?)?,
            vault_index,
            vault: self.vault(vault_index),
            ephemeral: (0..ephemeral_signers)
                .map(|at| self.ephemeral_signer(index, at))
                .collect(),
            message,
        };
        if !bound.message.lookups.is_empty() {
            return Some(StoredAccount::LookupTables(bound));
        }
        let instructions = bound.message.instructions()?;
        Some(StoredAccount::Vault(bound.finish(
            instructions,
            &Loaded::default(),
            Vec::new(),
            Vec::new(),
        )))
    }

    /// Ephemeral signer `at` of transaction `index`: an address the program
    /// derives for that transaction and signs as when it runs it.
    pub fn ephemeral_signer(&self, index: u64, at: u8) -> Pubkey {
        self.program.address(&[
            self.program.prefix(),
            self.transaction(index).as_ref(),
            b"ephemeral_signer",
            &[at],
        ])
    }
}
