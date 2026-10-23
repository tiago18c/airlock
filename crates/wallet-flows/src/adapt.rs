//! Rebuilding a transaction for a wallet that has no key: a programmatic
//! signer, a Vector wallet or a Squads vault.

use crate::error::FlowError;
use crate::simulate::{measure, simulate};
use solana_pubkey::Pubkey;
use wallet_core::adapt::{swap_fee_payer, AdaptPlan, Adapted, AdaptedStep, HeldBack, PayerSwap};
use wallet_core::relay::{
    fit_compute_units, fit_loaded_accounts, with_compute_unit_limit,
};
use wallet_core::{decode_v1_transaction, DecodedTx, Phrase, SolanaRpc, MAX_TRANSACTION_SIZE};

/// Carry `plan` out for `tx`, the transaction it was made for: read the
/// accounts it needs, rebuild on a fresh blockhash, and bring the compute
/// and loaded-accounts limits down to what the node says the result uses.
pub async fn rebuild(
    rpc: &dyn SolanaRpc,
    tx: &DecodedTx,
    plan: &AdaptPlan,
) -> Result<(Adapted, u64), FlowError> {
    let addresses = plan.accounts_to_read();
    let accounts: Vec<_> = addresses
        .iter()
        .zip(rpc.get_accounts(&addresses).await?)
        .filter_map(|(address, account)| Some((*address, account?)))
        .collect();
    let (blockhash, last_valid_height) = rpc.get_latest_blockhash_with_height().await?;
    let mut adapted = plan
        .build(tx, &accounts, blockhash)
        .map_err(FlowError::refused)?;
    // The wrap starts loaded accounts at the maximum, but keeps the inner
    // compute limit plus a fixed overhead. Raise compute to the maximum too,
    // measure, then fit both. A node that cannot say leaves the wire as it
    // is. An authorization that is not signed yet may still under-count
    // compute; Sign fits again once the authorization is signed.
    if decode_v1_transaction(&adapted.wire).is_ok() {
        if let Ok(fitted) = fit_limits_from_measure(rpc, &adapted.wire).await {
            adapted.wire = fitted;
        }
    }
    Ok((adapted, last_valid_height))
}

/// Probe `wire` at the highest compute limit, measure what it uses, and write
/// both limits back with the usual margin. Authorization signatures stay.
pub async fn fit_limits_from_measure(
    rpc: &dyn SolanaRpc,
    wire: &[u8],
) -> Result<Vec<u8>, FlowError> {
    // `with_compute_unit_limit` caps at the network maximum.
    let probe = with_compute_unit_limit(wire, u32::MAX).map_err(FlowError::refused)?;
    let probed = decode_v1_transaction(&probe).map_err(FlowError::refused)?;
    let simulation = measure(rpc, &probed).await?;
    apply_measured_limits(&probe, &simulation)
}

/// Simulate `wire` at the highest compute limit (for balance changes), then
/// write both limits back with the usual margin. Authorization signatures
/// stay. Returns the fitted wire and the simulation of the probe.
///
/// When a signature over the transaction is already in place, limits are left
/// alone: rewriting them would void that signature. The simulation then runs
/// on `wire` as it is, which is what would be submitted.
pub async fn simulate_fitting(
    rpc: &dyn SolanaRpc,
    wire: &[u8],
) -> Result<(Vec<u8>, wallet_core::simulate::Simulation), FlowError> {
    let original = decode_v1_transaction(wire).map_err(FlowError::refused)?;
    if original.signers.iter().any(|slot| slot.filled) {
        let simulation = simulate(rpc, &original).await?;
        return Ok((wire.to_vec(), simulation));
    }
    let probe = with_compute_unit_limit(wire, u32::MAX).map_err(FlowError::refused)?;
    let probed = decode_v1_transaction(&probe).map_err(FlowError::refused)?;
    let simulation = simulate(rpc, &probed).await?;
    let fitted = apply_measured_limits(&probe, &simulation)?;
    Ok((fitted, simulation))
}

fn apply_measured_limits(
    wire: &[u8],
    simulation: &wallet_core::simulate::Simulation,
) -> Result<Vec<u8>, FlowError> {
    let mut fitted = wire.to_vec();
    if let Some(units) = simulation.units_consumed {
        fitted = fit_compute_units(&fitted, units).map_err(FlowError::refused)?;
    }
    if let Some(size) = simulation.loaded_accounts_data_size {
        fitted = fit_loaded_accounts(&fitted, size).map_err(FlowError::refused)?;
    }
    Ok(fitted)
}

