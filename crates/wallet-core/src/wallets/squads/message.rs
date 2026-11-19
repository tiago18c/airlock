//! The transaction a vault stores: its accounts, its instructions and its bytes.

use super::*;
use solana_message::compiled_instruction::CompiledInstruction;
use wincode::containers;
use wincode::len::UseIntLen;
use wincode::{SchemaRead, SchemaWrite};

/// A transaction stored for a vault, read back from the instruction that
/// stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTransaction {
    pub squad: Squad,
    pub vault_index: u8,
    /// The vault that will run the instructions and sign for them.
    pub vault: Pubkey,
    pub instructions: Vec<Instruction>,
}

impl StoredTransaction {
    /// `None` when `instruction` is not a well-formed v4
    /// `vaultTransactionCreate` or smart account `createTransaction`.
    pub fn parse(instruction: &Instruction) -> Option<Self> {
        let program = SquadsProgram::from_program_id(&instruction.program_id)?;
        let parsed = program.client().parse_instruction(instruction).ok()??;
        let name = match parsed.path.last()? {
            codama_nodes::Node::Instruction(node) => node.name.to_string(),
            _ => return None,
        };
        let args = field(&parsed.data, "args")?;
        let (stored, index_name) = match (program, name.as_str()) {
            (SquadsProgram::V4, "vaultTransactionCreate") => (args, "vaultIndex"),
            (SquadsProgram::SmartAccount, "createTransaction") => {
                let Value::Enum { variant, fields } = args else {
                    return None;
                };
                if variant != "TransactionPayload" {
                    return None;
                }
                let Value::Tuple(inner) = field(fields, "fields")? else {
                    return None;
                };
                (inner.first()?, "accountIndex")
            }
            _ => return None,
        };
        let vault_index: u8 = number(field(stored, index_name)?)?;
        let message = VaultMessage::from_bytes(bytes(field(stored, "transactionMessage")?)?)?;
        let squad = Squad {
            program,
            address: instruction.accounts.first()?.pubkey,
        };
        Some(Self {
            vault: squad.vault(vault_index),
            squad,
            vault_index,
            instructions: message.instructions()?,
        })
    }
}

impl StoredTransaction {
    /// `instruction`, storing `instructions` in place of what it stores:
    /// the same multisig, vault, transaction account, creator and rent
    /// payer. `None` when it is not one [`Self::parse`] reads, or when the
    /// instructions do not fit a stored message.
    pub fn rewrite(instruction: &Instruction, instructions: &[Instruction]) -> Option<Instruction> {
        let stored = Self::parse(instruction)?;
        let program = stored.squad.program;
        let client = program.client();
        let parsed = client.parse_instruction(instruction).ok()??;
        let name = match parsed.path.last()? {
            codama_nodes::Node::Instruction(node) => node.name.to_string(),
            _ => return None,
        };
        let message = vault_message(&stored.vault, instructions).to_bytes()?;
        let mut args = field(&parsed.data, "args")?.clone();
        let carried = match program {
            SquadsProgram::V4 => &mut args,
            SquadsProgram::SmartAccount => {
                let Value::Enum { fields, .. } = &mut args else {
                    return None;
                };
                let Value::Tuple(inner) = field_mut(fields, "fields")? else {
                    return None;
                };
                inner.first_mut()?
            }
        };
        *field_mut(carried, "transactionMessage")? = Value::Bytes(message);
        let mut method = client.method(&name).ok()?;
        for account in &parsed.accounts {
            method = method.account(&account.name, account.meta.pubkey);
        }
        let mut built = method.argument("args", args).build().ok()?;
        built
            .accounts
            .extend(parsed.remaining_accounts.iter().cloned());
        Some(built)
    }
}

fn field_mut<'a>(value: &'a mut Value, name: &str) -> Option<&'a mut Value> {
    match value {
        Value::Struct(fields) => fields.get_mut(name),
        _ => None,
    }
}

/// Instructions compiled the way Squads stores them: accounts ordered writable
/// signers, read-only signers, writable, read-only, with the vault first.
/// Then, like a v0 message, addresses read from lookup tables, which an
/// instruction numbers after the accounts the message lists itself: every
/// table's writable ones, then every table's read-only ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VaultMessage {
    pub(super) num_signers: u8,
    pub(super) num_writable_signers: u8,
    pub(super) num_writable_non_signers: u8,
    pub(super) account_keys: Vec<Pubkey>,
    pub(super) instructions: Vec<solana_message::compiled_instruction::CompiledInstruction>,
    pub(super) lookups: Vec<MessageLookup>,
}

