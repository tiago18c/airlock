//! Compiling instructions into a transaction with what a node has to supply.

use crate::error::FlowError;
use crate::refresh::fetch_lifetime;
use crate::simulate::measure;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use wallet_core::{
    compile_draft, compile_v1, decode_v1_transaction, CompiledTx, Lifetime, NonceSpec, Phrase,
    SigningRoute, SolanaRpc, TxSettings,
};

/// Compile `instructions` on the node's latest blockhash, paid by `fee_payer`.
/// Returns the wire and the height at which that blockhash expires.
pub async fn compile_fresh(
    rpc: &dyn SolanaRpc,
    fee_payer: Pubkey,
    instructions: Vec<Instruction>,
) -> Result<(Vec<u8>, u64), FlowError> {
    let (blockhash, last_valid_height) = rpc.get_latest_blockhash_with_height().await?;
    compile_v1(fee_payer, instructions, None, Some(blockhash))
        .map(|compiled| (compiled.wire, last_valid_height))
        .map_err(FlowError::refused)
}

/// Compile `instructions` on the durable nonce `nonce`, read from the chain.
/// Refused when the nonce's authority is not one of `signers`, which are the
/// fee payer and the plain keys that sign.
pub async fn compile_on_nonce(
    rpc: &dyn SolanaRpc,
    fee_payer: Pubkey,
    instructions: Vec<Instruction>,
    nonce: Pubkey,
    signers: &[Pubkey],
) -> Result<Vec<u8>, FlowError> {
    let Lifetime::Nonce(found) = fetch_lifetime(rpc, Some(nonce)).await? else {
        return Err(FlowError::refused(format!(
            "Nonce account {nonce} could not be read."
        )));
    };
    if !signers.contains(&found.authority) {
        return Err(FlowError::refused(format!(
            "The durable nonce {nonce} is controlled by {}, which does not sign this transaction. Its authority has to be the fee payer or one of the wallets that sign.",
            found.authority
        )));
    }
    let spec = NonceSpec {
        account: found.pubkey,
        authority: found.authority,
        hash: found.hash,
    };
    compile_v1(fee_payer, instructions, Some(spec), None)
        .map(|compiled| compiled.wire)
        .map_err(FlowError::refused)
}

/// Compile `instructions` for the wallet of `route`, paid by `fee_payer`:
/// on the wallet's durable nonce when it has one, otherwise on the latest
/// blockhash. The height is the one at which a blockhash expires, `None` on
/// a nonce, which does not.
pub async fn compile_for_route(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
    fee_payer: Pubkey,
    instructions: Vec<Instruction>,
) -> Result<(Vec<u8>, Option<u64>), FlowError> {
    match route.durable_nonce {
        Some(nonce) => {
            let signers = [fee_payer, route.address];
            compile_on_nonce(rpc, fee_payer, instructions, nonce, &signers)
                .await
                .map(|wire| (wire, None))
        }
        None => compile_fresh(rpc, fee_payer, instructions)
            .await
            .map(|(wire, height)| (wire, Some(height))),
    }
}

/// `note` with what a durable nonce of `route` changes, when it has one.
pub fn with_nonce_note(note: Phrase, route: &SigningRoute) -> Phrase {
    match route.durable_nonce {
        Some(nonce) => note
            .t(" It lives on the durable nonce ")
            .a(nonce)
            .t(", so it stays valid until the offline key signs it."),
        None => note,
    }
}

/// What Compile was pressed on.
pub struct Draft {
    pub fee_payer: Option<Pubkey>,
    pub instructions: Vec<Instruction>,
    pub settings: TxSettings,
}

/// Compile `draft`, filling what was left blank from the node: limits from a
/// simulation of `probe`, then the latest blockhash. The blockhash is fetched
/// last, so none of its lifetime is spent waiting on the simulation. The
/// values are used for this compile only and are not written into the form.
/// Returns the transaction, what to say about the limits, and the height at
/// which a freshly fetched blockhash expires (when one was fetched).
pub async fn compile_with_defaults(
    rpc: &dyn SolanaRpc,
    draft: Draft,
    probe: &CompiledTx,
) -> Result<(CompiledTx, String, Option<u64>), FlowError> {
    let Draft {
        fee_payer,
        instructions,
        mut settings,
    } = draft;
    let mut note = String::new();
    if settings.wants_limits() {
        let typed = settings.clone();
        let measured = match decode_v1_transaction(&probe.wire) {
            Ok(tx) => measure(rpc, &tx).await.map_err(|err| err.to_string()),
            Err(err) => Err(err.to_string()),
        };
        note = match measured {
            Ok(simulation) => match &simulation.error {
                None => {
                    settings.fill_limits(
                        simulation.units_consumed,
                        simulation.loaded_accounts_data_size,
                    );
                    limits_note(&typed, &settings)
                }
                Some(failure) => {
                    let at = failure
                        .instruction
                        .map(|index| format!("instruction {}: ", index + 1))
                        .unwrap_or_default();
                    format!(
                        " · default limits, because the simulation failed ({at}{})",
                        failure.message
                    )
                }
            },
            Err(err) => format!(" · default limits, because the simulation could not run ({err})"),
        };
    }
    let mut last_valid_height = None;
    if settings.wants_blockhash() {
        let (hash, height) = rpc
            .get_latest_blockhash_with_height()
            .await
            .map_err(|err| FlowError::Refused(format!("Could not fetch a blockhash: {err}")))?;
        settings.blockhash = hash.to_string();
        last_valid_height = Some(height);
    }
    let compiled = compile_draft(fee_payer, instructions, &settings).map_err(FlowError::refused)?;
    Ok((compiled, note, last_valid_height))
}

