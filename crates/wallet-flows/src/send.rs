//! Sending a transaction to a node.

use std::future::Future;
use std::time::Duration;

use solana_signature::Signature;
use wallet_core::{parse_wire, ConfirmationLevel, RpcError, SolanaRpc};

/// How long a site's send waits for the commitment it asked for. A blockhash
/// lasts about this long.
pub const COMMITMENT_TIMEOUT: Duration = Duration::from_secs(90);
/// How often the node is asked meanwhile.
pub const COMMITMENT_POLL: Duration = Duration::from_secs(1);

/// How the transactions of one request a site made go out, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendPlan {
    /// One after the other, none held for the last to confirm. A rejected one
    /// stops the rest: `signAndSendTransaction`.
    InOrder,
    /// Every one is sent, whatever becomes of the others:
    /// `signAndSendAllTransactions` in `parallel` mode.
    Independent,
    /// Each waits for the last to confirm, and a failure stops the rest:
    /// `signAndSendAllTransactions` in `serial` mode.
    Serial,
}

const NOT_SENT: &str = "Not sent, because an earlier transaction failed.";

/// Send `commitments.len()` transactions by `plan`. `submit` sends one by its
/// index. `confirm` waits for one to reach a level: the level the site asked
/// for, or for `Serial`, confirmed when it asked for none. Answers one result
/// for each, in order.
pub async fn send_each<S, SF, C, CF>(
    plan: SendPlan,
    commitments: &[Option<ConfirmationLevel>],
    mut submit: S,
    mut confirm: C,
) -> Vec<Result<Signature, String>>
where
    S: FnMut(usize) -> SF,
    SF: Future<Output = Result<Signature, String>>,
    C: FnMut(Signature, ConfirmationLevel) -> CF,
    CF: Future<Output = Result<(), String>>,
{
    let mut results: Vec<Result<Signature, String>> = Vec::with_capacity(commitments.len());
    let mut stopped = false;
    for (index, commitment) in commitments.iter().enumerate() {
        if stopped {
            results.push(Err(NOT_SENT.to_owned()));
            continue;
        }
        let mut sent = submit(index).await;
        if plan == SendPlan::Serial {
            if let Ok(signature) = sent {
                let level = commitment.unwrap_or(ConfirmationLevel::Confirmed);
                sent = confirm(signature, level).await.map(|()| signature);
            }
        }
        stopped = sent.is_err() && plan != SendPlan::Independent;
        results.push(sent);
    }
    // Serial waited above. The rest wait once everything is on its way, so
    // none is held back by another.
    if plan != SendPlan::Serial {
        for (result, commitment) in results.iter_mut().zip(commitments) {
            if let (Ok(signature), Some(level)) = (&*result, commitment) {
                let signature = *signature;
                if let Err(error) = confirm(signature, *level).await {
                    *result = Err(error);
                }
            }
        }
    }
    results
}

fn reached(level: ConfirmationLevel, at: Option<ConfirmationLevel>) -> bool {
    match level {
        ConfirmationLevel::Processed => true,
        ConfirmationLevel::Confirmed => at.is_some_and(ConfirmationLevel::at_least_confirmed),
        ConfirmationLevel::Finalized => at == Some(ConfirmationLevel::Finalized),
    }
}

/// Wait for `signature` to reach `level`, asking the node every `poll` for at
/// most `timeout`. A node that fails to answer is asked again.
pub async fn wait_for_commitment<Sleep, SleepF>(
    rpc: &dyn SolanaRpc,
    signature: Signature,
    level: ConfirmationLevel,
    timeout: Duration,
    poll: Duration,
    mut sleep: Sleep,
) -> Result<(), String>
where
    Sleep: FnMut(Duration) -> SleepF,
    SleepF: Future<Output = ()>,
{
    let mut waited = Duration::ZERO;
    loop {
        let statuses = rpc.get_signature_statuses(&[signature]).await;
        if let Ok(Some(status)) = statuses.map(|mut found| found.pop().flatten()) {
            if let Some(err) = status.err {
                return Err(format!("The transaction failed: {err}"));
            }
            if reached(level, status.confirmation) {
                return Ok(());
            }
        }
        if waited >= timeout {
            return Err(format!(
                "Transaction {signature} was sent, but did not reach that commitment in time. It may still land."
            ));
        }
        sleep(poll).await;
        waited += poll;
    }
}

