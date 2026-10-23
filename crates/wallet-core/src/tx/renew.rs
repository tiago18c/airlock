//! Moving a transaction whose blockhash expired onto a fresh one.

use crate::error::SignError;
use solana_hash::Hash;
use solana_message::compiled_instruction::CompiledInstruction;
use solana_message::VersionedMessage;
use solana_pubkey::Pubkey;
use solana_signature::{Signature, SIGNATURE_BYTES};
use solana_system_interface::instruction::advance_nonce_account;
use solana_transaction::versioned::VersionedTransaction;

use crate::tx::builder::{compile_message, NonceSpec, BUILTIN_CU_PER_INSTRUCTION, MAX_CU_LIMIT};
use crate::tx::decode::{DecodedTx, TxFormat};

/// The same transaction on `blockhash`, with each `(old, new)` signer of
/// `swaps` put in place of the old one. Every signature slot is emptied,
/// since none of them covers the new message. An authorization carried in an
/// instruction's data is left as it is. Works on legacy, v0 and v1 wires.
///
/// A swapped signer has to be one of the transaction's signers. Where an
/// instruction's data names it, the new key is written there too.
pub fn renew(
    wire: &[u8],
    blockhash: Hash,
    swaps: &[(Pubkey, Pubkey)],
) -> Result<Vec<u8>, SignError> {
    let tx: VersionedTransaction = wincode::deserialize(wire)
        .map_err(|_| SignError::Message("the transaction could not be read".into()))?;
    let mut message = tx.message;
    let signers = usize::from(message.header().num_required_signatures);
    if is_nonce_advance(&message) {
        return Err(SignError::Message(
            "this transaction lives on a durable nonce, not on a blockhash".into(),
        ));
    }
    {
        let (keys, instructions) = parts(&mut message);
        for (old, new) in swaps {
            let Some(index) = keys.iter().position(|key| key == old) else {
                return Err(SignError::Message(format!(
                    "{old} is not in this transaction"
                )));
            };
            if index >= signers {
                return Err(SignError::Message(format!(
                    "{old} does not sign this transaction"
                )));
            }
            if keys.contains(new) {
                return Err(SignError::Message(format!(
                    "{new} is already in this transaction"
                )));
            }
            keys[index] = *new;
        }
        for ix in instructions {
            ix.data = swap_in_data(&ix.data, swaps);
        }
    }
    match &mut message {
        VersionedMessage::Legacy(message) => message.recent_blockhash = blockhash,
        VersionedMessage::V0(message) => message.recent_blockhash = blockhash,
        VersionedMessage::V1(message) => message.lifetime_specifier = blockhash,
    }
    match message {
        // A v1 wire is the message, then its signatures.
        VersionedMessage::V1(_) => {
            let mut out = message.serialize();
            out.extend(std::iter::repeat_n(
                0u8,
                signers.saturating_mul(SIGNATURE_BYTES),
            ));
            Ok(out)
        }
        message => wincode::serialize(&VersionedTransaction {
            signatures: vec![Signature::default(); signers],
            message,
        })
        .map_err(|_| SignError::Message("the transaction could not be written".into())),
    }
}

/// The v1 transaction `tx` moved onto the durable nonce `nonce`: its
/// advance goes first, and the transaction lives on the nonce's value
/// instead of a blockhash, so it waits however long the signing takes. The
/// limits and fees stay, with room for the advance.
///
/// Refused when `tx` already lives on a durable nonce, when it carries a
/// signature (which would not cover the new message), and when the nonce's
/// authority does not sign it. Deprecated once the programmatic signer or
/// Vector is on mainnet.
pub fn onto_nonce(tx: &DecodedTx, nonce: &NonceSpec) -> Result<Vec<u8>, SignError> {
    if tx.format != TxFormat::V1 {
        return Err(SignError::Message(
            "only a v1 transaction can be moved onto a durable nonce".into(),
        ));
    }
    if tx.uses_durable_nonce {
        return Err(SignError::Message(
            "this transaction already lives on a durable nonce".into(),
        ));
    }
    if tx.signers.iter().any(|slot| slot.filled) {
        return Err(SignError::Message(
            "this transaction is already signed, and moving it onto a durable nonce would void the signatures".into(),
        ));
    }
    if !tx.signers.iter().any(|slot| slot.pubkey == nonce.authority) {
        return Err(SignError::Message(format!(
            "the durable nonce {} is controlled by {}, which does not sign this transaction",
            nonce.account, nonce.authority
        )));
    }
    let fee_payer = tx
        .fee_payer()
        .ok_or_else(|| SignError::Message("this transaction names no fee payer".into()))?;
    let mut instructions = vec![advance_nonce_account(&nonce.account, &nonce.authority)];
    instructions.extend(tx.native_instructions());
    let mut budget = tx.budget;
    if let Some(limit) = budget.compute_unit_limit {
        budget.compute_unit_limit = Some(
            limit
                .saturating_add(BUILTIN_CU_PER_INSTRUCTION)
                .min(MAX_CU_LIMIT),
        );
    }
    compile_message(&fee_payer, &instructions, nonce.hash, budget.config())
        .map(|compiled| compiled.wire)
        .map_err(|err| SignError::Message(err.to_string()))
}

