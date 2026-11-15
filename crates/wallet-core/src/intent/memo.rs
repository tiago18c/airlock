//! A message signed as a memo: a transaction whose one instruction is the
//! Memo program's, with the account its only account and a signer, and the
//! message its data. An account with no key signs that, where it could not
//! sign the message itself.

use crate::edit::Carrier;
use crate::error::BuildError;
use crate::ids::{COMPUTE_BUDGET_PROGRAM_ID, MEMO_PROGRAM_ID};
use crate::tx::builder::compile_v1;
use crate::tx::decode::{DecodedTx, TxFormat};
use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_message::{Message as LegacyMessage, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;
use thiserror::Error;

/// The most bytes a message signed as a memo may hold.
pub const MAX_MEMO_BYTES: usize = 512;

/// Why a message cannot be a memo. On the wire it is `unsupported`, with
/// the requirement `messages`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MemoError {
    #[error("The message is empty.")]
    Empty,
    #[error("The message is not UTF-8 text, so it cannot be signed as a memo.")]
    NotText,
    #[error("The message is {0} bytes, and a memo holds at most 512.")]
    TooLong(usize),
}

/// `message` as the text of a memo: UTF-8, not empty, at most
/// [`MAX_MEMO_BYTES`].
pub fn memo_text(message: &[u8]) -> Result<&str, MemoError> {
    if message.is_empty() {
        return Err(MemoError::Empty);
    }
    if message.len() > MAX_MEMO_BYTES {
        return Err(MemoError::TooLong(message.len()));
    }
    std::str::from_utf8(message).map_err(|_| MemoError::NotText)
}

/// The Memo instruction that signs `message` as `account`.
pub fn memo_instruction(account: Pubkey, message: &str) -> Instruction {
    Instruction {
        program_id: MEMO_PROGRAM_ID,
        accounts: vec![AccountMeta::new_readonly(account, true)],
        data: message.as_bytes().to_vec(),
    }
}

/// The format a message signed as `account` goes in as a memo:
/// [`TxFormat::Legacy`] when `key_here` (a key of this device signs as the
/// account, at the top of the transaction), [`TxFormat::V1`] otherwise.
///
/// A node returns a legacy transaction by its signature to any app
/// (`getTransaction`), so where it can be, the memo is legacy. A wallet
/// without a key is rebuilt into v1, and only a v1 transaction can be handed
/// to another device, so every other account's memo is v1 from the start; a
/// node shows it only when asked for version 1 (`maxSupportedTransactionVersion`),
/// and one that does not know version 1 does not show it at all.
pub fn memo_format(key_here: bool) -> TxFormat {
    if key_here {
        TxFormat::Legacy
    } else {
        TxFormat::V1
    }
}

/// The unsigned wire of the transaction, in `format` (legacy or v1, see
/// [`memo_format`]), that signs `message` as `account`, paid by
/// `fee_payer`, on `blockhash` (the default hash when `None`, for a
/// transaction that is moved onto a fresh one before signing). Resolved like
/// any request with delivery `send`: a keyless account's rebuild wraps it.
pub fn memo_transaction(
    account: Pubkey,
    message: &str,
    fee_payer: Pubkey,
    blockhash: Option<Hash>,
    format: TxFormat,
) -> Result<Vec<u8>, BuildError> {
    let instruction = memo_instruction(account, message);
    if format != TxFormat::Legacy {
        return Ok(compile_v1(
            fee_payer,
            vec![instruction],
            None,
            Some(blockhash.unwrap_or_default()),
        )?
        .wire);
    }
    let mut legacy = LegacyMessage::new(&[instruction], Some(&fee_payer));
    legacy.recent_blockhash = blockhash.unwrap_or_default();
    let signers = usize::from(legacy.header.num_required_signatures);
    wincode::serialize(&VersionedTransaction {
        signatures: vec![Signature::default(); signers],
        message: VersionedMessage::Legacy(legacy),
    })
    .map_err(|err| BuildError::Message(err.to_string()))
}

/// Whether `tx` is exactly a memo of `message` signed as `account`: its own
/// instructions, compute budget aside, are that one Memo instruction, or a
/// carrier in it carries exactly that, at any depth. Read to label a
/// request, never to decide what is signed.
pub fn is_memo_of(tx: &DecodedTx, account: &Pubkey, message: &[u8]) -> bool {
    let own: Vec<Instruction> = tx
        .native_instructions()
        .into_iter()
        .filter(|ix| ix.program_id != COMPUTE_BUDGET_PROGRAM_ID)
        .collect();
    carries_memo(&own, account, message)
}

fn carries_memo(list: &[Instruction], account: &Pubkey, message: &[u8]) -> bool {
    if let [only] = list {
        if is_memo_instruction(only, account, message) {
            return true;
        }
    }
    list.iter().any(|ix| {
        Carrier::open(ix).is_some_and(|(_, inner)| carries_memo(&inner, account, message))
    })
}

fn is_memo_instruction(ix: &Instruction, account: &Pubkey, message: &[u8]) -> bool {
    ix.program_id == MEMO_PROGRAM_ID
        && ix.data == message
        && matches!(ix.accounts.as_slice(), [meta] if meta.pubkey == *account && meta.is_signer)
}
