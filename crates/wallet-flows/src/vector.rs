//! Vector wallets: what signing for one needs from the chain, the
//! transaction that creates its account, and the one that moves it to a key
//! another device signed for.

use crate::error::{FlowError, Prepared};
use solana_pubkey::Pubkey;
use wallet_core::protocol::KeyRotation;
use wallet_core::relay::{insert_vector_signature, vector_accounts, AuthContext, VectorContext};
use wallet_core::vector::{
    address, initialize as build_initialize, rotation as build_rotation, SchemeId, VectorAccount,
    PROGRAM_ID,
};
use wallet_core::{compile_v1_budget, ComputeBudgetSpec, DecodedTx, RpcError, SolanaRpc};

/// What is known of each Vector account the `Advance` of `tx` lists: what
/// the chain says now, over what `known` says.
///
/// The nonce and the key that verifies come from the account. A key the
/// account does not keep (Falcon-512's) comes from `known`. An account the
/// node does not have, or that is not a complete Vector account, is as
/// `known` says, or left out.
pub async fn read_context(
    rpc: &dyn SolanaRpc,
    tx: &DecodedTx,
    known: &[VectorContext],
) -> Result<Vec<VectorContext>, RpcError> {
    let listed = vector_accounts(tx);
    let read = rpc.get_accounts(&listed).await?;
    Ok(listed
        .iter()
        .zip(read)
        .filter_map(|(address, raw)| {
            let before = known.iter().find(|context| context.account == *address);
            let Some(account) = raw.and_then(|raw| VectorAccount::parse(&PROGRAM_ID, &raw).ok())
            else {
                return before.cloned();
            };
            let key = account
                .current_key()
                .map(<[u8]>::to_vec)
                .or_else(|| before.and_then(|context| context.key.clone()));
            Some(VectorContext {
                account: *address,
                scheme: account.scheme,
                nonce: Some(account.nonce),
                key,
                leaf_floor: account.next_leaf().unwrap_or(0),
            })
        })
        .collect())
}

/// The Vector account at `address`, when it is a complete account of our
/// program. `None` when the node has no account there, or one that is not
/// ours or not yet fully expanded.
pub async fn read_account(
    rpc: &dyn SolanaRpc,
    address: &Pubkey,
) -> Result<Option<VectorAccount>, RpcError> {
    let Some(raw) = rpc.get_account(address).await? else {
        return Ok(None);
    };
    Ok(VectorAccount::parse(&PROGRAM_ID, &raw).ok())
}

/// [`read_account`] for each of `addresses`, in the order asked, in one
/// request per hundred.
pub async fn read_accounts(
    rpc: &dyn SolanaRpc,
    addresses: &[Pubkey],
) -> Result<Vec<Option<VectorAccount>>, RpcError> {
    Ok(rpc
        .get_accounts(addresses)
        .await?
        .into_iter()
        .map(|raw| raw.and_then(|raw| VectorAccount::parse(&PROGRAM_ID, &raw).ok()))
        .collect())
}

/// Rent for a complete account of `scheme`, as the node prices it.
pub async fn rent_for(rpc: &dyn SolanaRpc, scheme: SchemeId) -> Result<u64, RpcError> {
    rpc.get_rent_exemption(scheme.account_len()).await
}

/// The transaction that creates the Vector account of `key` under `scheme`,
/// paid by `payer`. An ML-DSA-44 account takes three `Initialize`s in the
/// same transaction: the first creates it, the next two grow it. The address is derived here so the caller can check it
/// matches the wallet.
pub async fn initialize(
    rpc: &dyn SolanaRpc,
    payer: Pubkey,
    scheme: SchemeId,
    key: &[u8],
) -> Result<Prepared, FlowError> {
    let account = address(&PROGRAM_ID, scheme, key).map_err(FlowError::refused)?;
    let instructions = build_initialize(&payer, scheme, key).map_err(FlowError::refused)?;
    // Initialize alone is cheap; ML-DSA growing twice needs about 1.1M CU.
    let budget = if scheme == SchemeId::MlDsa44 {
        ComputeBudgetSpec {
            cu_limit: Some(1_400_000),
            ..ComputeBudgetSpec::default()
        }
    } else {
        ComputeBudgetSpec::default()
    };
    let (blockhash, _) = rpc.get_latest_blockhash_with_height().await?;
    let compiled = compile_v1_budget(payer, instructions, None, Some(blockhash), budget)
        .map_err(FlowError::refused)?;
    let note = wallet_core::Phrase::default()
        .t("This creates the Vector account ")
        .a(account)
        .t(". The fee payer pays the rent; the account can already hold SOL and tokens, and initializing it keeps what it holds.");
    Ok(Prepared {
        wire: compiled.wire,
        note,
    })
}