/// One lookup table a stored message reads addresses from, and which of its
/// entries, by position in the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MessageLookup {
    pub(super) table: Pubkey,
    pub(super) writable: Vec<u8>,
    pub(super) readonly: Vec<u8>,
}

/// The addresses a message's lookups name, read from the tables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Loaded {
    /// For each lookup, in the message's order: its writable addresses, then
    /// its read-only ones.
    per_table: Vec<(Vec<Pubkey>, Vec<Pubkey>)>,
}

impl Loaded {
    fn writable(&self) -> impl Iterator<Item = &Pubkey> {
        self.per_table.iter().flat_map(|(writable, _)| writable)
    }

    fn readonly(&self) -> impl Iterator<Item = &Pubkey> {
        self.per_table.iter().flat_map(|(_, readonly)| readonly)
    }
}

pub(super) fn vault_message(vault: &Pubkey, instructions: &[Instruction]) -> VaultMessage {
    let message = LegacyMessage::new(instructions, Some(vault));
    let header = message.header;
    let num_signers = header.num_required_signatures;
    VaultMessage {
        num_signers,
        num_writable_signers: num_signers - header.num_readonly_signed_accounts,
        num_writable_non_signers: (message.account_keys.len() as u8)
            - num_signers
            - header.num_readonly_unsigned_accounts,
        account_keys: message.account_keys,
        instructions: message.instructions,
        lookups: Vec::new(),
    }
}

/// `instructions` compiled the way Squads stores them, with every address
/// `tables` hold read from them where a v0 message would: anything that
/// signs or is a program an instruction calls stays in the message itself.
/// `None` when they do not compile.
pub(super) fn vault_message_with_tables(
    vault: &Pubkey,
    instructions: &[Instruction],
    tables: &[(Pubkey, Vec<Pubkey>)],
) -> Option<VaultMessage> {
    let tables: Vec<solana_message::AddressLookupTableAccount> = tables
        .iter()
        .map(
            |(key, addresses)| solana_message::AddressLookupTableAccount {
                key: *key,
                addresses: addresses.clone(),
            },
        )
        .collect();
    let message = solana_message::v0::Message::try_compile(
        vault,
        instructions,
        &tables,
        solana_hash::Hash::default(),
    )
    .ok()?;
    let header = message.header;
    let num_signers = header.num_required_signatures;
    VaultMessage {
        num_signers,
        num_writable_signers: num_signers.checked_sub(header.num_readonly_signed_accounts)?,
        num_writable_non_signers: u8::try_from(message.account_keys.len())
            .ok()?
            .checked_sub(num_signers)?
            .checked_sub(header.num_readonly_unsigned_accounts)?,
        account_keys: message.account_keys,
        instructions: message.instructions,
        lookups: message
            .address_table_lookups
            .into_iter()
            .map(|lookup| MessageLookup {
                table: lookup.account_key,
                writable: lookup.writable_indexes,
                readonly: lookup.readonly_indexes,
            })
            .collect(),
    }
    .checked()
}

impl VaultMessage {
    /// The lookup tables it reads from, in its order.
    pub(super) fn tables(&self) -> Vec<Pubkey> {
        self.lookups.iter().map(|lookup| lookup.table).collect()
    }

