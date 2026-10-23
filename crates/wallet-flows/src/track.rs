//! Poll a submitted signature until it confirms, fails, expires, or times out.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use solana_signature::Signature;
use wallet_core::{RpcError, SolanaRpc};

/// Sleep future: `Send` on native (tokio), not on wasm.
#[cfg(not(target_arch = "wasm32"))]
type SleepFut = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
type SleepFut = Pin<Box<dyn Future<Output = ()> + 'static>>;

/// How often to ask `getSignatureStatuses`.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Give up with [`TrackResult::Unknown`] after this long with no terminal status.
pub const TRACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Terminal outcome of tracking one signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackResult {
    Confirmed { slot: u64 },
    Failed { error: String },
    Expired,
    Unknown,
}

/// A node answers for at most this many signatures in one request.
const SIGNATURES_PER_REQUEST: usize = 256;

/// One poll of status (and block height when a last-valid height is known).
/// Returns `Ok(None)` when the signature is still pending.
pub async fn poll_once(
    rpc: &dyn SolanaRpc,
    signature: &Signature,
    last_valid_height: Option<u64>,
) -> Result<Option<TrackResult>, RpcError> {
    let mut done = poll_many(rpc, &[(*signature, last_valid_height)]).await?;
    Ok(done.pop().flatten())
}

/// One poll of every signature in `watched`, each with the height its
/// blockhash expires at when that is known: one status request per 256
/// signatures, and one block height for all of them when one still pending
/// can expire. Answers in the order asked, `None` for one still pending.
pub async fn poll_many(
    rpc: &dyn SolanaRpc,
    watched: &[(Signature, Option<u64>)],
) -> Result<Vec<Option<TrackResult>>, RpcError> {
    let mut statuses = Vec::with_capacity(watched.len());
    for batch in watched.chunks(SIGNATURES_PER_REQUEST) {
        let signatures: Vec<Signature> = batch.iter().map(|(signature, _)| *signature).collect();
        let mut answered = rpc.get_signature_statuses(&signatures).await?;
        answered.resize(batch.len(), None);
        statuses.extend(answered);
    }
    let mut height = None;
    let mut out = Vec::with_capacity(watched.len());
    for ((_, last_valid_height), status) in watched.iter().zip(statuses) {
        let result = match status {
            Some(s) if s.err.is_some() => Some(TrackResult::Failed {
                error: s.err.unwrap_or_default(),
            }),
            Some(s) if s.confirmation.is_some_and(|c| c.at_least_confirmed()) => {
                Some(TrackResult::Confirmed { slot: s.slot })
            }
            Some(_) | None => match last_valid_height {
                Some(limit) => {
                    let now = match height {
                        Some(now) => now,
                        None => *height.insert(rpc.get_block_height().await?),
                    };
                    (now > *limit).then_some(TrackResult::Expired)
                }
                None => None,
            },
        };
        out.push(result);
    }
    Ok(out)
}

/// Poll every [`POLL_INTERVAL`] until a terminal result or `deadline_ms`.
///
/// `sleep` and `now_ms` come from the app's clock (or a manual clock in tests).
/// RPC errors are retried on the next poll; only a terminal status, expiry, or
/// the deadline stops the loop.
pub async fn track(
    rpc: &dyn SolanaRpc,
    signature: &Signature,
    last_valid_height: Option<u64>,
    deadline_ms: u64,
    mut sleep: impl FnMut(Duration) -> SleepFut,
    mut now_ms: impl FnMut() -> u64,
) -> TrackResult {
    loop {
        if now_ms() >= deadline_ms {
            return TrackResult::Unknown;
        }
        if let Ok(Some(done)) = poll_once(rpc, signature, last_valid_height).await {
            return done;
        }
        if now_ms() >= deadline_ms {
            return TrackResult::Unknown;
        }
        sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeRpc;
    use std::sync::{Arc, Mutex};
    use wallet_core::{ConfirmationLevel, SignatureStatus};

    fn sig(n: u8) -> Signature {
        Signature::from([n; 64])
    }

    /// Sleep that advances a shared elapsed-ms counter.
    #[allow(dead_code)]
    fn manual(elapsed: Arc<Mutex<u64>>) -> (impl FnMut(Duration) -> SleepFut, impl FnMut() -> u64) {
        let for_sleep = elapsed.clone();
        let sleep = move |d: Duration| {
            let elapsed = for_sleep.clone();
            Box::pin(async move {
                *elapsed.lock().unwrap() += d.as_millis() as u64;
            }) as SleepFut
        };
        let for_now = elapsed;
        let now = move || *for_now.lock().unwrap();
        (sleep, now)
    }

    #[test]
    fn poll_once_to_confirmed() {
        let rpc = FakeRpc::default().with_signature_status(
            sig(1),
            SignatureStatus {
                slot: 9,
                err: None,
                confirmation: Some(ConfirmationLevel::Confirmed),
            },
        );
        let done = pollster::block_on(poll_once(&rpc, &sig(1), None)).unwrap();
        assert_eq!(done, Some(TrackResult::Confirmed { slot: 9 }));
    }

    #[test]
    fn poll_once_keeps_failed_error_text() {
        let rpc = FakeRpc::default().with_signature_status(
            sig(2),
            SignatureStatus {
                slot: 3,
                err: Some("InstructionError(0, Custom(1))".into()),
                confirmation: Some(ConfirmationLevel::Confirmed),
            },
        );
        let done = pollster::block_on(poll_once(&rpc, &sig(2), None)).unwrap();
        assert_eq!(
            done,
            Some(TrackResult::Failed {
                error: "InstructionError(0, Custom(1))".into(),
            })
        );
    }

    #[test]
    fn many_signatures_are_one_status_request_and_one_height() {
        let rpc = FakeRpc::default()
            .with_block_height(100)
            .with_signature_status(
                sig(1),
                SignatureStatus {
                    slot: 9,
                    err: None,
                    confirmation: Some(ConfirmationLevel::Confirmed),
                },
            );
        let done = pollster::block_on(poll_many(
            &rpc,
            &[
                (sig(1), Some(50)),
                (sig(2), Some(50)),
                (sig(3), Some(200)),
                (sig(4), None),
            ],
        ))
        .unwrap();
        assert_eq!(
            done,
            vec![
                Some(TrackResult::Confirmed { slot: 9 }),
                Some(TrackResult::Expired),
                None,
                None
            ]
        );
        assert_eq!(rpc.asked(), 2);
    }

    #[test]
    fn poll_once_expires_past_last_valid_height() {
        let rpc = FakeRpc::default().with_block_height(100);
        let done = pollster::block_on(poll_once(&rpc, &sig(3), Some(50))).unwrap();
        assert_eq!(done, Some(TrackResult::Expired));
    }
}
