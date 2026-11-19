//! The instructions that create a multisig and move a proposal along.

use super::*;

impl Squad {
    /// Create a v4 multisig. `create_key` and `creator` sign.
    pub fn create_v4(
        create_key: &Pubkey,
        creator: &Pubkey,
        treasury: &Pubkey,
        setup: &SquadsSetup,
    ) -> (Self, Instruction) {
        let squad = Self::v4(create_key);
        let program = squad.program;
        let instruction = program.instruction(
            "multisigCreateV2",
            &[
                ("programConfig", program.program_config()),
                ("treasury", *treasury),
                ("multisig", squad.address),
                ("createKey", *create_key),
                ("creator", *creator),
                ("systemProgram", SYSTEM_PROGRAM_ID),
            ],
            Some(setup_args(setup, "configAuthority", "members")),
        );
        (squad, instruction)
    }

    /// Create smart account number `number`. `creator` signs.
    ///
    /// The number has to be the program's counter plus one at the moment the
    /// transaction lands. If someone else creates one first, this fails and has
    /// to be rebuilt.
    pub fn create_smart_account(
        number: u128,
        creator: &Pubkey,
        treasury: &Pubkey,
        setup: &SquadsSetup,
    ) -> (Self, Instruction) {
        let squad = Self::smart_account(number);
        let program = squad.program;
        let mut instruction = program.instruction(
            "createSmartAccount",
            &[
                ("programConfig", program.program_config()),
                ("treasury", *treasury),
                ("creator", *creator),
                ("systemProgram", SYSTEM_PROGRAM_ID),
                ("program", program.program_id()),
            ],
            Some(setup_args(setup, "settingsAuthority", "signers")),
        );
        // The new settings account follows the accounts the IDL names.
        instruction
            .accounts
            .push(AccountMeta::new(squad.address, false));
        (squad, instruction)
    }

    /// Store `instructions`, to be run later as vault `vault_index`, as
    /// transaction number `index`. `creator` is a member and signs.
    /// `rent_payer` signs and pays for the account. `None` when the
    /// instructions name more accounts or hold more than a stored message can.
    pub fn create_transaction(
        &self,
        index: u64,
        vault_index: u8,
        instructions: &[Instruction],
        creator: &Pubkey,
        rent_payer: &Pubkey,
    ) -> Option<Instruction> {
        let message = vault_message(&self.vault(vault_index), instructions).to_bytes()?;
        Some(self.store(index, vault_index, message, 0, creator, rent_payer))
    }

    /// [`Self::create_transaction`], reading every address `tables` hold
    /// from them, and with `ephemeral_signers` addresses the program derives
    /// for the transaction and signs as (see [`Self::ephemeral_signer`]).
    /// This is how other apps store a transaction too large to list every
    /// account itself. `None` when the instructions do not compile so.
    #[allow(clippy::too_many_arguments)]
    pub fn create_transaction_with_tables(
        &self,
        index: u64,
        vault_index: u8,
        instructions: &[Instruction],
        tables: &[(Pubkey, Vec<Pubkey>)],
        ephemeral_signers: u8,
        creator: &Pubkey,
        rent_payer: &Pubkey,
    ) -> Option<Instruction> {
        let message = vault_message_with_tables(&self.vault(vault_index), instructions, tables)?
            .to_bytes()?;
        Some(self.store(
            index,
            vault_index,
            message,
            ephemeral_signers,
            creator,
            rent_payer,
        ))
    }