/// The transaction that moves the Vector account made from `key` under
/// `scheme` to the key `rotation` names, paid by `payer`. The device that
/// holds the key signed it at each nonce the account may be at. The one the
/// account holds now is written in and checked against its key.
pub async fn rotate(
    rpc: &dyn SolanaRpc,
    payer: Pubkey,
    scheme: SchemeId,
    key: &[u8],
    rotation: &KeyRotation,
) -> Result<Prepared, FlowError> {
    let account = address(&PROGRAM_ID, scheme, key).map_err(FlowError::refused)?;
    if rotation.account != account.to_bytes() || rotation.scheme != scheme.byte() {
        return Err(FlowError::refused(
            "this rotation is for another account than this wallet's",
        ));
    }
    let held = read_account(rpc, &account)
        .await?
        .ok_or_else(|| FlowError::refused("this account is not initialized on this cluster"))?;
    let current = held
        .current_key()
        .map_or_else(|| key.to_vec(), <[u8]>::to_vec);
    if rotation.next_key == current {
        return Err(FlowError::refused(
            "the account already holds this key: the rotation has landed",
        ));
    }
    let signed = rotation
        .signed
        .iter()
        .find(|signed| signed.nonce == held.nonce)
        .ok_or_else(|| {
            FlowError::refused(
                "it was signed at nonces this account is not at. The account acted after it was made, or the device that made it missed a transaction. Make the rotation again there, after it signs the transaction this device sends next.",
            )
        })?;
    let instruction =
        build_rotation(scheme, key, &rotation.next_key).map_err(FlowError::refused)?;
    let budget = ComputeBudgetSpec {
        cu_limit: Some(scheme.advance_cu() + 20_000),
        ..ComputeBudgetSpec::default()
    };
    let (blockhash, _) = rpc.get_latest_blockhash_with_height().await?;
    let compiled = compile_v1_budget(payer, vec![instruction], None, Some(blockhash), budget)
        .map_err(FlowError::refused)?;
    let known = [VectorContext {
        account,
        scheme,
        nonce: Some(held.nonce),
        key: Some(current),
        leaf_floor: held.next_leaf().unwrap_or(0),
    }];
    let authorized = insert_vector_signature(
        &compiled.wire,
        &account,
        &signed.nonce,
        &signed.signature,
        &AuthContext { vector: &known },
    )
    .map_err(FlowError::refused)?;
    let note = wallet_core::Phrase::default()
        .t("This moves the Vector account ")
        .a(account)
        .t(" to its next key, as the device that holds the key signed it. The fee payer pays the fee, and nothing else changes.");
    Ok(Prepared {
        wire: authorized.wire,
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use wallet_core::{decode_v1_transaction, KeypairSigner, WalletSigner};

    #[test]
    fn initialize_is_paid_by_the_fee_payer_and_names_the_account() {
        let rpc = FakeRpc::default();
        let payer = pk(2);
        let key = KeypairSigner::new_random();
        let bytes = key.pubkey().to_bytes();
        let account = address(&PROGRAM_ID, SchemeId::Ed25519, &bytes).unwrap();
        let made = pollster::block_on(initialize(&rpc, payer, SchemeId::Ed25519, &bytes)).unwrap();
        let tx = decode_v1_transaction(&made.wire).unwrap();
        assert_eq!(tx.fee_payer(), Some(payer));
        assert!(tx.accounts.iter().any(|meta| meta.pubkey == account));
        assert!(made.note.0.contains(&wallet_core::Part::Address(account)));
    }

    #[test]
    fn a_missing_account_reads_as_none() {
        let found = pollster::block_on(read_account(&FakeRpc::default(), &pk(9))).unwrap();
        assert!(found.is_none());
    }
}
