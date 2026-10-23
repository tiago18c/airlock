//! Transactions that mention an address, and its balance as it changes, from
//! the node's WebSocket, with Agave's `PubsubClient`.

use crate::Activity;
use futures::channel::{mpsc, oneshot};
use futures::stream::{select_all, Stream, StreamExt};
use solana_account_decoder_client_types::{UiAccountEncoding, UiDataSliceConfig};
use solana_client::nonblocking::pubsub_client::{PubsubClient, PubsubClientError};
use solana_client::rpc_config::{
    RpcAccountInfoConfig, RpcTransactionLogsConfig, RpcTransactionLogsFilter,
};
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use wallet_core::RpcError;

/// What one WebSocket connection reports. Ends when the connection does, or
/// when this is dropped, which closes it.
pub struct ActivityStream {
    events: mpsc::UnboundedReceiver<Activity>,
    _stop: oneshot::Sender<()>,
}

/// Each transaction and balance as it comes. The stream ends once the
/// connection is gone.
impl Stream for ActivityStream {
    type Item = Activity;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Activity>> {
        self.events.poll_next_unpin(cx)
    }
}

/// Open one connection to `ws_url` with a `logsSubscribe` and an
/// `accountSubscribe` for each of `addresses`, and answer once every
/// subscription is in place.
pub async fn subscribe_activity(
    ws_url: &str,
    addresses: &[Pubkey],
) -> Result<ActivityStream, RpcError> {
    let (ready_tx, ready_rx) = oneshot::channel();
    let (events_tx, events) = mpsc::unbounded();
    let (stop, stop_rx) = oneshot::channel::<()>();
    let url = ws_url.to_string();
    let addresses = addresses.to_vec();
    tokio::spawn(async move {
        let client = match PubsubClient::new(url.as_str()).await {
            Ok(client) => client,
            Err(err) => {
                let _ = ready_tx.send(Err(pubsub_error(err)));
                return;
            }
        };
        let mut streams = Vec::with_capacity(addresses.len() * 2);
        for address in &addresses {
            let address = *address;
            let logs = client
                .logs_subscribe(
                    RpcTransactionLogsFilter::Mentions(vec![address.to_string()]),
                    RpcTransactionLogsConfig {
                        commitment: Some(CommitmentConfig::confirmed()),
                    },
                )
                .await;
            match logs {
                Ok((stream, _unsubscribe)) => streams.push(
                    stream
                        .map(move |answer| {
                            let signature = Signature::from_str(&answer.value.signature).ok()?;
                            Some(Activity::Transaction {
                                address,
                                signature,
                                failed: answer.value.err.is_some(),
                                slot: answer.context.slot,
                            })
                        })
                        .boxed(),
                ),
                Err(err) => {
                    let _ = ready_tx.send(Err(pubsub_error(err)));
                    return;
                }
            }
            let balance = client
                .account_subscribe(
                    &address,
                    Some(RpcAccountInfoConfig {
                        encoding: Some(UiAccountEncoding::Base64),
                        data_slice: Some(UiDataSliceConfig {
                            offset: 0,
                            length: 0,
                        }),
                        commitment: Some(CommitmentConfig::confirmed()),
                        min_context_slot: None,
                    }),
                )
                .await;
            match balance {
                Ok((stream, _unsubscribe)) => streams.push(
                    stream
                        .map(move |answer| {
                            Some(Activity::Balance {
                                address,
                                lamports: answer.value.lamports,
                                slot: answer.context.slot,
                            })
                        })
                        .boxed(),
                ),
                Err(err) => {
                    let _ = ready_tx.send(Err(pubsub_error(err)));
                    return;
                }
            }
        }
        if ready_tx.send(Ok(())).is_err() {
            return;
        }
        let mut merged = select_all(streams);
        let mut stop_rx = stop_rx;
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                next = merged.next() => {
                    let Some(activity) = next else {
                        break;
                    };
                    let Some(activity) = activity else {
                        continue;
                    };
                    if events_tx.unbounded_send(activity).is_err() {
                        break;
                    }
                }
            }
        }
        drop(merged);
        let _ = client.shutdown().await;
    });
    match ready_rx.await {
        Ok(Ok(())) => Ok(ActivityStream {
            events,
            _stop: stop,
        }),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(RpcError::Unreachable(
            "the WebSocket connection ended before it was ready".into(),
        )),
    }
}

fn pubsub_error(err: PubsubClientError) -> RpcError {
    match err {
        PubsubClientError::ConnectionError(inner) => RpcError::Unreachable(inner.to_string()),
        PubsubClientError::WsError(inner) => RpcError::Unreachable(inner.to_string()),
        PubsubClientError::ConnectionClosed(reason) => RpcError::Unreachable(reason),
        other => RpcError::Message(other.to_string()),
    }
}