    fn store(
        &self,
        index: u64,
        vault_index: u8,
        message: Vec<u8>,
        ephemeral_signers: u8,
        creator: &Pubkey,
        rent_payer: &Pubkey,
    ) -> Instruction {
        let stored = |index_name: &str| {
            record([
                (index_name, vault_index.into()),
                ("ephemeralSigners", ephemeral_signers.into()),
                ("transactionMessage", Value::Bytes(message.clone())),
                ("memo", Value::Option(None)),
            ])
        };
        match self.program {
            SquadsProgram::V4 => self.program.instruction(
                "vaultTransactionCreate",
                &[
                    ("multisig", self.address),
                    ("transaction", self.transaction(index)),
                    ("creator", *creator),
                    ("rentPayer", *rent_payer),
                    ("systemProgram", SYSTEM_PROGRAM_ID),
                ],
                Some(stored("vaultIndex")),
            ),
            SquadsProgram::SmartAccount => self.program.instruction(
                "createTransaction",
                &[
                    ("consensusAccount", self.address),
                    ("transaction", self.transaction(index)),
                    ("creator", *creator),
                    ("rentPayer", *rent_payer),
                    ("systemProgram", SYSTEM_PROGRAM_ID),
                    ("program", self.program.program_id()),
                ],
                Some(Value::Enum {
                    variant: "TransactionPayload".into(),
                    fields: Box::new(record([(
                        "fields",
                        Value::Tuple(vec![stored("accountIndex")]),
                    )])),
                }),
            ),
        }
    }

    /// Open voting on transaction `index`.
    pub fn create_proposal(
        &self,
        index: u64,
        creator: &Pubkey,
        rent_payer: &Pubkey,
    ) -> Instruction {
        // Not a draft: open for votes at once.
        self.proposal_of(index, creator, rent_payer, false)
    }

    /// A proposal for transaction `index` that is a draft: no one can vote on
    /// it until a member that may propose opens it ([`Self::activate`]). The
    /// app makes none of its own; other apps do, and the tests need them.
    pub fn create_draft(&self, index: u64, creator: &Pubkey, rent_payer: &Pubkey) -> Instruction {
        self.proposal_of(index, creator, rent_payer, true)
    }

    fn proposal_of(
        &self,
        index: u64,
        creator: &Pubkey,
        rent_payer: &Pubkey,
        draft: bool,
    ) -> Instruction {
        let args = record([("transactionIndex", index.into()), ("draft", draft.into())]);
        let mut accounts = vec![
            (self.owner_account(), self.address),
            ("proposal", self.proposal(index)),
            ("creator", *creator),
            ("rentPayer", *rent_payer),
            ("systemProgram", SYSTEM_PROGRAM_ID),
        ];
        let name = match self.program {
            SquadsProgram::V4 => "proposalCreate",
            SquadsProgram::SmartAccount => {
                accounts.push(("program", self.program.program_id()));
                "createProposal"
            }
        };
        self.program.instruction(name, &accounts, Some(args))
    }

    /// Vote yes on proposal `index`. `member` signs.
    pub fn approve(&self, index: u64, member: &Pubkey) -> Instruction {
        self.vote(index, member, ["proposalApprove", "approveProposal"])
    }

    /// Vote no on proposal `index`, which must be open for votes. `member`
    /// signs. A rejection takes back the member's approval, if it gave one.
    /// The proposal is rejected once the rejections reach the cutoff (see
    /// `proposals::rejection_cutoff`).
    pub fn reject(&self, index: u64, member: &Pubkey) -> Instruction {
        self.vote(index, member, ["proposalReject", "rejectProposal"])
    }

    /// Open draft proposal `index` for votes. `member` signs. Both programs
    /// take it from any member that may propose (not only the draft's
    /// creator), on a draft that is not stale.
    pub fn activate(&self, index: u64, member: &Pubkey) -> Instruction {
        let proposal = ("proposal", self.proposal(index));
        match self.program {
            SquadsProgram::V4 => self.program.instruction(
                "proposalActivate",
                &[("multisig", self.address), ("member", *member), proposal],
                None,
            ),
            SquadsProgram::SmartAccount => self.program.instruction(
                "activateProposal",
                &[("settings", self.address), ("signer", *member), proposal],
                None,
            ),
        }
    }