/// The limits a simulation filled in, for the line under Compile.
fn limits_note(typed: &TxSettings, filled: &TxSettings) -> String {
    let mut parts = Vec::new();
    if filled.cu_limit != typed.cu_limit {
        parts.push(format!("CU limit {}", filled.cu_limit));
    }
    if filled.loaded_accounts != typed.loaded_accounts {
        parts.push(format!("loaded accounts {} bytes", filled.loaded_accounts));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(" · {} (simulated, plus 10%)", parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{nonce_account, pk, transfer_instruction, FakeRpc};
    use wallet_core::compile_probe;
    use wallet_core::simulate::SimulationReply;

    fn draft() -> Draft {
        Draft {
            fee_payer: Some(pk(1)),
            instructions: vec![transfer_instruction(pk(1), pk(2))],
            settings: TxSettings::default(),
        }
    }

    fn probe() -> CompiledTx {
        let draft = draft();
        compile_probe(draft.fee_payer, draft.instructions, &draft.settings).unwrap()
    }

    #[test]
    fn a_fresh_compile_is_one_request_for_the_blockhash() {
        let rpc = FakeRpc::default();
        let (wire, height) = pollster::block_on(compile_fresh(
            &rpc,
            pk(1),
            vec![transfer_instruction(pk(1), pk(2))],
        ))
        .unwrap();
        assert_eq!(wire.first(), Some(&wallet_core::V1_PREFIX));
        assert_eq!(height, 150);
        assert_eq!(rpc.asked(), 1);
    }

    fn on_nonce(rpc: &FakeRpc, signers: &[Pubkey]) -> Result<Vec<u8>, FlowError> {
        pollster::block_on(compile_on_nonce(
            rpc,
            pk(1),
            vec![transfer_instruction(pk(1), pk(2))],
            pk(7),
            signers,
        ))
    }

    #[test]
    fn a_compile_on_a_durable_nonce_advances_it_first_and_lives_on_its_hash() {
        let (account, hash) = nonce_account(pk(1));
        let rpc = FakeRpc::default().with_account(pk(7), account);
        let wire = on_nonce(&rpc, &[pk(1)]).unwrap();
        let tx = decode_v1_transaction(&wire).unwrap();
        assert!(tx.uses_durable_nonce);
        assert_eq!(tx.lifetime, hash);
        assert_eq!(
            tx.instructions[0].program_id,
            wallet_core::SYSTEM_PROGRAM_ID
        );
        assert_eq!(tx.instructions[0].accounts[0], pk(7));
        assert_eq!(tx.instructions.len(), 2);
        // One request, for the account. No blockhash is asked for.
        assert_eq!(rpc.asked(), 1);
    }

    #[test]
    fn a_nonce_whose_authority_does_not_sign_or_that_is_absent_is_refused() {
        // The authority is a plain key that votes, not the payer.
        let (account, _) = nonce_account(pk(4));
        let rpc = FakeRpc::default().with_account(pk(7), account);
        let tx = decode_v1_transaction(&on_nonce(&rpc, &[pk(1), pk(4)]).unwrap()).unwrap();
        let signers: Vec<Pubkey> = tx.signers.iter().map(|slot| slot.pubkey).collect();
        assert_eq!(signers, vec![pk(1), pk(4)]);

        let refused = on_nonce(&rpc, &[pk(1)]).unwrap_err().to_string();
        assert!(refused.contains(&pk(7).to_string()), "{refused}");
        assert!(refused.contains(&pk(4).to_string()), "{refused}");

        let absent = on_nonce(&FakeRpc::default(), &[pk(1)]).unwrap_err();
        assert!(absent.to_string().contains("does not exist"), "{absent}");
    }

    #[test]
    fn blank_limits_come_from_a_simulation_and_the_blockhash_is_fetched_last() {
        let rpc = FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                loaded_accounts_data_size: Some(1_000),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        };
        let (compiled, note, height) =
            pollster::block_on(compile_with_defaults(&rpc, draft(), &probe())).unwrap();
        assert!(!compiled.uses_durable_nonce);
        assert_eq!(
            note,
            " · CU limit 165 · loaded accounts 1100 bytes (simulated, plus 10%)"
        );
        assert_eq!(height, Some(150));
        assert_eq!(rpc.asked(), 2);
    }

    #[test]
    fn a_simulation_that_cannot_run_leaves_the_default_limits() {
        // No simulation was set up, so the fake refuses that request.
        let rpc = FakeRpc::default();
        let (_, note, _) =
            pollster::block_on(compile_with_defaults(&rpc, draft(), &probe())).unwrap();
        assert!(note.contains("default limits, because the simulation could not run"));
    }
}
