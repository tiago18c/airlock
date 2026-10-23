//! The context note that travels with a transaction: which IDL each program
//! was read with, so another device can tell whether it reads the same thing,
//! and what each Vector account's next signature has to be made over.

use crate::idl::catalog::{sha256_bytes, IdlCatalog};
use crate::review::explain::{explain_with_note, DisplayNote};
use crate::transport::protocol::{ContextAuthorization, ContextPin, ProtocolError, TxContext};
use crate::tx::decode::DecodedTx;
use crate::wallets::relay::{authorizations_for_note, VectorContext};
use solana_pubkey::Pubkey;
use std::collections::HashMap;

/// The context note for `tx`: one checksum per program this device has an IDL
/// for, and no Vector authorizations.
pub fn context_note(store: &dyn IdlCatalog, tx: &DecodedTx) -> Result<TxContext, ProtocolError> {
    context_note_for(store, tx, &[])
}

/// [`context_note`], also carrying what `known` says of each Vector account
/// so another device can place and sign their authorizations.
pub fn context_note_for(
    store: &dyn IdlCatalog,
    tx: &DecodedTx,
    known: &[VectorContext],
) -> Result<TxContext, ProtocolError> {
    context_note_with(store, tx, &authorizations_for_note(known))
}

/// [`context_note`], with authorization entries already built.
pub fn context_note_with(
    store: &dyn IdlCatalog,
    tx: &DecodedTx,
    authorizations: &[ContextAuthorization],
) -> Result<TxContext, ProtocolError> {
    let note = DisplayNote::default().with_vector_schemes(
        &authorizations
            .iter()
            .filter_map(VectorContext::from_note)
            .collect::<Vec<_>>(),
    );
    let pins: Vec<ContextPin> = explain_with_note(store, tx, &note)
        .programs
        .iter()
        .filter_map(|line| {
            line.sha256.map(|idl_sha256| ContextPin {
                program_id: line.program_id.to_bytes(),
                idl_sha256,
            })
        })
        .collect();
    TxContext::new(sha256_bytes(&tx.message), &pins, authorizations)
}

/// A note is written for one message. It says nothing about any other.
pub fn context_is_for(context: &TxContext, tx: &DecodedTx) -> bool {
    context.message_sha256 == sha256_bytes(&tx.message)
}

/// What a stored note contributes to the display of `tx`. A note for a
/// different transaction contributes nothing.
pub fn display_note(tx: &DecodedTx, context: Option<&TxContext>) -> DisplayNote {
    match context {
        Some(context) if context_is_for(context, tx) => DisplayNote {
            pins: context
                .pins
                .iter()
                .map(|pin| (Pubkey::new_from_array(pin.program_id), pin.idl_sha256))
                .collect(),
            accounts: context
                .accounts
                .iter()
                .map(|account| (Pubkey::new_from_array(account.pubkey), account.data.clone()))
                .collect(),
            schemes: context
                .authorizations
                .iter()
                .filter_map(|authorization| {
                    VectorContext::from_note(authorization)
                        .map(|known| (known.account, known.scheme))
                })
                .collect(),
            // A note carries no keys.
            keys: HashMap::new(),
        },
        _ => DisplayNote::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{compile, transfer};
    use crate::idl::catalog::{IdlStore, MemoryBodies};
    use crate::transport::protocol::ContextAccount;
    use crate::tx::decode::decode_v1_transaction;
    use wallet_vault::store::MemoryStore;

    fn unsigned(payer: Pubkey, other: Pubkey) -> Vec<u8> {
        // Two required signers: the payer, and the source of the transfer.
        compile(payer, vec![transfer(other, payer, 5)]).wire
    }

    #[test]
    fn a_note_only_shapes_the_transaction_it_was_written_for() {
        let payer = Pubkey::new_from_array([1; 32]);
        let other = Pubkey::new_from_array([2; 32]);
        let tx = decode_v1_transaction(&unsigned(payer, other)).unwrap();
        let store = IdlStore::open(MemoryStore::new(), MemoryBodies::new());

        let mut context = context_note(&store, &tx).unwrap();
        assert!(context_is_for(&context, &tx));
        assert!(context.pins.is_empty());
        assert!(context.authorizations.is_empty());

        context.accounts.push(ContextAccount {
            pubkey: [9; 32],
            data: vec![1, 2, 3],
        });
        assert_eq!(display_note(&tx, Some(&context)).accounts.len(), 1);
        assert!(display_note(&tx, None).accounts.is_empty());

        let elsewhere = decode_v1_transaction(&unsigned(other, payer)).unwrap();
        assert!(!context_is_for(&context, &elsewhere));
        assert!(display_note(&elsewhere, Some(&context)).accounts.is_empty());
    }

    #[test]
    fn a_note_carries_vector_authorizations_without_their_keys() {
        use crate::wallets::relay::VectorContext;
        use crate::wallets::vector::SchemeId;

        let payer = Pubkey::new_from_array([1; 32]);
        let other = Pubkey::new_from_array([2; 32]);
        let tx = decode_v1_transaction(&unsigned(payer, other)).unwrap();
        let store = IdlStore::open(MemoryStore::new(), MemoryBodies::new());
        let known = [VectorContext {
            account: Pubkey::new_from_array([3; 32]),
            scheme: SchemeId::Ed25519,
            nonce: Some([4; 32]),
            key: Some(vec![9; 32]),
            leaf_floor: 7,
        }];

        let context = context_note_for(&store, &tx, &known).unwrap();
        assert_eq!(context.authorizations.len(), 1);
        assert_eq!(context.authorizations[0].scheme, SchemeId::Ed25519.byte());
        assert_eq!(context.authorizations[0].nonce, [4; 32]);
        assert_eq!(context.authorizations[0].leaf_floor, 7);
        let back = VectorContext::from_note(&context.authorizations[0]).unwrap();
        assert_eq!(back.key, None);
        assert_eq!(back.nonce, Some([4; 32]));
    }
}
