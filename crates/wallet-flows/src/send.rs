//! Sending a transaction to a node.

use std::future::Future;
use std::time::Duration;

use solana_signature::Signature;
use thiserror::Error;
use wallet_core::{parse_wire, ConfirmationLevel, RpcError, SolanaRpc};

/// How long a site's send waits for the commitment it asked for. A blockhash
/// lasts about this long.
pub const COMMITMENT_TIMEOUT: Duration = Duration::from_secs(90);
/// How often the node is asked meanwhile.
pub const COMMITMENT_POLL: Duration = Duration::from_secs(1);

/// Why a send has no signature that is known to be on its way.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SendFailure {
    /// It did not go out, the node refused it, or it landed and failed.
    #[error("{0}")]
    Failed(String),
    /// Nobody can say yet. It may land, and whatever sends it again as a new
    /// transaction could do the same thing twice.
    #[error("{why}")]
    Unknown { signature: Signature, why: String },
}

/// How the transactions of one request a site made go out, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendPlan {
    /// One after the other, none held for the last to confirm. A rejected one
    /// stops the rest, as does one the node gave no answer about that has not
    /// been seen to land: `signAndSendTransaction`.
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
) -> Vec<Result<Signature, SendFailure>>
where
    S: FnMut(usize) -> SF,
    SF: Future<Output = Result<Signature, SendFailure>>,
    C: FnMut(Signature, ConfirmationLevel) -> CF,
    CF: Future<Output = Result<(), SendFailure>>,
{
    let mut results: Vec<Result<Signature, SendFailure>> = Vec::with_capacity(commitments.len());
    let mut stopped = false;
    for (index, commitment) in commitments.iter().enumerate() {
        if stopped {
            results.push(Err(SendFailure::Failed(NOT_SENT.to_owned())));
            continue;
        }
        let mut sent = submit(index).await;
        // Whether the rest go depends on this one, so find out.
        if plan != SendPlan::Independent {
            if let Err(SendFailure::Unknown { signature, .. }) = sent {
                let waited = confirm(signature, ConfirmationLevel::Processed).await;
                sent = after_waiting(sent, signature, waited);
            }
        }
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
            let wait = match &*result {
                Ok(signature) => commitment.map(|level| (*signature, level)),
                Err(SendFailure::Unknown { signature, .. }) if plan == SendPlan::Independent => {
                    Some((
                        *signature,
                        commitment.unwrap_or(ConfirmationLevel::Processed),
                    ))
                }
                _ => None,
            };
            if let Some((signature, level)) = wait {
                let waited = confirm(signature, level).await;
                *result = after_waiting(result.clone(), signature, waited);
            }
        }
    }
    results
}

/// What is known of a transaction once it has been waited for. One still
/// unknown keeps the reason it first was.
fn after_waiting(
    sent: Result<Signature, SendFailure>,
    signature: Signature,
    waited: Result<(), SendFailure>,
) -> Result<Signature, SendFailure> {
    match (sent, waited) {
        (_, Ok(())) => Ok(signature),
        (Err(first @ SendFailure::Unknown { .. }), Err(SendFailure::Unknown { .. })) => Err(first),
        (_, Err(failure)) => Err(failure),
    }
}

fn reached(level: ConfirmationLevel, at: Option<ConfirmationLevel>) -> bool {
    match level {
        ConfirmationLevel::Processed => true,
        ConfirmationLevel::Confirmed => at.is_some_and(ConfirmationLevel::at_least_confirmed),
        ConfirmationLevel::Finalized => at == Some(ConfirmationLevel::Finalized),
    }
}