/// `tx` with the fee payer of `swap`, when the node says that would succeed.
///
/// Simulated before any rebuild: a simulation checks no signature, so an
/// address without a key can stand as a signer in it.
pub async fn swap_payer_if_it_works(
    rpc: &dyn SolanaRpc,
    tx: &DecodedTx,
    swap: &PayerSwap,
) -> Option<Vec<u8>> {
    let swapped = swap_fee_payer(tx, swap).ok()?;
    let simulation = simulate(rpc, &decode_v1_transaction(&swapped).ok()?)
        .await
        .ok()?;
    simulation.error.is_none().then_some(swapped)
}

/// One sentence for why a proposal was not taken all the way.
pub fn held_back_note(held: &HeldBack) -> String {
    match held {
        HeldBack::Size { bytes } => format!(
            "Going further would make it {bytes} of {MAX_TRANSACTION_SIZE} bytes, so this transaction stops here. Finish it from Proposals."
        ),
        HeldBack::Compute { units } => format!(
            "Going further would need {units} compute units, above the limit, so this transaction stops here. Finish it from Proposals."
        ),
        HeldBack::TimeLock => {
            "The multisig's time lock holds execution, so the proposal is left approved.".into()
        }
        HeldBack::NoExecutor => {
            "Nobody among the voters may execute, so the proposal waits for a member that can."
                .into()
        }
        HeldBack::Order => {
            "Executing too would put another member's vote inside the programmatic signer's authorization, so the proposal is left for a member to execute from Proposals.".into()
        }
        HeldBack::Votes { missing } => format!(
            "It still needs {missing} more approval(s) from members that can vote in this transaction."
        ),
    }
}

/// What a rebuild did, for the note above the transaction. Each wallet is
/// named by its address, so the screen shows it with its current name.
pub fn rebuilt_note(adapted: &Adapted) -> Phrase {
    let mut note = Phrase::default();
    let mut sentence = |part: Phrase| {
        if !note.0.is_empty() {
            note.push_text(" ");
        }
        note = std::mem::take(&mut note).and(part);
    };
    for step in &adapted.steps {
        if let AdaptedStep::Squads {
            vault,
            member,
            index,
            voters,
            executor,
            held_back,
            ..
        } = step
        {
            let done = match (voters.len(), executor.is_some()) {
                (0, _) => "creates".into(),
                (1, true) => "creates, approves and executes".into(),
                (1, false) => "creates and approves".into(),
                (n, true) => format!("creates, approves as {n} members and executes"),
                (n, false) => format!("creates and approves as {n} members"),
            };
            let mut part = Phrase::default()
                .t("Rebuilt for ")
                .a(*vault)
                .t(format!(
                    ": the instructions are stored as its transaction {index}, in a proposal "
                ))
                .a(*member)
                .t(format!(" {done}."));
            if let Some(held) = held_back {
                part = part.t(format!(" {}", held_back_note(held)));
            }
            sentence(part);
        }
    }
    // Several programmatic signers share one authorization, and several
    // Vector wallets one Advance, so each kind gets one sentence.
    let signers: Vec<Pubkey> = adapted
        .steps
        .iter()
        .filter_map(|step| match step {
            AdaptedStep::Programmatic { address, .. } => Some(*address),
            AdaptedStep::Squads { .. } | AdaptedStep::Vector { .. } => None,
        })
        .collect();
    match signers.as_slice() {
        [] => {}
        [one] => sentence(
            Phrase::default()
                .t("Rebuilt for ")
                .a(*one)
                .t(": the instructions run through its authorization."),
        ),
        many => sentence(Phrase::default().t("Rebuilt for ").and(listed(many)).t(
            ": the instructions run through one authorization, which the authority of each signs.",
        )),
    }
    let accounts: Vec<Pubkey> = adapted
        .steps
        .iter()
        .filter_map(|step| match step {
            AdaptedStep::Vector { account, .. } => Some(*account),
            AdaptedStep::Squads { .. } | AdaptedStep::Programmatic { .. } => None,
        })
        .collect();
    match accounts.as_slice() {
        [] => {}
        [one] => sentence(
            Phrase::default()
                .t("Rebuilt for ")
                .a(*one)
                .t(": what it signs runs as its account, once its key signs the whole transaction."),
        ),
        many => sentence(
            Phrase::default()
                .t("Rebuilt for ")
                .and(listed(many))
                .t(": what each signs runs as its account, once the key of each signs the whole transaction."),
        ),
    }
    sentence(Phrase::text("Read it again before signing."));
    note
}

