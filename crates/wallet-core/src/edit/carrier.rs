//! Instructions that carry other instructions in their data, opened so what
//! they carry can be edited, and written again around the edit.

use crate::wallets::programmatic::{
    message_instructions, Authorization, Execution, ExecutorNonce, Submission,
    ED25519_SIGNER_PROGRAM_ID,
};
use crate::wallets::squads::StoredTransaction;
use crate::wallets::vector::{
    parse_passthrough, rewrite_passthrough, PassthroughSigner, PASSTHROUGH_DISCRIMINATOR,
    PROGRAM_ID as VECTOR_PROGRAM_ID,
};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;

/// What kind of instruction a [`Carrier`] is, and what it says about the
/// instructions it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CarrierKind {
    /// A Squads `vaultTransactionCreate` or `createTransaction`: what the
    /// vault runs once the proposal is approved.
    Squads { vault: Pubkey, vault_index: u8 },
    /// A Vector `Passthrough`: what runs as its accounts.
    Passthrough { signers: Vec<PassthroughSigner> },
    /// A programmatic signer's `Submit` and the executor's `Execute` inside
    /// it: what runs once the authorities have signed.
    Programmatic {
        authorities: Vec<Pubkey>,
        nonce_account: Pubkey,
    },
}

/// One instruction that carries others, as it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carrier {
    pub kind: CarrierKind,
    /// The instruction as it came.
    pub instruction: Instruction,
    /// What it carried, so an unchanged edit gives back the same bytes, and
    /// the signatures over them stay good.
    inner: Vec<Instruction>,
    /// For a `Submit`: whoever the execution message names first.
    payer: Option<Pubkey>,
    nonce: Option<ExecutorNonce>,
}

impl Carrier {
    /// `instruction` and what it carries, when it is one of the kinds this
    /// app writes again.
    pub fn open(instruction: &Instruction) -> Option<(Self, Vec<Instruction>)> {
        let carrier = if instruction.program_id == ED25519_SIGNER_PROGRAM_ID {
            open_submit(instruction)?
        } else if instruction.program_id == VECTOR_PROGRAM_ID
            && instruction.data.first() == Some(&PASSTHROUGH_DISCRIMINATOR)
        {
            let parsed = parse_passthrough(instruction).ok()?;
            Self {
                kind: CarrierKind::Passthrough {
                    signers: parsed.signers,
                },
                instruction: instruction.clone(),
                inner: parsed.instructions,
                payer: None,
                nonce: None,
            }
        } else {
            let stored = StoredTransaction::parse(instruction)?;
            Self {
                kind: CarrierKind::Squads {
                    vault: stored.vault,
                    vault_index: stored.vault_index,
                },
                instruction: instruction.clone(),
                inner: stored.instructions,
                payer: None,
                nonce: None,
            }
        };
        let inner = carrier.inner.clone();
        Some((carrier, inner))
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            CarrierKind::Squads { .. } => "Squads vault",
            CarrierKind::Passthrough { .. } => "Vector",
            CarrierKind::Programmatic { .. } => "Programmatic signer",
        }
    }

    pub fn instruction_label(&self) -> &'static str {
        match self.kind {
            CarrierKind::Squads { .. } => "Stored transaction",
            CarrierKind::Passthrough { .. } => "Passthrough",
            CarrierKind::Programmatic { .. } => "Submit · Execute",
        }
    }

    /// This instruction, carrying `instructions`. The same instructions give
    /// back the instruction as it came. Other instructions drop the
    /// signatures an authority made over the old ones: a `Submit` is written
    /// with empty places for them.
    pub fn wrap(&self, instructions: &[Instruction]) -> Result<Instruction, String> {
        if instructions == self.inner.as_slice() {
            return Ok(self.instruction.clone());
        }
        match &self.kind {
            CarrierKind::Squads { .. } => {
                StoredTransaction::rewrite(&self.instruction, instructions).ok_or_else(|| {
                    "These instructions do not fit a transaction stored for a Squads vault."
                        .to_string()
                })
            }
            CarrierKind::Passthrough { .. } => {
                rewrite_passthrough(&self.instruction, instructions).map_err(|err| err.to_string())
            }
            CarrierKind::Programmatic { authorities, .. } => {
                let (Some(payer), Some(nonce)) = (self.payer, self.nonce) else {
                    return Err("This Submit cannot be written again.".to_string());
                };
                Authorization::new(payer, instructions, authorities, nonce)
                    .and_then(|authorization| {
                        authorization.submit_instruction_unsigned(&BTreeMap::new())
                    })
                    .map_err(|err| err.to_string())
            }
        }
    }
}

/// A `Submit` whose authorization message holds one `Execute`. The two are
/// opened as one, since the `Execute` follows from what it runs.
fn open_submit(instruction: &Instruction) -> Option<Carrier> {
    let submission = Submission::parse(&instruction.data)?;
    let [execute] = message_instructions(&submission.message).try_into().ok()?;
    let execution = Execution::parse(&execute)?;
    let nonce = ExecutorNonce {
        account: execution.nonce_account,
        authority: execution.nonce_authority,
        value: execution.nonce_value,
    };
    let authorities: Vec<Pubkey> = submission.authorities().map(|(key, _)| key).collect();
    Some(Carrier {
        kind: CarrierKind::Programmatic {
            authorities,
            nonce_account: nonce.account,
        },
        instruction: instruction.clone(),
        inner: execution.instructions(),
        payer: execution.message.account_keys.first().copied(),
        nonce: Some(nonce),
    })
}
