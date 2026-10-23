//! The message an authority signs, and the instruction that carries it.

use super::*;

/// Instructions wrapped so that programmatic signers can sign for them.
///
/// Built once, by whoever proposes the action. Every authority signs
/// [`Authorization::signing_bytes`]. Whoever holds the signatures calls
/// [`Authorization::submit_instruction`] and puts the result in an ordinary
/// transaction paid for by any wallet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authorization {
    /// What the executor runs. Its lifetime field holds the nonce value.
    pub execution: v1::Message,
    /// What the authorities sign. Its only instruction is the executor call.
    pub message: v1::Message,
    pub nonce: ExecutorNonce,
    /// Keys that sign the authorization message. Their derived addresses become signers.
    pub authorities: Vec<Pubkey>,
    /// Keys the instructions use directly as signers. They sign the relay transaction.
    pub forwarded: Vec<Pubkey>,
}

impl Authorization {
    /// Wrap `instructions` for `authorities`.
    ///
    /// `payer` is listed first in the execution message. Nothing is charged to
    /// it. The message format requires a first writable signer, so pass a
    /// programmatic signer address or a forwarded signer.
    pub fn new(
        payer: Pubkey,
        instructions: &[Instruction],
        authorities: &[Pubkey],
        nonce: ExecutorNonce,
    ) -> Result<Self, ProgrammaticError> {
        if instructions.is_empty() {
            return Err(ProgrammaticError::NoInstructions);
        }
        let authorities: Vec<Pubkey> = authorities
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if authorities.is_empty() {
            return Err(ProgrammaticError::NoAuthorities);
        }

        let execution = v1::Message::try_compile_with_config(
            &payer,
            instructions,
            nonce.value,
            TransactionConfig::empty(),
        )
        .map_err(|err| ProgrammaticError::Message(err.to_string()))?;
        check_limits(&execution)?;

        let execution_signers =
            &execution.account_keys[..usize::from(execution.header.num_required_signatures)];
        let derived: BTreeMap<Pubkey, Pubkey> = authorities
            .iter()
            .map(|authority| (programmatic_signer_address(authority), *authority))
            .collect();
        for (address, authority) in &derived {
            if *address != nonce.authority && !execution_signers.contains(address) {
                return Err(ProgrammaticError::UnusedAuthority {
                    authority: *authority,
                    derived: *address,
                });
            }
        }
        // A signer the executor uses that no authority derives must be a real
        // key. It signs the relay transaction, and Submit passes that on.
        let forwarded: Vec<Pubkey> = execution_signers
            .iter()
            .chain(std::iter::once(&nonce.authority))
            .filter(|address| !derived.contains_key(*address))
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let signers: Vec<Pubkey> = authorities
            .iter()
            .chain(&forwarded)
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if signers.len() > usize::from(MAX_SIGNATURES) {
            return Err(ProgrammaticError::TooManySigners {
                count: signers.len(),
            });
        }

        let execute = execute_instruction(&nonce, &execution);
        let message = authorization_message(&execute, &signers)?;
        check_limits(&message)?;

        Ok(Self {
            execution,
            message,
            nonce,
            authorities,
            forwarded,
        })
    }

    /// The bytes each authority signs.
    pub fn signing_bytes(&self) -> Vec<u8> {
        self.message.serialize()
    }

    /// The nonce value once this has executed.
    ///
    /// It depends only on the current value and on this exact execution
    /// message, so it is known before anything is submitted. An authorization
    /// built against it can only execute after this one.
    pub fn next_nonce(&self) -> ExecutorNonce {
        let commitment = sha256(&[&self.execution.serialize()]);
        let value = sha256(&[
            NONCE_STEP_TAG,
            EXECUTOR_NONCE_PROGRAM_ID.as_ref(),
            self.nonce.account.as_ref(),
            self.nonce.value.as_ref(),
            &commitment,
        ]);
        ExecutorNonce {
            value: Hash::new_from_array(value),
            ..self.nonce
        }
    }

    /// Sign as one authority.
    pub fn sign(
        &self,
        signer: &dyn crate::keys::signer::WalletSigner,
    ) -> Result<Signature, ProgrammaticError> {
        let authority = signer.pubkey();
        if !self.authorities.contains(&authority) {
            return Err(ProgrammaticError::NotAnAuthority(authority));
        }
        signer
            .try_sign_message(&self.signing_bytes())
            .map_err(|err| ProgrammaticError::Message(err.to_string()))
    }

    /// The `Submit` instruction for a relay transaction.
    ///
    /// Every authority needs a signature here. Every forwarded signer is marked
    /// as a signer of the instruction, so it has to sign the relay transaction.
    pub fn submit_instruction(
        &self,
        signatures: &BTreeMap<Pubkey, Signature>,
    ) -> Result<Instruction, ProgrammaticError> {
        self.submit(signatures, false)
    }