    /// Read every address the lookups name from `tables`, the addresses each
    /// table holds by its address. `Err` names the first lookup that cannot
    /// be read: its table is not there, or holds fewer entries than it asks
    /// for.
    pub(super) fn load(
        &self,
        tables: &std::collections::HashMap<Pubkey, Vec<Pubkey>>,
    ) -> Result<Loaded, TableProblem> {
        let mut per_table = Vec::new();
        for lookup in &self.lookups {
            let Some(addresses) = tables.get(&lookup.table) else {
                return Err(TableProblem::Missing {
                    table: lookup.table,
                });
            };
            let pick = |indexes: &[u8]| {
                indexes
                    .iter()
                    .map(|index| {
                        addresses
                            .get(usize::from(*index))
                            .copied()
                            .ok_or(TableProblem::Short {
                                table: lookup.table,
                                index: *index,
                                holds: addresses.len(),
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()
            };
            per_table.push((pick(&lookup.writable)?, pick(&lookup.readonly)?));
        }
        Ok(Loaded { per_table })
    }

    /// The address at `position` as an instruction numbers it, and whether
    /// the message asks for it writable.
    fn resolve(&self, position: usize, loaded: &Loaded) -> Option<(Pubkey, bool)> {
        if let Some(key) = self.account_keys.get(position) {
            return Some((*key, self.is_writable(position)));
        }
        let position = position - self.account_keys.len();
        let writable: Vec<&Pubkey> = loaded.writable().collect();
        if let Some(key) = writable.get(position) {
            return Some((**key, true));
        }
        loaded
            .readonly()
            .nth(position - writable.len())
            .map(|key| (*key, false))
    }
    pub(super) fn is_writable(&self, position: usize) -> bool {
        let signers = usize::from(self.num_signers);
        if position < signers {
            position < usize::from(self.num_writable_signers)
        } else {
            position - signers < usize::from(self.num_writable_non_signers)
        }
    }

    /// Every account in its order, as an execution lists them. The vault,
    /// first, signs inside the program. Any other signer of the stored
    /// message has to sign the transaction that executes it.
    pub(super) fn execute_accounts(&self) -> Vec<AccountMeta> {
        let vault = self.account_keys.first().copied().unwrap_or_default();
        self.execute_listing(&Loaded::default(), &[vault])
    }

    /// What an execution lists after its own accounts, in the order both
    /// programs read them: the lookup tables, then the message's own
    /// accounts, then for each table the addresses read from it, writable
    /// ones first. Of the message's signers, those in `program_signs` (the
    /// vault and the ephemeral signers, which the program signs for) are
    /// listed as not signing; any other has to sign the transaction.
    pub(super) fn execute_listing(
        &self,
        loaded: &Loaded,
        program_signs: &[Pubkey],
    ) -> Vec<AccountMeta> {
        let tables = self
            .lookups
            .iter()
            .map(|lookup| AccountMeta::new_readonly(lookup.table, false));
        let own = self.account_keys.iter().enumerate().map(|(position, key)| {
            let signs = position < usize::from(self.num_signers);
            AccountMeta {
                pubkey: *key,
                is_signer: signs && !program_signs.contains(key),
                is_writable: self.is_writable(position),
            }
        });
        let read = loaded.per_table.iter().flat_map(|(writable, readonly)| {
            writable
                .iter()
                .map(|key| AccountMeta::new(*key, false))
                .chain(
                    readonly
                        .iter()
                        .map(|key| AccountMeta::new_readonly(*key, false)),
                )
        });
        tables.chain(own).chain(read).collect()
    }

    pub(super) fn checked(self) -> Option<Self> {
        let fits = self.num_writable_signers <= self.num_signers
            && usize::from(self.num_signers) + usize::from(self.num_writable_non_signers)
                <= self.account_keys.len();
        fits.then_some(self)
    }

    /// The message of a transaction account, as the IDL decodes it.
    pub(super) fn from_account(message: &Value) -> Option<Self> {
        let instructions = list(field(message, "instructions")?)?
            .iter()
            .map(|instruction| {
                Some(solana_message::compiled_instruction::CompiledInstruction {
                    program_id_index: number(field(instruction, "programIdIndex")?)?,
                    accounts: bytes(field(instruction, "accountIndexes")?)?.to_vec(),
                    data: bytes(field(instruction, "data")?)?.to_vec(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let lookups = list(field(message, "addressTableLookups")?)?
            .iter()
            .map(|lookup| {
                Some(MessageLookup {
                    table: key(field(lookup, "accountKey")?)?,
                    writable: bytes(field(lookup, "writableIndexes")?)?.to_vec(),
                    readonly: bytes(field(lookup, "readonlyIndexes")?)?.to_vec(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Self {
            num_signers: number(field(message, "numSigners")?)?,
            num_writable_signers: number(field(message, "numWritableSigners")?)?,
            num_writable_non_signers: number(field(message, "numWritableNonSigners")?)?,
            account_keys: list(field(message, "accountKeys")?)?
                .iter()
                .map(key)
                .collect::<Option<Vec<_>>>()?,
            instructions,
            lookups,
        }
        .checked()
    }

    /// The message as an instruction carries it ([`WireMessage`]). A message
    /// that reads accounts from lookup tables is not one this app stores, and
    /// is not read.
    pub(super) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let wire: WireMessage = wincode::deserialize_exact(bytes).ok()?;
        if !wire.address_table_lookups.is_empty() {
            return None;
        }
        Self {
            num_signers: wire.num_signers,
            num_writable_signers: wire.num_writable_signers,
            num_writable_non_signers: wire.num_writable_non_signers,
            account_keys: wire
                .account_keys
                .into_iter()
                .map(Pubkey::new_from_array)
                .collect(),
            instructions: wire
                .instructions
                .into_iter()
                .map(
                    |instruction| solana_message::compiled_instruction::CompiledInstruction {
                        program_id_index: instruction.program_id_index,
                        accounts: instruction.account_indexes,
                        data: instruction.data,
                    },
                )
                .collect(),
            lookups: Vec::new(),
        }
        .checked()
    }

    /// Each instruction with its accounts resolved and flagged. `None` when
    /// one names an account the message does not have.
    pub(super) fn instructions(&self) -> Option<Vec<Instruction>> {
        self.instructions_with(&Loaded::default())
    }

    /// [`Self::instructions`], with the addresses its lookups name read
    /// from `loaded`.
    pub(super) fn instructions_with(&self, loaded: &Loaded) -> Option<Vec<Instruction>> {
        self.instructions
            .iter()
            .map(|compiled| {
                Some(Instruction {
                    program_id: self
                        .resolve(usize::from(compiled.program_id_index), loaded)?
                        .0,
                    accounts: compiled
                        .accounts
                        .iter()
                        .map(|index| {
                            let position = usize::from(*index);
                            let (pubkey, is_writable) = self.resolve(position, loaded)?;
                            Some(AccountMeta {
                                pubkey,
                                is_signer: position < usize::from(self.num_signers),
                                is_writable,
                            })
                        })
                        .collect::<Option<Vec<_>>>()?,
                    data: compiled.data.clone(),
                })
            })
            .collect()
    }

    /// The counterpart of [`Self::from_bytes`]. `None` when a count or a
    /// length is more than its field holds: more than 255 accounts or
    /// instructions, or more than 64 KiB of data in one instruction.
    pub(super) fn to_bytes(&self) -> Option<Vec<u8>> {
        wincode::serialize(&WireMessage {
            num_signers: self.num_signers,
            num_writable_signers: self.num_writable_signers,
            num_writable_non_signers: self.num_writable_non_signers,
            account_keys: self.account_keys.iter().map(Pubkey::to_bytes).collect(),
            instructions: self
                .instructions
                .iter()
                .map(|instruction| WireInstruction {
                    program_id_index: instruction.program_id_index,
                    account_indexes: instruction.accounts.clone(),
                    data: instruction.data.clone(),
                })
                .collect(),
            address_table_lookups: self
                .lookups
                .iter()
                .map(|lookup| WireLookup {
                    account_key: lookup.table.to_bytes(),
                    writable_indexes: lookup.writable.clone(),
                    readonly_indexes: lookup.readonly.clone(),
                })
                .collect(),
        })
        .ok()
    }
}

/// Squads' `TransactionMessage`, as the program lays it out. The IDL types
/// the argument that carries it as `bytes`, so the layout is written here:
/// one-byte counts, and two bytes (little-endian) for the length of an
/// instruction's data.
#[derive(SchemaWrite, SchemaRead)]
struct WireMessage {
    num_signers: u8,
    num_writable_signers: u8,
    num_writable_non_signers: u8,
    #[wincode(with = "containers::Vec<[u8; 32], UseIntLen<u8>>")]
    account_keys: Vec<[u8; 32]>,
    #[wincode(with = "containers::Vec<WireInstruction, UseIntLen<u8>>")]
    instructions: Vec<WireInstruction>,
    #[wincode(with = "containers::Vec<WireLookup, UseIntLen<u8>>")]
    address_table_lookups: Vec<WireLookup>,
}

/// A list of compiled instructions alone, as `executeTransactionSync`
/// carries them: a one-byte count, then each as in [`WireMessage`].
#[derive(SchemaWrite, SchemaRead)]
struct WireInstructions {
    #[wincode(with = "containers::Vec<WireInstruction, UseIntLen<u8>>")]
    instructions: Vec<WireInstruction>,
}

/// `instructions` as `executeTransactionSync` carries them. `None` when a
/// count or a length is more than its field holds.
pub(super) fn instructions_to_bytes(instructions: &[CompiledInstruction]) -> Option<Vec<u8>> {
    wincode::serialize(&WireInstructions {
        instructions: instructions
            .iter()
            .map(|instruction| WireInstruction {
                program_id_index: instruction.program_id_index,
                account_indexes: instruction.accounts.clone(),
                data: instruction.data.clone(),
            })
            .collect(),
    })
    .ok()
}

/// The counterpart of [`instructions_to_bytes`].
pub(super) fn instructions_from_bytes(bytes: &[u8]) -> Option<Vec<CompiledInstruction>> {
    let wire: WireInstructions = wincode::deserialize_exact(bytes).ok()?;
    Some(
        wire.instructions
            .into_iter()
            .map(|instruction| CompiledInstruction {
                program_id_index: instruction.program_id_index,
                accounts: instruction.account_indexes,
                data: instruction.data,
            })
            .collect(),
    )
}

#[derive(SchemaWrite, SchemaRead)]
struct WireInstruction {
    program_id_index: u8,
    #[wincode(with = "containers::Vec<u8, UseIntLen<u8>>")]
    account_indexes: Vec<u8>,
    #[wincode(with = "containers::Vec<u8, UseIntLen<u16>>")]
    data: Vec<u8>,
}

#[derive(SchemaWrite, SchemaRead)]
struct WireLookup {
    account_key: [u8; 32],
    #[wincode(with = "containers::Vec<u8, UseIntLen<u8>>")]
    writable_indexes: Vec<u8>,
    #[wincode(with = "containers::Vec<u8, UseIntLen<u8>>")]
    readonly_indexes: Vec<u8>,
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use solana_message::compiled_instruction::CompiledInstruction;

    fn message() -> VaultMessage {
        VaultMessage {
            num_signers: 1,
            num_writable_signers: 1,
            num_writable_non_signers: 1,
            account_keys: vec![
                Pubkey::new_from_array([1; 32]),
                Pubkey::new_from_array([2; 32]),
                Pubkey::new_from_array([3; 32]),
            ],
            instructions: vec![CompiledInstruction {
                program_id_index: 2,
                accounts: vec![0, 1],
                data: vec![9; 300],
            }],
            lookups: Vec::new(),
        }
    }

    #[test]
    fn the_stored_message_is_laid_out_as_squads_reads_it() {
        let mut expected = vec![1, 1, 1, 3];
        for key in 1..=3u8 {
            expected.extend_from_slice(&[key; 32]);
        }
        // One instruction: program 2, accounts 0 and 1, 300 bytes of data.
        expected.extend_from_slice(&[1, 2, 2, 0, 1]);
        expected.extend_from_slice(&300u16.to_le_bytes());
        expected.extend_from_slice(&[9; 300]);
        // No lookup tables.
        expected.push(0);

        let bytes = message().to_bytes().unwrap();
        assert_eq!(bytes, expected);
        let back = VaultMessage::from_bytes(&bytes).unwrap();
        assert_eq!(back.account_keys, message().account_keys);
        assert_eq!(back.instructions, message().instructions);
    }

    #[test]
    fn what_does_not_fit_or_does_not_read_is_refused() {
        let mut too_many = message();
        too_many.account_keys = (0..=255u8)
            .map(|n| Pubkey::new_from_array([n; 32]))
            .chain([Pubkey::new_unique()])
            .collect();
        assert_eq!(too_many.to_bytes(), None);

        let bytes = message().to_bytes().unwrap();
        assert!(VaultMessage::from_bytes(&bytes[..bytes.len() - 1]).is_none());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(VaultMessage::from_bytes(&longer).is_none());
        // A lookup table is not something a stored message here has.
        let mut with_lookup = bytes;
        *with_lookup.last_mut().unwrap() = 1;
        with_lookup.extend_from_slice(&[4; 32]);
        with_lookup.extend_from_slice(&[0, 0]);
        assert!(VaultMessage::from_bytes(&with_lookup).is_none());
    }
}
