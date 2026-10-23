//! Reading a `Submit` instruction back: who signs it and what it runs.

use super::*;

/// One signer of a `Submit`, as its data states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmitSigner {
    pub pubkey: Pubkey,
    pub role: SubmitRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitRole {
    /// An ordinary key. It signs the transaction that carries the `Submit`.
    Forwarded,
    /// A key that signs the authorization message. Its derived address is
    /// what the wrapped instructions see as the signer.
    Authority {
        /// The signature is there and matches the authorization message.
        signed: bool,
        /// Where its 64 signature bytes start in the instruction data.
        offset: usize,
    },
}

/// A `Submit` instruction read back from its data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub signers: Vec<SubmitSigner>,
    /// What the authorities sign.
    pub message: v1::Message,
    /// The exact bytes they sign.
    pub signing_bytes: Vec<u8>,
}

impl Submission {
    /// Read the data of an instruction to the signer program. `None` when it
    /// is not a well-formed `Submit`.
    pub fn parse(data: &[u8]) -> Option<Self> {
        let (&tag, rest) = data.split_first()?;
        let (&count, _) = rest.split_first()?;
        if tag != SUBMIT_TAG {
            return None;
        }
        let mut at = 2;
        let mut places = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            match *data.get(at)? {
                0 => {
                    places.push(None);
                    at += 1;
                }
                1 => {
                    data.get(at + 1..at + 1 + SIGNATURE_BYTES)?;
                    places.push(Some(at + 1));
                    at += 1 + SIGNATURE_BYTES;
                }
                _ => return None,
            }
        }
        let signing_bytes = data.get(at..)?.to_vec();
        let message = parse_message(&signing_bytes)?;
        if usize::from(message.header.num_required_signatures) != places.len() {
            return None;
        }
        let signers = message
            .account_keys
            .iter()
            .zip(places)
            .map(|(pubkey, place)| SubmitSigner {
                pubkey: *pubkey,
                role: match place {
                    None => SubmitRole::Forwarded,
                    Some(offset) => SubmitRole::Authority {
                        signed: Signature::try_from(&data[offset..offset + SIGNATURE_BYTES])
                            .is_ok_and(|signature| {
                                signature.verify(pubkey.as_ref(), &signing_bytes)
                            }),
                        offset,
                    },
                },
            })
            .collect();
        Some(Self {
            signers,
            message,
            signing_bytes,
        })
    }

    /// The authorities, each with whether its signature is in place.
    pub fn authorities(&self) -> impl Iterator<Item = (Pubkey, bool)> + '_ {
        self.signers.iter().filter_map(|signer| match signer.role {
            SubmitRole::Authority { signed, .. } => Some((signer.pubkey, signed)),
            SubmitRole::Forwarded => None,
        })
    }

    /// The executor call the authorities sign for.
    pub fn execute(&self) -> Option<Instruction> {
        message_instructions(&self.message).into_iter().next()
    }
}

/// An executor `Execute` read back: the nonce it consumes and what it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Execution {
    pub nonce_authority: Pubkey,
    pub nonce_account: Pubkey,
    /// The nonce value the message was signed against.
    pub nonce_value: Hash,
    pub message: v1::Message,
}

impl Execution {
    /// Read an instruction to the executor program. `None` when it is not a
    /// well-formed `Execute`.
    pub fn parse(instruction: &Instruction) -> Option<Self> {
        if instruction.program_id != MESSAGE_EXECUTOR_PROGRAM_ID {
            return None;
        }
        let (&tag, bytes) = instruction.data.split_first()?;
        if tag != EXECUTE_TAG {
            return None;
        }
        let message = parse_message(bytes)?;
        Some(Self {
            nonce_authority: instruction.accounts.first()?.pubkey,
            nonce_account: instruction.accounts.get(1)?.pubkey,
            nonce_value: message.lifetime_specifier,
            message,
        })
    }

    pub fn instructions(&self) -> Vec<Instruction> {
        message_instructions(&self.message)
    }
}

/// Write `signature` into the place `authority` has in a `Submit`'s data.
/// The signature is checked against the authorization message first.
pub fn fill_submit_signature(
    data: &mut [u8],
    authority: &Pubkey,
    signature: &Signature,
) -> Result<(), ProgrammaticError> {
    let submission = Submission::parse(data).ok_or(ProgrammaticError::NotASubmit)?;
    let offset = submission
        .signers
        .iter()
        .find_map(|signer| match signer.role {
            SubmitRole::Authority { offset, .. } if signer.pubkey == *authority => Some(offset),
            _ => None,
        })
        .ok_or(ProgrammaticError::NotAnAuthority(*authority))?;
    if !signature.verify(authority.as_ref(), &submission.signing_bytes) {
        return Err(ProgrammaticError::BadSignature(*authority));
    }
    data[offset..offset + SIGNATURE_BYTES].copy_from_slice(signature.as_ref());
    Ok(())
}

/// A v1 message from its prefixed bytes. Refuses bytes that do not serialize
/// back to themselves, so nothing can hide after the message.
pub(super) fn parse_message(bytes: &[u8]) -> Option<v1::Message> {
    let (&prefix, body) = bytes.split_first()?;
    if prefix != v1::V1_PREFIX {
        return None;
    }
    let message = v1::deserialize(body).ok()?;
    (message.serialize() == bytes).then_some(message)
}

/// Every instruction of `message` with its accounts resolved and flagged.
pub fn message_instructions(message: &v1::Message) -> Vec<Instruction> {
    message
        .instructions
        .iter()
        .filter_map(|compiled| {
            let program_id = *message
                .account_keys
                .get(usize::from(compiled.program_id_index))?;
            let accounts = compiled
                .accounts
                .iter()
                .map(|index| {
                    let index = usize::from(*index);
                    Some(AccountMeta {
                        pubkey: *message.account_keys.get(index)?,
                        is_signer: message.is_signer(index),
                        is_writable: is_writable(message, index),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Instruction {
                program_id,
                accounts,
                data: compiled.data.clone(),
            })
        })
        .collect()
}

/// The executor's `Execute`: nonce authority, nonce account, nonce program,
/// then every account of the execution message in order.
pub(super) fn execute_instruction(nonce: &ExecutorNonce, execution: &v1::Message) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new_readonly(nonce.authority, true),
        AccountMeta::new(nonce.account, false),
        AccountMeta::new_readonly(EXECUTOR_NONCE_PROGRAM_ID, false),
    ];
    for (index, key) in execution.account_keys.iter().enumerate() {
        accounts.push(AccountMeta {
            pubkey: *key,
            is_signer: execution.is_signer(index),
            is_writable: is_writable(execution, index),
        });
    }
    let mut data = vec![EXECUTE_TAG];
    data.extend_from_slice(&execution.serialize());
    Instruction {
        program_id: MESSAGE_EXECUTOR_PROGRAM_ID,
        accounts,
        data,
    }
}
