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
    pub instructions: Vec<Instruction>,
    /// Every account of the stored message in its order, as the execution
    /// has to list them.
    pub(super) execute_accounts: Vec<AccountMeta>,
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
            instructions: message.instructions().expect("a well-formed message"),
            execute_accounts: message.execute_accounts(),
        }
    }
}

/// What a transaction account of a multisig holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredAccount {
    Vault(VaultTransaction),
    /// A vault transaction whose accounts come partly from address lookup
    /// tables. Its instructions cannot be resolved from the account alone.
    LookupTables,
    /// A change to the multisig itself, a batch, or a policy payload.
    Other,
}

impl Squad {
    /// Read the multisig (v4) or settings (smart account) account's data.
    pub fn parse_state(&self, data: &[u8]) -> Option<SquadState> {
        let (account, members) = match self.program {
            SquadsProgram::V4 => ("multisig", "members"),
            SquadsProgram::SmartAccount => ("settings", "signers"),
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
        Some(SquadState {
            threshold: number(field(&state, "threshold")?)?,
            time_lock: number(field(&state, "timeLock")?)?,
            transaction_index: number(field(&state, "transactionIndex")?)?,
            stale_transaction_index: number(field(&state, "staleTransactionIndex")?)?,
            members,
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
        let message = field(details, "message")?;
        if !list(field(message, "addressTableLookups")?)?.is_empty() {
            return Some(StoredAccount::LookupTables);
        }
        let message = VaultMessage::from_account(message)?;
        Some(StoredAccount::Vault(VaultTransaction {
            creator: key(field(&stored, "creator")?)?,
            index: number(field(&stored, "index")?)?,
            vault_index,
            vault: self.vault(vault_index),
            instructions: message.instructions()?,
            execute_accounts: message.execute_accounts(),
        }))
    }
}
