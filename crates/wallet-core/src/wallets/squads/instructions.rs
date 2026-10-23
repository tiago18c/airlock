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
        let stored = |index_name: &str| {
            record([
                (index_name, vault_index.into()),
                // No extra program-derived signers.
                ("ephemeralSigners", 0u8.into()),
                ("transactionMessage", Value::Bytes(message.clone())),
                ("memo", Value::Option(None)),
            ])
        };
        Some(match self.program {
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
        })
    }

    /// Open voting on transaction `index`.
    pub fn create_proposal(
        &self,
        index: u64,
        creator: &Pubkey,
        rent_payer: &Pubkey,
    ) -> Instruction {
        let args = record([
            ("transactionIndex", index.into()),
            // Not a draft: open for votes at once.
            ("draft", false.into()),
        ]);
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
        let args = record([("memo", Value::Option(None))]);
        let proposal = ("proposal", self.proposal(index));
        match self.program {
            SquadsProgram::V4 => self.program.instruction(
                "proposalApprove",
                &[("multisig", self.address), ("member", *member), proposal],
                Some(args),
            ),
            // The optional system program is left out.
            SquadsProgram::SmartAccount => self.program.instruction(
                "approveProposal",
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
/// no authority over the configuration, threshold, members, time lock, no
/// rent collector, no memo.
pub(super) fn setup_args(setup: &SquadsSetup, authority: &str, members: &str) -> Value {
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
        (authority, Value::Option(None)),
        ("threshold", setup.threshold.into()),
        (members, Value::Array(list)),
        ("timeLock", setup.time_lock.into()),
        ("rentCollector", Value::Option(None)),
        ("memo", Value::Option(None)),
    ])
}
