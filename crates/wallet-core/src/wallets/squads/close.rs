//! Closing a transaction's accounts: the stored transaction and its proposal,
//! whose rent goes back to a collector.
//!
//! Both programs close the two together, in one instruction no member signs:
//! anyone who pays the fee may send it, and the rent goes where the program
//! says, never to the sender. The two differ in where that is:
//!
//! - v4 sends the rent of both to the multisig's rent collector, a setting of
//!   the multisig. A multisig without one cannot have its accounts closed.
//! - The smart account program keeps no such setting: each account records
//!   who paid its rent (`rentCollector`), and the rent goes back there.
//!
//! A vault transaction and a change to the multisig are closed by sibling
//! instructions. A batch is closed one stored transaction at a time first,
//! which this app does not do. When the program allows a close is decided in
//! `proposals::available_close`.

use super::*;

/// Which instruction closes a stored transaction's accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseKind {
    /// A vault transaction: v4 `vaultTransactionAccountsClose`, smart account
    /// `closeTransaction`.
    Vault,
    /// A change to the multisig: v4 `configTransactionAccountsClose`, smart
    /// account `closeSettingsTransaction`.
    Config,
}

/// What a transaction account says about closing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransactionClosing {
    pub kind: CloseKind,
    pub index: u64,
    /// Smart account: who paid its rent, and gets it back. `None` on v4,
    /// where the multisig's rent collector does.
    pub rent_collector: Option<Pubkey>,
}

impl Squad {
    /// How the transaction account `data` of this multisig is closed. `None`
    /// when it is not one this app closes: a batch, another multisig's, or
    /// nothing the program knows. A change holding an action this app cannot
    /// read is closed all the same: closing needs no reading.
    pub fn transaction_closing(&self, data: &[u8]) -> Option<TransactionClosing> {
        let candidates: [(&str, &str, CloseKind); 2] = match self.program {
            SquadsProgram::V4 => [
                ("vaultTransaction", "multisig", CloseKind::Vault),
                ("configTransaction", "multisig", CloseKind::Config),
            ],
            SquadsProgram::SmartAccount => [
                ("transaction", "consensusAccount", CloseKind::Vault),
                ("settingsTransaction", "settings", CloseKind::Config),
            ],
        };
        candidates.into_iter().find_map(|(name, owner, kind)| {
            let account = self.program.account(name, data)?;
            if key(field(&account, owner)?)? != self.address {
                return None;
            }
            let rent_collector = match self.program {
                SquadsProgram::V4 => None,
                SquadsProgram::SmartAccount => Some(key(field(&account, "rentCollector")?)?),
            };
            Some(TransactionClosing {
                kind,
                index: number(field(&account, "index")?)?,
                rent_collector,
            })
        })
    }

    /// Smart account: who paid the rent of proposal account `data`, and gets
    /// it back when it is closed. `None` on v4, whose proposals record none,
    /// and for anything that is not a proposal of this multisig.
    pub fn proposal_rent_collector(&self, data: &[u8]) -> Option<Pubkey> {
        if self.program != SquadsProgram::SmartAccount {
            return None;
        }
        let proposal = self.program.account("proposal", data)?;
        if key(field(&proposal, "settings")?)? != self.address {
            return None;
        }
        key(field(&proposal, "rentCollector")?)
    }

    /// Close transaction `index` and its proposal, if it has one. No one
    /// signs it but the fee payer, who may be anyone.
    ///
    /// v4 sends the rent of both accounts to `transaction_collector`, which
    /// must be the multisig's rent collector (`proposal_collector` is not
    /// used). The smart account program sends each account's rent to the one
    /// it recorded: `proposal_collector` for the proposal (unchecked when
    /// there is no proposal) and `transaction_collector` for the transaction.
    pub fn close(
        &self,
        index: u64,
        kind: CloseKind,
        proposal_collector: &Pubkey,
        transaction_collector: &Pubkey,
    ) -> Instruction {
        let proposal = ("proposal", self.proposal(index));
        let transaction = ("transaction", self.transaction(index));
        match self.program {
            SquadsProgram::V4 => self.program.instruction(
                match kind {
                    CloseKind::Vault => "vaultTransactionAccountsClose",
                    CloseKind::Config => "configTransactionAccountsClose",
                },
                &[
                    ("multisig", self.address),
                    proposal,
                    transaction,
                    ("rentCollector", *transaction_collector),
                    ("systemProgram", SYSTEM_PROGRAM_ID),
                ],
                None,
            ),
            SquadsProgram::SmartAccount => {
                let (name, owner) = match kind {
                    CloseKind::Vault => ("closeTransaction", "consensusAccount"),
                    CloseKind::Config => ("closeSettingsTransaction", "settings"),
                };
                self.program.instruction(
                    name,
                    &[
                        (owner, self.address),
                        proposal,
                        transaction,
                        ("proposalRentCollector", *proposal_collector),
                        ("transactionRentCollector", *transaction_collector),
                        ("systemProgram", SYSTEM_PROGRAM_ID),
                        ("program", self.program.program_id()),
                    ],
                    None,
                )
            }
        }
    }
}
