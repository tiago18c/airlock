//! What moves back from a device that signed: one signature per signer.

use crate::transport::protocol::Signed;
use crate::tx::decode::DecodedTx;
use solana_pubkey::Pubkey;
use solana_signature::SIGNATURE_BYTES;

/// A filled, valid signature slot, as the signature object Assemble accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureObject {
    pub pubkey: Pubkey,
    pub signed: Signed,
}

/// One object per valid signature in `tx`. Each carries its pubkey, so the
/// receiver knows which slot to fill.
pub fn signature_objects(tx: &DecodedTx) -> Vec<SignatureObject> {
    tx.signers
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.valid)
        .filter_map(|(index, slot)| {
            let start = tx.message.len() + index * SIGNATURE_BYTES;
            let signature: [u8; SIGNATURE_BYTES] = tx
                .wire
                .get(start..start + SIGNATURE_BYTES)?
                .try_into()
                .ok()?;
            Some(SignatureObject {
                pubkey: slot.pubkey,
                signed: Signed::Transaction {
                    pubkey: slot.pubkey.to_bytes(),
                    signature,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{compile, transfer};
    use crate::keys::signer::{KeypairSigner, WalletSigner};
    use crate::transport::protocol::{decode_object, encode_object, QrObject};
    use crate::tx::decode::decode_v1_transaction;
    use crate::tx::signatures::apply_local_signatures;
    use crate::wallets::relay::{accept_signature, SignatureUse};

    fn unsigned(payer: Pubkey, other: Pubkey) -> Vec<u8> {
        // Two required signers: the payer, and the source of the transfer.
        compile(payer, vec![transfer(other, payer, 5)]).wire
    }

    #[test]
    fn a_signature_travels_as_an_object_and_fills_its_slot() {
        let payer = KeypairSigner::new_random();
        let other = KeypairSigner::new_random();
        let wire = unsigned(payer.pubkey(), other.pubkey());
        assert!(signature_objects(&decode_v1_transaction(&wire).unwrap()).is_empty());

        // The signing device fills only its own slot and exports it.
        let signed = apply_local_signatures(&wire, &[&other as &dyn WalletSigner]).unwrap();
        let objects = signature_objects(&decode_v1_transaction(&signed).unwrap());
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].pubkey, other.pubkey());

        // The assembling device still holds the unsigned wire.
        let object = QrObject::Signature(objects[0].signed.clone());
        let QrObject::Signature(signed) = decode_object(&encode_object(&object).unwrap()).unwrap()
        else {
            panic!("a signature object");
        };
        assert!(matches!(signed, Signed::Transaction { .. }));
        let (filled, used) = accept_signature(&wire, &signed).unwrap();
        assert_eq!(used, SignatureUse::Transaction);
        let decoded = decode_v1_transaction(&filled).unwrap();
        assert_eq!(decoded.signers.iter().filter(|slot| slot.valid).count(), 1);

        // A signature for another message is refused.
        let different = unsigned(other.pubkey(), payer.pubkey());
        assert!(accept_signature(&different, &signed).is_err());
    }
}