fn parts(message: &mut VersionedMessage) -> (&mut Vec<Pubkey>, &mut Vec<CompiledInstruction>) {
    match message {
        VersionedMessage::Legacy(message) => (&mut message.account_keys, &mut message.instructions),
        VersionedMessage::V0(message) => (&mut message.account_keys, &mut message.instructions),
        VersionedMessage::V1(message) => (&mut message.account_keys, &mut message.instructions),
    }
}

/// `data` with each `(old, new)` of `swaps` written in place of every whole
/// 32 bytes that name `old`.
pub(crate) fn swap_in_data(data: &[u8], swaps: &[(Pubkey, Pubkey)]) -> Vec<u8> {
    let mut out = data.to_vec();
    let mut at = 0;
    while at + 32 <= out.len() {
        match swaps
            .iter()
            .find(|(old, _)| out[at..at + 32] == old.to_bytes())
        {
            Some((_, new)) => {
                out[at..at + 32].copy_from_slice(new.as_ref());
                at += 32;
            }
            None => at += 1,
        }
    }
    out
}

/// Whether the first instruction advances a durable nonce.
fn is_nonce_advance(message: &VersionedMessage) -> bool {
    let keys = message.static_account_keys();
    message.instructions().first().is_some_and(|ix| {
        keys.get(usize::from(ix.program_id_index)) == Some(&crate::ids::SYSTEM_PROGRAM_ID)
            && ix.data.get(..4) == Some(&4u32.to_le_bytes())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::transfer;
    use crate::keys::signer::{KeypairSigner, WalletSigner};
    use crate::tx::decode::{decode_transaction, parse_wire, LookupTables, TxFormat};
    use crate::tx::signatures::apply_local_signatures;
    use solana_message::Message as LegacyMessage;

    fn legacy(payer: &KeypairSigner, other: &KeypairSigner) -> Vec<u8> {
        let mut ix = transfer(payer.pubkey(), Pubkey::new_from_array([9; 32]), 5);
        ix.accounts
            .push(solana_instruction::AccountMeta::new(other.pubkey(), true));
        let mut message = LegacyMessage::new(&[ix], Some(&payer.pubkey()));
        message.recent_blockhash = Hash::new_from_array([1; 32]);
        let wire = wincode::serialize(&VersionedTransaction {
            signatures: vec![Signature::default(); 2],
            message: VersionedMessage::Legacy(message),
        })
        .unwrap();
        apply_local_signatures(&wire, &[payer as &dyn WalletSigner, other]).unwrap()
    }

    #[test]
    fn moves_to_the_new_blockhash_and_clears_signatures() {
        let payer = KeypairSigner::new_random();
        let other = KeypairSigner::new_random();
        let wire = legacy(&payer, &other);
        let hash = Hash::new_from_array([7; 32]);
        let renewed = renew(&wire, hash, &[]).unwrap();
        let tx = decode_transaction(&renewed, &LookupTables::new()).unwrap();
        assert_eq!(tx.format, TxFormat::Legacy);
        assert_eq!(tx.lifetime, hash);
        assert!(tx.signers.iter().all(|slot| !slot.filled));
    }

    #[test]
    fn swaps_a_signer_for_a_new_key() {
        let payer = KeypairSigner::new_random();
        let other = KeypairSigner::new_random();
        let fresh = KeypairSigner::new_random();
        let wire = legacy(&payer, &other);
        let renewed = renew(
            &wire,
            Hash::new_from_array([7; 32]),
            &[(other.pubkey(), fresh.pubkey())],
        )
        .unwrap();
        let view = parse_wire(&renewed).unwrap();
        assert!(view.static_keys.contains(&fresh.pubkey()));
        assert!(!view.static_keys.contains(&other.pubkey()));
        let signed =
            apply_local_signatures(&renewed, &[&payer as &dyn WalletSigner, &fresh]).unwrap();
        let tx = decode_transaction(&signed, &LookupTables::new()).unwrap();
        assert!(tx.fully_signed());
    }

    #[test]
    fn refuses_to_swap_an_account_that_does_not_sign() {
        let payer = KeypairSigner::new_random();
        let other = KeypairSigner::new_random();
        let wire = legacy(&payer, &other);
        let to = Pubkey::new_from_array([9; 32]);
        assert!(renew(&wire, Hash::default(), &[(to, Pubkey::new_unique())]).is_err());
    }

    fn nonce_for(authority: Pubkey) -> NonceSpec {
        NonceSpec {
            account: Pubkey::new_from_array([4; 32]),
            authority,
            hash: Hash::new_from_array([8; 32]),
        }
    }

    fn v1_transfer(payer: Pubkey, from: Pubkey, cu_limit: Option<u32>) -> Vec<u8> {
        crate::tx::builder::compile_v1_budget(
            payer,
            vec![transfer(from, Pubkey::new_from_array([9; 32]), 5)],
            None,
            Some(Hash::new_from_array([1; 32])),
            crate::tx::builder::ComputeBudgetSpec {
                cu_limit,
                cu_price_micro_lamports: Some(1_000),
                loaded_accounts_data_size: None,
            },
        )
        .unwrap()
        .wire
    }

    #[test]
    fn moves_a_v1_transaction_onto_a_durable_nonce_with_room_for_its_advance() {
        let payer = Pubkey::new_from_array([2; 32]);
        let cold = Pubkey::new_from_array([3; 32]);
        let wire = v1_transfer(payer, cold, Some(10_000));
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        let moved = onto_nonce(&tx, &nonce_for(cold)).unwrap();
        let after = decode_transaction(&moved, &LookupTables::new()).unwrap();
        assert!(after.uses_durable_nonce);
        assert_eq!(after.lifetime, Hash::new_from_array([8; 32]));
        assert_eq!(after.fee_payer(), Some(payer));
        assert_eq!(after.instructions.len(), 2);
        assert_eq!(after.instructions[1].data, tx.instructions[0].data);
        assert_eq!(
            after.budget.compute_unit_limit,
            Some(10_000 + crate::tx::builder::BUILTIN_CU_PER_INSTRUCTION)
        );
        assert_eq!(after.budget.priority_fee, tx.budget.priority_fee);
        assert!(after.signers.iter().all(|slot| !slot.filled));
    }

    #[test]
    fn will_not_move_a_signed_or_nonce_or_foreign_transaction_onto_a_nonce() {
        let payer = KeypairSigner::new_random();
        let cold = Pubkey::new_from_array([3; 32]);
        let wire = v1_transfer(payer.pubkey(), cold, None);
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        // Its authority does not sign.
        assert!(onto_nonce(&tx, &nonce_for(Pubkey::new_from_array([6; 32]))).is_err());
        // Already on a nonce.
        let moved = onto_nonce(&tx, &nonce_for(cold)).unwrap();
        let on_nonce = decode_transaction(&moved, &LookupTables::new()).unwrap();
        assert!(onto_nonce(&on_nonce, &nonce_for(cold)).is_err());
        // Signed by the payer already.
        let signed = apply_local_signatures(&wire, &[&payer as &dyn WalletSigner]).unwrap();
        let signed = decode_transaction(&signed, &LookupTables::new()).unwrap();
        assert!(onto_nonce(&signed, &nonce_for(cold)).is_err());
        // Not a v1 wire.
        let legacy = decode_transaction(
            &legacy(&payer, &KeypairSigner::new_random()),
            &LookupTables::new(),
        )
        .unwrap();
        assert!(onto_nonce(&legacy, &nonce_for(payer.pubkey())).is_err());
    }

    #[test]
    fn renews_a_v1_wire_and_swaps_its_one_time_key() {
        let payer = KeypairSigner::new_random();
        let other = KeypairSigner::new_random();
        let fresh = KeypairSigner::new_random();
        let mut ix = transfer(payer.pubkey(), Pubkey::new_from_array([9; 32]), 5);
        ix.accounts
            .push(solana_instruction::AccountMeta::new(other.pubkey(), true));
        let wire = crate::tx::builder::compile_v1(
            payer.pubkey(),
            vec![ix],
            None,
            Some(Hash::new_from_array([1; 32])),
        )
        .unwrap()
        .wire;
        let signed = apply_local_signatures(&wire, &[&payer as &dyn WalletSigner, &other]).unwrap();
        let hash = Hash::new_from_array([7; 32]);
        let renewed = renew(&signed, hash, &[(other.pubkey(), fresh.pubkey())]).unwrap();
        let tx = decode_transaction(&renewed, &LookupTables::new()).unwrap();
        assert_eq!(tx.format, TxFormat::V1);
        assert_eq!(tx.lifetime, hash);
        assert!(tx.signers.iter().all(|slot| !slot.filled));
        let resigned =
            apply_local_signatures(&renewed, &[&payer as &dyn WalletSigner, &fresh]).unwrap();
        assert!(decode_transaction(&resigned, &LookupTables::new())
            .unwrap()
            .fully_signed());
    }
}