    /// Like [`Self::submit_instruction`], with room left for the signatures
    /// that have not arrived. An empty place is 64 zero bytes, which no key
    /// signs, so the instruction has its final size and fails on chain until
    /// [`fill_submit_signature`] has filled every one.
    pub fn submit_instruction_unsigned(
        &self,
        signatures: &BTreeMap<Pubkey, Signature>,
    ) -> Result<Instruction, ProgrammaticError> {
        self.submit(signatures, true)
    }

    pub(super) fn submit(
        &self,
        signatures: &BTreeMap<Pubkey, Signature>,
        leave_room: bool,
    ) -> Result<Instruction, ProgrammaticError> {
        let bytes = self.signing_bytes();
        let required = usize::from(self.message.header.num_required_signatures);

        let mut data = vec![SUBMIT_TAG, required as u8];
        for signer in &self.message.account_keys[..required] {
            if !self.authorities.contains(signer) {
                data.push(0);
                continue;
            }
            data.push(1);
            match signatures.get(signer) {
                Some(signature) => {
                    if !signature.verify(signer.as_ref(), &bytes) {
                        return Err(ProgrammaticError::BadSignature(*signer));
                    }
                    data.extend_from_slice(signature.as_ref());
                }
                None if leave_room => data.extend_from_slice(&[0u8; SIGNATURE_BYTES]),
                None => return Err(ProgrammaticError::MissingSignature(*signer)),
            }
        }
        data.extend_from_slice(&bytes);

        let accounts = self
            .message
            .account_keys
            .iter()
            .enumerate()
            .map(|(index, key)| AccountMeta {
                pubkey: *key,
                is_signer: self.forwarded.contains(key),
                is_writable: is_writable(&self.message, index),
            })
            .collect();
        Ok(Instruction {
            program_id: ED25519_SIGNER_PROGRAM_ID,
            accounts,
            data,
        })
    }
}

/// The envelope the authorities sign around `execute`.
///
/// Account order: signers that `execute` writes to, the other signers,
/// writable accounts, the executor program, read-only accounts. It carries no
/// blockhash and no fee settings because it is never a transaction itself.
pub(super) fn authorization_message(
    execute: &Instruction,
    signers: &[Pubkey],
) -> Result<v1::Message, ProgrammaticError> {
    let (writable_signers, readonly_signers): (Vec<Pubkey>, Vec<Pubkey>) =
        signers.iter().partition(|signer| {
            execute
                .accounts
                .iter()
                .any(|meta| meta.is_writable && meta.pubkey == **signer)
        });
    // The format needs one writable signer. When `execute` writes to none, the
    // first carries the flag. The flag alone grants nothing.
    let writable_signer_count = writable_signers.len().max(1);

    let mut writable = Vec::new();
    let mut readonly = Vec::new();
    for meta in &execute.accounts {
        if signers.contains(&meta.pubkey) || meta.pubkey == execute.program_id {
            continue;
        }
        let list = if meta.is_writable {
            &mut writable
        } else {
            &mut readonly
        };
        if !list.contains(&meta.pubkey) {
            list.push(meta.pubkey);
        }
    }
    readonly.retain(|key| !writable.contains(key));

    let account_keys: Vec<Pubkey> = writable_signers
        .into_iter()
        .chain(readonly_signers)
        .chain(writable)
        .chain(std::iter::once(execute.program_id))
        .chain(readonly.iter().copied())
        .collect();
    if account_keys.len() > usize::from(MAX_ADDRESSES) {
        return Err(ProgrammaticError::TooManyAccounts {
            count: account_keys.len(),
        });
    }
    let instructions = AccountKeys::new(&account_keys, None)
        .try_compile_instructions(std::slice::from_ref(execute))
        .map_err(|err| ProgrammaticError::Message(err.to_string()))?;

    let header = MessageHeader {
        num_required_signatures: signers.len() as u8,
        num_readonly_signed_accounts: signers.len().saturating_sub(writable_signer_count) as u8,
        num_readonly_unsigned_accounts: readonly.len().saturating_add(1) as u8,
    };
    Ok(v1::Message::new(
        header,
        TransactionConfig::empty(),
        Hash::default(),
        account_keys,
        instructions,
    ))
}

pub(super) fn check_limits(message: &v1::Message) -> Result<(), ProgrammaticError> {
    let count = message.account_keys.len();
    if count > usize::from(MAX_ADDRESSES) {
        return Err(ProgrammaticError::TooManyAccounts { count });
    }
    message
        .validate()
        .map_err(|err| ProgrammaticError::Message(err.to_string()))
}

pub(super) fn is_writable(message: &v1::Message, index: usize) -> bool {
    message.is_maybe_writable_with_reserved_addresses(index, None::<&std::collections::HashSet<_>>)
}

pub(super) fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}