    /// Vote to cancel approved proposal `index`. `member` signs. The proposal
    /// is cancelled once the cancel votes of members reach the threshold.
    ///
    /// Both programs grow the proposal account when the multisig has more
    /// members now than when it was made, and the member pays the rent of
    /// the growth, through the system program both are given. On v4 that is
    /// `proposalCancelV2`: `proposalCancel` cannot grow the account, so it
    /// fails once the cancel votes no longer fit.
    pub fn cancel(&self, index: u64, member: &Pubkey) -> Instruction {
        let args = record([("memo", Value::Option(None))]);
        let proposal = ("proposal", self.proposal(index));
        match self.program {
            SquadsProgram::V4 => self.program.instruction(
                "proposalCancelV2",
                &[
                    ("multisig", self.address),
                    ("member", *member),
                    proposal,
                    ("systemProgram", SYSTEM_PROGRAM_ID),
                ],
                Some(args),
            ),
            // Its system program is optional in the IDL, and the program
            // refuses a cancel without it.
            SquadsProgram::SmartAccount => self.program.instruction(
                "cancelProposal",
                &[
                    ("consensusAccount", self.address),
                    ("signer", *member),
                    proposal,
                    ("systemProgram", SYSTEM_PROGRAM_ID),
                    ("program", self.program.program_id()),
                ],
                Some(args),
            ),
        }
    }

    /// A vote on proposal `index`: both programs take the same accounts for
    /// approving and rejecting. `names` is the instruction's name in v4 and
    /// in the smart account program.
    fn vote(&self, index: u64, member: &Pubkey, names: [&str; 2]) -> Instruction {
        let args = record([("memo", Value::Option(None))]);
        let proposal = ("proposal", self.proposal(index));
        match self.program {
            SquadsProgram::V4 => self.program.instruction(
                names[0],
                &[("multisig", self.address), ("member", *member), proposal],
                Some(args),
            ),
            // The optional system program is left out.
            SquadsProgram::SmartAccount => self.program.instruction(
                names[1],
                &[
                    ("consensusAccount", self.address),
                    ("signer", *member),
                    proposal,
                    ("program", self.program.program_id()),
                ],
                Some(args),
            ),
        }
    }

    /// Run approved transaction `index`. `member` signs. `instructions` and
    /// `vault_index` must be the ones the transaction was stored with.
    pub fn execute(
        &self,
        index: u64,
        vault_index: u8,
        instructions: &[Instruction],
        member: &Pubkey,
    ) -> Instruction {
        let vault = self.vault(vault_index);
        let message = vault_message(&vault, instructions);
        self.execute_listing(index, member, message.execute_accounts())
    }

    /// Run approved transaction `stored`, as read from its account by
    /// [`Self::parse_transaction`]. `member` signs.
    pub fn execute_stored(&self, stored: &VaultTransaction, member: &Pubkey) -> Instruction {
        self.execute_listing(stored.index, member, stored.execute_accounts.clone())
    }

    pub(super) fn execute_listing(
        &self,
        index: u64,
        member: &Pubkey,
        stored_accounts: Vec<AccountMeta>,
    ) -> Instruction {
        let mut accounts = vec![
            (self.owner_account(), self.address),
            ("proposal", self.proposal(index)),
            ("transaction", self.transaction(index)),
        ];
        let name = match self.program {
            SquadsProgram::V4 => {
                accounts.push(("member", *member));
                "vaultTransactionExecute"
            }
            SquadsProgram::SmartAccount => {
                accounts.push(("signer", *member));
                accounts.push(("program", self.program.program_id()));
                "executeTransaction"
            }
        };
        let mut instruction = self.program.instruction(name, &accounts, None);
        // Then every account of the stored message, in its order.
        instruction.accounts.extend(stored_accounts);
        instruction
    }
}

/// Creation arguments. The two programs name two of the fields differently:
/// the authority over the configuration, threshold, members, time lock, the
/// rent collector, no memo.
pub(super) fn setup_args(setup: &SquadsSetup, authority: &str, members: &str) -> Value {
    let maybe_key = |key: Option<Pubkey>| {
        Value::Option(key.map(|key| Box::new(Value::PublicKey(key.to_bytes()))))
    };
    let list = setup
        .members
        .iter()
        .map(|member| {
            record([
                ("key", Value::PublicKey(member.key.to_bytes())),
                ("permissions", record([("mask", member.permissions.into())])),
            ])
        })
        .collect();
    record([
        (authority, maybe_key(setup.config_authority)),
        ("threshold", setup.threshold.into()),
        (members, Value::Array(list)),
        ("timeLock", setup.time_lock.into()),
        ("rentCollector", maybe_key(setup.rent_collector)),
        ("memo", Value::Option(None)),
    ])
}
