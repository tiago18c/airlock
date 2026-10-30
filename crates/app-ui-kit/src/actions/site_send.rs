//! Sending what a site asked to have signed and sent. The site gets the
//! signature of what went out, which is the rebuilt transaction when it had to
//! be rebuilt, not the one it sent.

use app_core::bridge::SendPlan;
use app_core::FlowId;
use dioxus::prelude::*;
use wallet_core::parse_wire;
use wallet_flows::send::{send_each, wait_for_commitment, COMMITMENT_POLL, COMMITMENT_TIMEOUT};

use super::{sentence, spawn_app_task};
use crate::state::session::Session;

/// Why `wires` cannot go out as they are.
fn missing_signature(wires: &[(Vec<u8>, FlowId)]) -> Option<String> {
    wires.iter().enumerate().find_map(|(index, (wire, _))| {
        let complete = parse_wire(wire)
            .is_ok_and(|view| (0..view.num_required_signatures).all(|slot| view.slot_valid(slot)));
        (!complete).then(|| {
            format!(
                "Transaction {} still needs a signature, so it cannot be sent.",
                index + 1
            )
        })
    })
}

/// Send `items`, each a finished transaction with its flow, for request `id`,
/// and tell the site how it went. Each gets a transaction toast. An `Err` means
/// nothing went out and the request still waits.
pub fn send_for_site(
    session: &Session,
    id: &str,
    origin: &str,
    items: Vec<(Vec<u8>, FlowId)>,
) -> Result<(), String> {
    if let Some(why) = missing_signature(&items) {
        return Err(why);
    }
    let cluster = session.cluster();
    // The user may have changed the network since the modal opened.
    let chains = session
        .bridge
        .peek()
        .get(id)
        .map(|request| request.chains())
        .unwrap_or_default();
    if let Some(chain) = chains
        .iter()
        .find(|chain| cluster.differs_from_chain(chain))
    {
        return Err(format!(
            "This site is on {chain}, and Airlock is on {}. Change the network to send this.",
            cluster.name()
        ));
    }
    let rpc = session.rpc().map_err(sentence)?;
    let Some(sending) = session.command(|core| core.bridge_begin_send(id, items.len())) else {
        return Err("The site stopped waiting for this request.".into());
    };
    let plan = match sending.request.send_plan() {
        SendPlan::InOrder => wallet_flows::send::SendPlan::InOrder,
        SendPlan::Independent => wallet_flows::send::SendPlan::Independent,
        SendPlan::Serial => wallet_flows::send::SendPlan::Serial,
    };
    let commitments = sending.request.send_commitments();
    let submitter = session.submitter();
    let clock = session.core().ports().clock.clone();
    let label = format!("Sent for {origin}");
    let session = *session;
    spawn_app_task(async move {
        let results = send_each(
            plan,
            &commitments,
            |index| {
                let (wire, flow) = &items[index];
                let sent = submitter.submit(rpc.clone(), wire, *flow, &label, &cluster, None);
                async move { sent.await.map_err(sentence) }
            },
            |signature, level| {
                let rpc = rpc.clone();
                let clock = clock.clone();
                async move {
                    wait_for_commitment(
                        rpc.as_ref(),
                        signature,
                        level,
                        COMMITMENT_TIMEOUT,
                        COMMITMENT_POLL,
                        |every| clock.sleep(every),
                    )
                    .await
                }
            },
        )
        .await;
        let results = results
            .into_iter()
            .map(|result| result.map(<[u8; 64]>::from))
            .collect();
        session.command(|core| core.bridge_send_finished(sending, results));
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_core::WalletSigner;

    #[test]
    fn a_transaction_goes_out_only_with_every_signature() {
        let payer = wallet_core::KeypairSigner::new_random();
        let compiled = wallet_core::compile_v1(
            payer.pubkey(),
            vec![solana_system_interface::instruction::transfer(
                &payer.pubkey(),
                &solana_pubkey::Pubkey::new_from_array([2; 32]),
                1,
            )],
            None,
            Some(solana_hash::Hash::new_from_array([1; 32])),
        )
        .unwrap();
        let unsigned = compiled.wire;
        assert!(missing_signature(&[(unsigned.clone(), 1)])
            .unwrap()
            .contains("Transaction 1"));
        assert!(missing_signature(&[(vec![1, 2, 3], 1)]).is_some());

        let view = parse_wire(&unsigned).unwrap();
        let signature = payer.try_sign_message(&view.message).unwrap();
        let signed =
            wallet_core::insert_signature(&unsigned, &payer.pubkey(), signature.as_array())
                .unwrap();
        assert_eq!(missing_signature(&[(signed, 1)]), None);
    }
}
