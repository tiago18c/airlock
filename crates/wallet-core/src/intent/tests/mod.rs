//! A site's request that creates a mint and its owner's token account, in
//! each format, and the notes it sends with it.

mod notes;
mod plan;
mod profile;
mod resolution;
mod rewrite;
mod role;

use super::*;
use crate::fixtures::{blockhash, pk};
use crate::ids::{ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::spl::{associated_token_address, create_ata_idempotent};
use crate::tx::builder::compile_v1;
use crate::tx::decode::{decode_transaction, DecodedTx, LookupTables};
use solana_instruction::{AccountMeta, Instruction};
use solana_message::v0::Message as V0Message;
use solana_message::{AddressLookupTableAccount, Message as LegacyMessage, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;

/// A program of the site's that reads a token account and its bump.
const SITE_PROGRAM: Pubkey = Pubkey::new_from_array([77; 32]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Legacy,
    V0,
    V1,
}

const FORMATS: [Format; 3] = [Format::Legacy, Format::V0, Format::V1];

/// The request's accounts: the sponsor that pays, the owner (the account
/// acted as), the new mint, and the owner's token account for it.
struct Site {
    payer: Pubkey,
    owner: Pubkey,
    mint: Pubkey,
}

impl Site {
    fn new() -> Self {
        Self {
            payer: pk(1),
            owner: pk(2),
            mint: pk(3),
        }
    }

    fn ata(&self) -> Pubkey {
        associated_token_address(&self.owner, &self.mint, &TOKEN_PROGRAM_ID)
    }

    fn bump(&self) -> u8 {
        Pubkey::find_program_address(
            &[
                self.owner.as_ref(),
                TOKEN_PROGRAM_ID.as_ref(),
                self.mint.as_ref(),
            ],
            &ASSOCIATED_TOKEN_PROGRAM_ID,
        )
        .1
    }

    /// Create the mint, initialize it (its address in the data at 1, as a
    /// metadata pointer would name it), create the owner's token account,
    /// and hand the token account and its bump to the site's program.
    fn instructions(&self) -> Vec<Instruction> {
        let create = solana_system_interface::instruction::create_account(
            &self.payer,
            &self.mint,
            1_000,
            82,
            &TOKEN_PROGRAM_ID,
        );
        let mut init_data = vec![20u8];
        init_data.extend_from_slice(self.mint.as_ref());
        let init = Instruction {
            program_id: TOKEN_PROGRAM_ID,
            accounts: vec![AccountMeta::new(self.mint, false)],
            data: init_data,
        };
        let ata = create_ata_idempotent(self.payer, self.owner, self.mint, TOKEN_PROGRAM_ID);
        let read = Instruction {
            program_id: SITE_PROGRAM,
            accounts: vec![
                AccountMeta::new(self.ata(), false),
                AccountMeta::new_readonly(self.owner, true),
            ],
            data: vec![9, self.bump()],
        };
        vec![create, init, ata, read]
    }

    fn notes(&self) -> Notes {
        Notes {
            accounts: AccountNotes(vec![
                (self.mint, AccountNote::New { replaceable: true }),
                (
                    self.ata(),
                    AccountNote::Derived(Derivation::Pda(Pda {
                        program: ASSOCIATED_TOKEN_PROGRAM_ID,
                        seeds: vec![
                            Seed::Account(self.owner),
                            Seed::Bytes(TOKEN_PROGRAM_ID.to_bytes().to_vec()),
                            Seed::Account(self.mint),
                        ],
                    })),
                ),
                (
                    self.payer,
                    AccountNote::FeePayer {
                        replaceable: true,
                        signs: Some(SignsBy::App),
                    },
                ),
            ]),
            data: vec![
                DataNote {
                    transaction: 0,
                    instruction: 1,
                    offset: 1,
                    holds: DataHolds::Account(self.mint),
                },
                DataNote {
                    transaction: 0,
                    instruction: 3,
                    offset: 1,
                    holds: DataHolds::BumpOf(self.ata()),
                },
            ],
        }
    }

    fn wire(&self, format: Format) -> Vec<u8> {
        wire_of(format, &self.instructions(), &self.payer, &[])
    }
}

/// `instructions`, paid by `payer`, laid out as `format`, with every slot
/// empty. A v0 message reads what `tables` hold from them.
fn wire_of(
    format: Format,
    instructions: &[Instruction],
    payer: &Pubkey,
    tables: &[AddressLookupTableAccount],
) -> Vec<u8> {
    let message = match format {
        Format::Legacy => {
            let mut message = LegacyMessage::new(instructions, Some(payer));
            message.recent_blockhash = blockhash(1);
            VersionedMessage::Legacy(message)
        }
        Format::V0 => VersionedMessage::V0(
            V0Message::try_compile(payer, instructions, tables, blockhash(1)).unwrap(),
        ),
        Format::V1 => {
            return compile_v1(*payer, instructions.to_vec(), None, Some(blockhash(1)))
                .unwrap()
                .wire
        }
    };
    let signers = usize::from(message.header().num_required_signatures);
    wincode::serialize(&VersionedTransaction {
        signatures: vec![Signature::default(); signers],
        message,
    })
    .unwrap()
}

fn decoded(wire: &[u8]) -> DecodedTx {
    decode_transaction(wire, &LookupTables::new()).unwrap()
}
