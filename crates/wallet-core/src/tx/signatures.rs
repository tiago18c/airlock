//! Writing signatures into a transaction's slots.

use crate::error::SignError;
use crate::keys::signer::WalletSigner;
use crate::tx::decode::parse_wire;
use solana_pubkey::Pubkey;
use solana_signature::{Signature, SIGNATURE_BYTES};

/// Fill empty (or invalid) signature slots with matching local signers.
/// Already-valid signatures are kept. Partial signing is allowed. Works on
/// legacy, v0 and v1 wires.
pub fn apply_local_signatures(
    wire: &[u8],
    signers: &[&dyn WalletSigner],
) -> Result<Vec<u8>, SignError> {
    let view = parse_wire(wire).map_err(|e| SignError::Message(e.to_string()))?;
    let mut out = view.wire.clone();
    let mut wrote = 0usize;
    let mut missing = 0usize;

    for (i, offset) in view.signature_offsets.iter().enumerate() {
        if view.slot_valid(i) {
            continue;
        }
        let pubkey = view.static_keys[i];
        missing += 1;
        let Some(signer) = signers.iter().find(|s| s.pubkey() == pubkey) else {
            continue;
        };
        let signature = signer.try_sign_message(&view.message)?;
        out[*offset..*offset + SIGNATURE_BYTES].copy_from_slice(signature.as_ref());
        wrote += 1;
    }

    if wrote == 0 && missing > 0 {
        return Err(SignError::NoLocalSigner);
    }
    Ok(out)
}

/// Write one verified signature into the slot for `pubkey`.
pub fn insert_signature(
    wire: &[u8],
    pubkey: &Pubkey,
    signature: &[u8; SIGNATURE_BYTES],
) -> Result<Vec<u8>, SignError> {
    let view = parse_wire(wire).map_err(|e| SignError::Message(e.to_string()))?;
    let Some(index) = view.static_keys.iter().position(|key| key == pubkey) else {
        return Err(SignError::Message(
            "this pubkey is not a required signer".into(),
        ));
    };
    if index >= view.num_required_signatures {
        return Err(SignError::Message(
            "this pubkey is not a required signer".into(),
        ));
    }
    let sig = Signature::from(*signature);
    if !sig.verify(pubkey.as_ref(), &view.message) {
        return Err(SignError::Message(
            "signature does not match this transaction".into(),
        ));
    }
    let mut out = view.wire.clone();
    let start = view.signature_offsets[index];
    out[start..start + SIGNATURE_BYTES].copy_from_slice(signature);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::transfer;
    use crate::keys::signer::KeypairSigner;
    use crate::tx::decode::{decode_transaction, LookupTables};
    use solana_hash::Hash;
    use solana_instruction::Instruction;
    use solana_message::v0::Message as V0Message;
    use solana_message::{Message as LegacyMessage, VersionedMessage};
    use solana_transaction::versioned::VersionedTransaction;

    fn legacy_wire(instructions: &[Instruction], payer: &Pubkey) -> Vec<u8> {
        let mut message = LegacyMessage::new(instructions, Some(payer));
        message.recent_blockhash = Hash::new_from_array([1; 32]);
        let nsig = usize::from(message.header.num_required_signatures);
        wincode::serialize(&VersionedTransaction {
            signatures: vec![Signature::default(); nsig],
            message: VersionedMessage::Legacy(message),
        })
        .unwrap()
    }

    fn v0_transfer_wire(payer: &Pubkey, to: &Pubkey) -> Vec<u8> {
        let header = solana_message::MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 1,
        };
        let data = {
            let mut data = 2u32.to_le_bytes().to_vec();
            data.extend_from_slice(&5u64.to_le_bytes());
            data
        };
        let message = V0Message {
            header,
            account_keys: vec![*payer, crate::ids::SYSTEM_PROGRAM_ID, *to],
            recent_blockhash: Hash::new_from_array([1; 32]),
            instructions: vec![solana_message::compiled_instruction::CompiledInstruction {
                program_id_index: 1,
                accounts: vec![0, 2],
                data,
            }],
            address_table_lookups: vec![],
        };
        wincode::serialize(&VersionedTransaction {
            signatures: vec![Signature::default()],
            message: VersionedMessage::V0(message),
        })
        .unwrap()
    }

    #[test]
    fn signs_and_inserts_into_a_legacy_slot() {
        let from = KeypairSigner::new_random();
        let wire = legacy_wire(
            &[transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5)],
            &from.pubkey(),
        );
        let signed = apply_local_signatures(&wire, &[&from as &dyn WalletSigner]).unwrap();
        let tx = decode_transaction(&signed, &LookupTables::new()).unwrap();
        assert!(tx.signers[0].valid);

        let again = apply_local_signatures(&signed, &[&from as &dyn WalletSigner]).unwrap();
        assert_eq!(again, signed);

        let view = parse_wire(&wire).unwrap();
        let signature = from.try_sign_message(&view.message).unwrap();
        let mut bytes = [0u8; SIGNATURE_BYTES];
        bytes.copy_from_slice(signature.as_ref());
        let inserted = insert_signature(&wire, &from.pubkey(), &bytes).unwrap();
        assert!(
            decode_transaction(&inserted, &LookupTables::new())
                .unwrap()
                .signers[0]
                .valid
        );
    }

    #[test]
    fn signs_and_keeps_signatures_on_a_v0_slot() {
        let from = KeypairSigner::new_random();
        let to = Pubkey::new_from_array([2; 32]);
        let wire = v0_transfer_wire(&from.pubkey(), &to);
        assert_eq!(
            parse_wire(&wire).unwrap().format,
            crate::tx::decode::TxFormat::V0
        );

        let signed = apply_local_signatures(&wire, &[&from as &dyn WalletSigner]).unwrap();
        let tx = decode_transaction(&signed, &LookupTables::new()).unwrap();
        assert!(tx.signers[0].valid);
        assert_eq!(
            parse_wire(&signed).unwrap().format,
            crate::tx::decode::TxFormat::V0
        );

        let kept = apply_local_signatures(&signed, &[&from as &dyn WalletSigner]).unwrap();
        assert_eq!(kept, signed);

        let view = parse_wire(&wire).unwrap();
        let signature = from.try_sign_message(&view.message).unwrap();
        let mut bytes = [0u8; SIGNATURE_BYTES];
        bytes.copy_from_slice(signature.as_ref());
        let inserted = insert_signature(&wire, &from.pubkey(), &bytes).unwrap();
        assert!(
            decode_transaction(&inserted, &LookupTables::new())
                .unwrap()
                .signers[0]
                .valid
        );
    }
}