/// "A, B and C", with each an address.
fn listed(addresses: &[Pubkey]) -> Phrase {
    let mut out = Phrase::default();
    for (i, address) in addresses.iter().enumerate() {
        if i > 0 {
            out.push_text(if i + 1 == addresses.len() {
                " and "
            } else {
                ", "
            });
        }
        out.push_address(*address);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, transfer, FakeRpc};
    use wallet_core::simulate::SimulationReply;
    use wallet_core::{apply_local_signatures, compile_v1, KeypairSigner, WalletSigner};

    fn rebuilt_for(wallets: &[&str]) -> Adapted {
        Adapted {
            wire: Vec::new(),
            steps: wallets
                .iter()
                .enumerate()
                .map(|(i, wallet)| AdaptedStep::Programmatic {
                    wallet: wallet.to_string(),
                    authority: pk(1),
                    address: pk(10 + i as u8),
                })
                .collect(),
            fee_payer: pk(3),
            voided: 0,
            context: Vec::new(),
        }
    }

    #[test]
    fn a_rebuild_for_several_programmatic_signers_is_told_in_one_sentence() {
        let short = |n: u8| wallet_core::short_pubkey(&pk(n));
        assert_eq!(
            rebuilt_note(&rebuilt_for(&["Alice"])).plain(),
            format!("Rebuilt for {}: the instructions run through its authorization. Read it again before signing.", short(10))
        );
        assert_eq!(
            rebuilt_note(&rebuilt_for(&["Alice", "Bob"])).plain(),
            format!("Rebuilt for {} and {}: the instructions run through one authorization, which the authority of each signs. Read it again before signing.", short(10), short(11))
        );
        assert_eq!(
            rebuilt_note(&rebuilt_for(&["Alice", "Bob", "Carol"])).plain(),
            format!("Rebuilt for {}, {} and {}: the instructions run through one authorization, which the authority of each signs. Read it again before signing.", short(10), short(11), short(12))
        );
    }

    #[test]
    fn held_back_size_names_the_byte_count() {
        assert_eq!(
            held_back_note(&HeldBack::Size { bytes: 4412 }),
            "Going further would make it 4412 of 4096 bytes, so this transaction stops here. Finish it from Proposals."
        );
    }

    #[test]
    fn simulate_fitting_leaves_a_signed_transaction_alone() {
        let payer = KeypairSigner::new_random();
        let wire = compile_v1(
            payer.pubkey(),
            vec![crate::testing::transfer_instruction(payer.pubkey(), pk(2))],
            None,
            Some(solana_hash::Hash::default()),
        )
        .unwrap()
        .wire;
        let signed = apply_local_signatures(&wire, &[&payer as &dyn WalletSigner]).unwrap();
        assert!(decode_v1_transaction(&signed).unwrap().signers[0].filled);

        let rpc = FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                loaded_accounts_data_size: Some(1_000),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        };
        let (fitted, _) =
            pollster::block_on(simulate_fitting(&rpc, &signed)).unwrap();
        assert_eq!(fitted, signed);
        assert!(decode_v1_transaction(&fitted).unwrap().signers[0].filled);
    }

    #[test]
    fn simulate_fitting_rewrites_limits_while_unsigned() {
        let tx = transfer(pk(1), pk(2));
        let rpc = FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(150),
                loaded_accounts_data_size: Some(1_000),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        };
        let (fitted, _) =
            pollster::block_on(simulate_fitting(&rpc, &tx.wire)).unwrap();
        let budget = decode_v1_transaction(&fitted).unwrap().budget;
        assert_eq!(budget.compute_unit_limit, Some(165));
        assert_eq!(budget.loaded_accounts_data_size_limit, Some(1_100));
        assert!(!decode_v1_transaction(&fitted).unwrap().signers[0].filled);
    }
}