/// Wait for `signature` to reach `level`, asking the node every `poll` for at
/// most `timeout`. A node that fails to answer is asked again, and the time it
/// takes to fail counts, by `now_ms`: one read of a dead node can take as long
/// as many polls.
pub async fn wait_for_commitment<Now, Sleep, SleepF>(
    rpc: &dyn SolanaRpc,
    signature: Signature,
    level: ConfirmationLevel,
    timeout: Duration,
    poll: Duration,
    now_ms: Now,
    mut sleep: Sleep,
) -> Result<(), SendFailure>
where
    Now: Fn() -> u64,
    Sleep: FnMut(Duration) -> SleepF,
    SleepF: Future<Output = ()>,
{
    let started = now_ms();
    let mut waited = Duration::ZERO;
    loop {
        let statuses = rpc.get_signature_statuses(&[signature]).await;
        if let Ok(Some(status)) = statuses.map(|mut found| found.pop().flatten()) {
            if let Some(err) = status.err {
                return Err(SendFailure::Failed(format!(
                    "The transaction failed: {err}"
                )));
            }
            if reached(level, status.confirmation) {
                return Ok(());
            }
        }
        let passed = Duration::from_millis(now_ms().saturating_sub(started));
        if waited.max(passed) >= timeout {
            return Err(SendFailure::Unknown {
                signature,
                why: format!(
                    "Transaction {signature} was sent, but did not reach that commitment in time. It may still land."
                ),
            });
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
    ) -> (Vec<Result<Signature, SendFailure>>, Vec<String>) {
        run_with_unknown(plan, commitments, failing_submit, &[], failing_confirm)
    }

    /// As [`run`], with the node giving no answer about those in `unknown`.
    fn run_with_unknown(
        plan: SendPlan,
        commitments: &[Option<ConfirmationLevel>],
        failing_submit: &[usize],
        unknown: &[usize],
        failing_confirm: &[u8],
    ) -> (Vec<Result<Signature, SendFailure>>, Vec<String>) {
        let log = std::cell::RefCell::new(Vec::new());
        let results = pollster::block_on(send_each(
            plan,
            commitments,
            |index| {
                log.borrow_mut().push(format!("send {index}"));
                let failed = failing_submit.contains(&index);
                let unknown = unknown.contains(&index);
                async move {
                    if failed {
                        Err(SendFailure::Failed("rejected".to_string()))
                    } else if unknown {
                        Err(SendFailure::Unknown {
                            signature: signature(index as u8),
                            why: "no answer".to_string(),
                        })
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
                        Err(SendFailure::Failed("failed".to_string()))
                    } else {
                        Ok(())
                    }
                }
            },
        ));
        (results, log.into_inner())
    }

    fn text(result: &Result<Signature, SendFailure>) -> String {
        result.as_ref().unwrap_err().to_string()
    }

    #[test]
    fn in_order_goes_on_after_no_answer_once_that_one_is_seen() {
        let (results, log) = run_with_unknown(SendPlan::InOrder, &[None, None], &[], &[0], &[]);
        assert_eq!(log, ["send 0", "confirm 0 Processed", "send 1"]);
        assert_eq!(results[0], Ok(signature(0)));
        assert_eq!(results[1], Ok(signature(1)));
    }

    /// As the node does when a transaction it gave no answer about is not seen.
    fn unseen(plan: SendPlan) -> (Vec<Result<Signature, SendFailure>>, Vec<String>) {
        let log = std::cell::RefCell::new(Vec::new());
        let results = pollster::block_on(send_each(
            plan,
            &[None, None],
            |index| {
                log.borrow_mut().push(format!("send {index}"));
                async move {
                    if index == 0 {
                        Err(SendFailure::Unknown {
                            signature: signature(0),
                            why: "no answer".to_string(),
                        })
                    } else {
                        Ok(signature(1))
                    }
                }
            },
            |signature, level| {
                let byte = signature.as_ref()[0];
                log.borrow_mut().push(format!("confirm {byte} {level:?}"));
                async move {
                    if byte == 0 {
                        Err(SendFailure::Unknown {
                            signature,
                            why: "not seen".to_string(),
                        })
                    } else {
                        Ok(())
                    }
                }
            },
        ));
        (results, log.into_inner())
    }

    #[test]
    fn in_order_stops_at_no_answer_that_is_never_seen_and_says_it_is_unknown() {
        let (results, log) = unseen(SendPlan::InOrder);
        assert_eq!(log, ["send 0", "confirm 0 Processed"]);
        assert!(matches!(&results[0], Err(SendFailure::Unknown { .. })));
        // Why it is unknown, rather than that it was not seen in the wait.
        assert_eq!(text(&results[0]), "no answer");
        assert_eq!(text(&results[1]), NOT_SENT);
    }

    #[test]
    fn serial_stops_at_no_answer_that_is_never_seen() {
        let (results, log) = unseen(SendPlan::Serial);
        assert_eq!(log, ["send 0", "confirm 0 Processed"]);
        assert!(matches!(&results[0], Err(SendFailure::Unknown { .. })));
        assert_eq!(text(&results[1]), NOT_SENT);
    }

    #[test]
    fn independent_holds_nothing_back_for_no_answer_and_looks_for_it_after() {
        let (results, log) = unseen(SendPlan::Independent);
        assert_eq!(log, ["send 0", "send 1", "confirm 0 Processed"]);
        assert_eq!(text(&results[0]), "no answer");
        assert_eq!(results[1], Ok(signature(1)));

        // Found after all, it is simply sent.
        let (results, _) = run_with_unknown(SendPlan::Independent, &[None], &[], &[0], &[]);
        assert_eq!(results[0], Ok(signature(0)));
    }

    #[test]
    fn in_order_stops_at_a_rejection_and_waits_only_when_asked() {
        let (results, log) = run(SendPlan::InOrder, &[None, None, None], &[1], &[]);
        assert_eq!(log, ["send 0", "send 1"]);
        assert!(results[0].is_ok());
        assert_eq!(text(&results[1]), "rejected");
        assert_eq!(text(&results[2]), NOT_SENT);

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
        assert_eq!(text(&results[1]), "rejected");
        assert_eq!(text(&results[2]), "failed");
    }

    #[test]
    fn serial_waits_for_each_and_a_failure_stops_the_rest() {
        let (results, log) = run(SendPlan::Serial, &[None, None, None], &[], &[0]);
        assert_eq!(log, ["send 0", "confirm 0 Confirmed"]);
        assert_eq!(text(&results[0]), "failed");
        assert_eq!(text(&results[1]), NOT_SENT);
        assert_eq!(text(&results[2]), NOT_SENT);

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

    fn wait(rpc: &FakeRpc, level: ConfirmationLevel) -> Result<(), SendFailure> {
        pollster::block_on(wait_for_commitment(
            rpc,
            signature(1),
            level,
            Duration::from_secs(2),
            Duration::from_secs(1),
            || 0,
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
        assert!(matches!(late, SendFailure::Unknown { .. }));
        assert!(late.to_string().contains("may still land"));
    }

    #[test]
    fn waiting_reports_a_failure_on_chain() {
        let rpc = FakeRpc::default();
        rpc.set_signature_status(signature(1), Some(status(Some("boom"), None)));
        let failed = wait(&rpc, ConfirmationLevel::Processed).unwrap_err();
        assert!(matches!(failed, SendFailure::Failed(_)));
        assert!(failed.to_string().contains("boom"));
    }

    #[test]
    fn waiting_gives_up_on_one_the_node_never_saw() {
        let rpc = FakeRpc::default();
        assert!(matches!(
            wait(&rpc, ConfirmationLevel::Processed),
            Err(SendFailure::Unknown { .. })
        ));
    }

    #[test]
    fn a_slow_node_uses_up_the_wait_however_few_times_it_is_asked() {
        let rpc = FakeRpc::default();
        // Each read of the clock finds another 30 seconds gone.
        let now = std::cell::Cell::new(0);
        let late = pollster::block_on(wait_for_commitment(
            &rpc,
            signature(1),
            ConfirmationLevel::Processed,
            Duration::from_secs(90),
            Duration::from_secs(1),
            || {
                now.set(now.get() + 30_000);
                now.get()
            },
            |_| async {},
        ));
        assert!(matches!(late, Err(SendFailure::Unknown { .. })));
        assert_eq!(rpc.asked(), 3);
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