/// Submit a signed transaction in any format the node takes: legacy, v0 or
/// v1. Bytes that are not a transaction are refused before they reach the
/// node.
pub async fn submit(rpc: &dyn SolanaRpc, wire: &[u8]) -> Result<Signature, RpcError> {
    if parse_wire(wire).is_err() {
        return Err(RpcError::Message(
            "payload is not a Solana transaction".into(),
        ));
    }
    rpc.send_transaction(wire).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{legacy_transfer, pk, transfer, FakeRpc};

    fn signature(byte: u8) -> Signature {
        Signature::from([byte; 64])
    }

    /// Run `plan` over `failing`, answering which were submitted and confirmed.
    fn run(
        plan: SendPlan,
        commitments: &[Option<ConfirmationLevel>],
        failing_submit: &[usize],
        failing_confirm: &[u8],
    ) -> (Vec<Result<Signature, String>>, Vec<String>) {
        let log = std::cell::RefCell::new(Vec::new());
        let results = pollster::block_on(send_each(
            plan,
            commitments,
            |index| {
                log.borrow_mut().push(format!("send {index}"));
                let failed = failing_submit.contains(&index);
                async move {
                    if failed {
                        Err("rejected".to_string())
                    } else {
                        Ok(signature(index as u8))
                    }
                }
            },
            |signature, level| {
                let byte = signature.as_ref()[0];
                log.borrow_mut().push(format!("confirm {byte} {level:?}"));
                let failed = failing_confirm.contains(&byte);
                async move {
                    if failed {
                        Err("failed".to_string())
                    } else {
                        Ok(())
                    }
                }
            },
        ));
        (results, log.into_inner())
    }

    #[test]
    fn in_order_stops_at_a_rejection_and_waits_only_when_asked() {
        let (results, log) = run(SendPlan::InOrder, &[None, None, None], &[1], &[]);
        assert_eq!(log, ["send 0", "send 1"]);
        assert!(results[0].is_ok());
        assert_eq!(results[1].as_ref().unwrap_err(), "rejected");
        assert_eq!(results[2].as_ref().unwrap_err(), NOT_SENT);

        let asked = Some(ConfirmationLevel::Finalized);
        let (_, log) = run(SendPlan::InOrder, &[asked], &[], &[]);
        assert_eq!(log, ["send 0", "confirm 0 Finalized"]);
    }

    #[test]
    fn independent_sends_every_one_before_waiting_for_any() {
        let asked = Some(ConfirmationLevel::Confirmed);
        let (results, log) = run(SendPlan::Independent, &[asked, asked, asked], &[1], &[2]);
        assert_eq!(
            log,
            [
                "send 0",
                "send 1",
                "send 2",
                "confirm 0 Confirmed",
                "confirm 2 Confirmed"
            ]
        );
        assert!(results[0].is_ok());
        assert_eq!(results[1].as_ref().unwrap_err(), "rejected");
        assert_eq!(results[2].as_ref().unwrap_err(), "failed");
    }

    #[test]
    fn serial_waits_for_each_and_a_failure_stops_the_rest() {
        let (results, log) = run(SendPlan::Serial, &[None, None, None], &[], &[0]);
        assert_eq!(log, ["send 0", "confirm 0 Confirmed"]);
        assert_eq!(results[0].as_ref().unwrap_err(), "failed");
        assert_eq!(results[1].as_ref().unwrap_err(), NOT_SENT);
        assert_eq!(results[2].as_ref().unwrap_err(), NOT_SENT);

        let asked = Some(ConfirmationLevel::Finalized);
        let (results, log) = run(SendPlan::Serial, &[asked, None], &[], &[]);
        assert_eq!(
            log,
            [
                "send 0",
                "confirm 0 Finalized",
                "send 1",
                "confirm 1 Confirmed"
            ]
        );
        assert!(results.iter().all(Result::is_ok));
    }

    fn status(
        err: Option<&str>,
        confirmation: Option<ConfirmationLevel>,
    ) -> wallet_core::SignatureStatus {
        wallet_core::SignatureStatus {
            slot: 1,
            err: err.map(str::to_owned),
            confirmation,
        }
    }

    fn wait(rpc: &FakeRpc, level: ConfirmationLevel) -> Result<(), String> {
        pollster::block_on(wait_for_commitment(
            rpc,
            signature(1),
            level,
            Duration::from_secs(2),
            Duration::from_secs(1),
            |_| async {},
        ))
    }

    #[test]
    fn waiting_ends_at_the_level_asked_for() {
        let rpc = FakeRpc::default();
        rpc.set_signature_status(
            signature(1),
            Some(status(None, Some(ConfirmationLevel::Confirmed))),
        );
        assert!(wait(&rpc, ConfirmationLevel::Processed).is_ok());
        assert!(wait(&rpc, ConfirmationLevel::Confirmed).is_ok());
        let late = wait(&rpc, ConfirmationLevel::Finalized).unwrap_err();
        assert!(late.contains("may still land"));
    }

    #[test]
    fn waiting_reports_a_failure_on_chain() {
        let rpc = FakeRpc::default();
        rpc.set_signature_status(signature(1), Some(status(Some("boom"), None)));
        assert!(wait(&rpc, ConfirmationLevel::Processed)
            .unwrap_err()
            .contains("boom"));
    }

    #[test]
    fn waiting_gives_up_on_one_the_node_never_saw() {
        let rpc = FakeRpc::default();
        assert!(wait(&rpc, ConfirmationLevel::Processed).is_err());
    }

    #[test]
    fn only_a_transaction_reaches_the_node() {
        let rpc = FakeRpc::default();
        let err = pollster::block_on(submit(&rpc, &[1, 2, 3])).unwrap_err();
        assert!(err.to_string().contains("not a Solana transaction"));
        assert_eq!(rpc.asked(), 0);

        let wire = transfer(pk(1), pk(2)).wire;
        assert!(pollster::block_on(submit(&rpc, &wire)).is_ok());
        assert_eq!(rpc.asked(), 1);
    }

    #[test]
    fn a_legacy_transaction_is_sent_as_it_is() {
        let rpc = FakeRpc::default();
        let wire = legacy_transfer(pk(1), pk(2));
        assert!(pollster::block_on(submit(&rpc, &wire)).is_ok());
        assert_eq!(rpc.asked(), 1);
    }
}
