//! A smart account that runs instructions at once, with no stored
//! transaction and no proposal: `executeTransactionSync`.
//!
//! The members that sign the one instruction stand in for the votes. The
//! program takes them as they are, provided that:
//!
//! - the multisig has no time lock;
//! - every one of them is a member and signs, each once;
//! - together they hold every permission (initiate, vote, execute);
//! - at least as many of them may vote as the threshold asks.
//!
//! The instruction names the multisig and the program, then lists the
//! members that sign, then every account of the instructions it runs, as a
//! stored message lists them (the vault first). The instructions travel in
//! its data, compiled against that whole list, members included.
//!
//! v4 has no such instruction.

use super::*;
use solana_message::compiled_instruction::CompiledInstruction;

/// What one `executeTransactionSync` runs, read back from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncExecution {
    pub squad: Squad,
    pub vault_index: u8,
    /// The vault that runs the instructions and signs for them.
    pub vault: Pubkey,
    /// The members that sign in place of votes, in their order.
    pub signers: Vec<Pubkey>,
    pub instructions: Vec<Instruction>,
}

impl Squad {
    /// Run `instructions` as vault `vault_index` in this transaction,
    /// approved by `signers`, who sign it. `None` for a v4 multisig, which
    /// cannot, and when the members and the accounts are more than one
    /// instruction can number.
    ///
    /// Whether `signers` are enough is for the caller to know: see the
    /// module's notes.
    pub fn execute_sync(
        &self,
        vault_index: u8,
        instructions: &[Instruction],
        signers: &[Pubkey],
    ) -> Option<Instruction> {
        if self.program != SquadsProgram::SmartAccount {
            return None;
        }
        let message = vault_message(&self.vault(vault_index), instructions);
        let num_signers = u8::try_from(signers.len()).ok()?;
        // The instructions number the accounts after the members.
        let compiled = message
            .instructions
            .iter()
            .map(|instruction| {
                let shift = |index: u8| index.checked_add(num_signers);
                Some(CompiledInstruction {
                    program_id_index: shift(instruction.program_id_index)?,
                    accounts: instruction
                        .accounts
                        .iter()
                        .map(|index| shift(*index))
                        .collect::<Option<Vec<u8>>>()?,
                    data: instruction.data.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let mut instruction = self.program.instruction(
            "executeTransactionSync",
            &[
                ("consensusAccount", self.address),
                ("program", self.program.program_id()),
            ],
            Some(record([
                ("accountIndex", vault_index.into()),
                ("numSigners", num_signers.into()),
                (
                    "instructions",
                    Value::Bytes(instructions_to_bytes(&compiled)?),
                ),
            ])),
        );
        instruction.accounts.extend(
            signers
                .iter()
                .map(|signer| AccountMeta::new_readonly(*signer, true)),
        );
        instruction.accounts.extend(message.execute_accounts());
        Some(instruction)
    }
}

impl SyncExecution {
    /// `None` when `instruction` is not a well-formed smart account
    /// `executeTransactionSync`.
    pub fn parse(instruction: &Instruction) -> Option<Self> {
        let program = SquadsProgram::from_program_id(&instruction.program_id)?;
        if program != SquadsProgram::SmartAccount {
            return None;
        }
        let parsed = program.client().parse_instruction(instruction).ok()??;
        match parsed.path.last()? {
            codama_nodes::Node::Instruction(node)
                if node.name.as_ref() == "executeTransactionSync" => {}
            _ => return None,
        }
        let args = field(&parsed.data, "args")?;
        let vault_index: u8 = number(field(args, "accountIndex")?)?;
        let num_signers: usize = number(field(args, "numSigners")?)?;
        let compiled = instructions_from_bytes(bytes(field(args, "instructions")?)?)?;

        let squad = Squad {
            program,
            address: instruction.accounts.first()?.pubkey,
        };
        let vault = squad.vault(vault_index);
        // Everything after the multisig and the program, read straight from the
        // instruction rather than from how the IDL groups it.
        let listed = instruction.accounts.get(2..)?;
        let signers: Vec<Pubkey> = listed
            .get(..num_signers)?
            .iter()
            .map(|meta| meta.pubkey)
            .collect();
        // As the program passes each account on: the vault signs, the
        // members that signed here do not, and the multisig is not written.
        let meta = |index: u8| {
            let listed = listed.get(usize::from(index))?;
            Some(AccountMeta {
                pubkey: listed.pubkey,
                is_signer: listed.pubkey == vault
                    || (listed.is_signer && !signers.contains(&listed.pubkey)),
                is_writable: listed.is_writable && listed.pubkey != squad.address,
            })
        };
        let instructions = compiled
            .iter()
            .map(|compiled| {
                Some(Instruction {
                    program_id: listed.get(usize::from(compiled.program_id_index))?.pubkey,
                    accounts: compiled
                        .accounts
                        .iter()
                        .map(|index| meta(*index))
                        .collect::<Option<Vec<_>>>()?,
                    data: compiled.data.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            squad,
            vault_index,
            vault,
            signers,
            instructions,
        })
    }

    /// `instruction`, running `instructions` in place of what it runs: the
    /// same multisig, vault and members. `None` when it is not one
    /// [`Self::parse`] reads, or the instructions name more accounts than it
    /// can number.
    pub fn rewrite(instruction: &Instruction, instructions: &[Instruction]) -> Option<Instruction> {
        let sync = Self::parse(instruction)?;
        sync.squad
            .execute_sync(sync.vault_index, instructions, &sync.signers)
    }
}
