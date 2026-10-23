//! Transactions that mention an address, and its balance as it changes, from
//! the node's WebSocket, with the browser's own `WebSocket`.

use crate::Activity;
use futures::channel::{mpsc, oneshot};
use futures::future::{select, Either};
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use std::collections::HashMap;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use wallet_core::RpcError;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{CloseEvent, Event, MessageEvent, WebSocket};

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

/// What the socket's callbacks pass on.
enum Wire {
    Open,
    Text(String),
    Closed(String),
}

/// The socket and the callbacks it holds. Dropping it closes the socket.
struct Socket {
    ws: WebSocket,
    _on_open: Closure<dyn FnMut(Event)>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
    _on_error: Closure<dyn FnMut(Event)>,
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.ws.set_onopen(None);
        self.ws.set_onmessage(None);
        self.ws.set_onclose(None);
        self.ws.set_onerror(None);
        let _ = self.ws.close();
    }
}

impl Socket {
    fn open(url: &str) -> Result<(Self, mpsc::UnboundedReceiver<Wire>), RpcError> {
        let ws = WebSocket::new(url).map_err(|err| RpcError::Unreachable(format!("{err:?}")))?;
        let (tx, wire) = mpsc::unbounded();
        let on_open = {
            let tx = tx.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| {
                let _ = tx.unbounded_send(Wire::Open);
            })
        };
        let on_message = {
            let tx = tx.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                if let Some(text) = event.data().as_string() {
                    let _ = tx.unbounded_send(Wire::Text(text));
                }
            })
        };
        let on_close = {
            let tx = tx.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |event: CloseEvent| {
                let _ = tx.unbounded_send(Wire::Closed(format!(
                    "the WebSocket closed ({})",
                    event.code()
                )));
            })
        };
        let on_error = Closure::<dyn FnMut(Event)>::new(move |_| {
            let _ = tx.unbounded_send(Wire::Closed("the WebSocket failed".into()));
        });
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        Ok((
            Self {
                ws,
                _on_open: on_open,
                _on_message: on_message,
                _on_close: on_close,
                _on_error: on_error,
            },
            wire,
        ))
    }
}

/// What a subscription reports.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Logs,
    Account,
}

impl Kind {
    fn method(self) -> &'static str {
        match self {
            Self::Logs => "logsSubscribe",
            Self::Account => "accountSubscribe",
        }
    }
}

/// Open one connection to `ws_url` with a `logsSubscribe` and an
/// `accountSubscribe` for each of `addresses`, and answer once every
/// subscription is in place.
pub async fn subscribe_activity(
    ws_url: &str,
    addresses: &[Pubkey],
) -> Result<ActivityStream, RpcError> {
    let (socket, mut wire) = Socket::open(ws_url)?;
    loop {
        match wire.next().await {
            Some(Wire::Open) => break,
            Some(Wire::Text(_)) => {}
            Some(Wire::Closed(reason)) => return Err(RpcError::Unreachable(reason)),
            None => return Err(RpcError::Unreachable("the WebSocket closed".into())),
        }
    }
    // Request `id` asks for `asked[id - 1]`.
    let mut asked = Vec::with_capacity(addresses.len() * 2);
    for address in addresses {
        for kind in [Kind::Logs, Kind::Account] {
            let params = match kind {
                Kind::Logs => json!([
                    { "mentions": [address.to_string()] },
                    { "commitment": "confirmed" },
                ]),
                Kind::Account => json!([
                    address.to_string(),
                    {
                        "commitment": "confirmed",
                        "encoding": "base64",
                        "dataSlice": { "offset": 0, "length": 0 },
                    },
                ]),
            };
            asked.push((kind, *address));
            let request = json!({
                "jsonrpc": "2.0",
                "id": asked.len(),
                "method": kind.method(),
                "params": params,
            });
            socket
                .ws
                .send_with_str(&request.to_string())
                .map_err(|err| RpcError::Unreachable(format!("{err:?}")))?;
        }
    }
    let mut watched = HashMap::with_capacity(asked.len());
    while watched.len() < asked.len() {
        match wire.next().await {
            Some(Wire::Text(text)) => {
                let Ok(answer) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let Some(id) = answer.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                let request = usize::try_from(id)
                    .ok()
                    .and_then(|id| id.checked_sub(1))
                    .and_then(|index| asked.get(index));
                let Some(&(kind, address)) = request else {
                    continue;
                };
                if let Some(error) = answer.get("error") {
                    return Err(RpcError::Message(format!(
                        "{} failed: {error}",
                        kind.method()
                    )));
                }
                if let Some(subscription) = answer.get("result").and_then(Value::as_u64) {
                    watched.insert((kind, subscription), address);
                }
            }
            Some(Wire::Open) => {}
            Some(Wire::Closed(reason)) => return Err(RpcError::Unreachable(reason)),
            None => return Err(RpcError::Unreachable("the WebSocket closed".into())),
        }
    }

    let (events_tx, events) = mpsc::unbounded();
    let (stop, mut stop_rx) = oneshot::channel::<()>();
    wasm_bindgen_futures::spawn_local(async move {
        let _socket = socket;
        loop {
            match select(&mut stop_rx, wire.next()).await {
                Either::Left(_) => break,
                Either::Right((Some(Wire::Text(text)), _)) => {
                    let Some(activity) = notification(&text, &watched) else {
                        continue;
                    };
                    if events_tx.unbounded_send(activity).is_err() {
                        break;
                    }
                }
                Either::Right((Some(Wire::Open), _)) => {}
                Either::Right((Some(Wire::Closed(_)) | None, _)) => break,
            }
        }
    });
    Ok(ActivityStream {
        events,
        _stop: stop,
    })
}

/// A `logsNotification` or an `accountNotification` for one of the
/// `watched` subscriptions.
fn notification(text: &str, watched: &HashMap<(Kind, u64), Pubkey>) -> Option<Activity> {
    let message: Value = serde_json::from_str(text).ok()?;
    let kind = match message.get("method")?.as_str()? {
        "logsNotification" => Kind::Logs,
        "accountNotification" => Kind::Account,
        _ => return None,
    };
    let params = message.get("params")?;
    let address = *watched.get(&(kind, params.get("subscription")?.as_u64()?))?;
    let result = params.get("result")?;
    let slot = result.get("context")?.get("slot")?.as_u64()?;
    let value = result.get("value")?;
    match kind {
        Kind::Logs => {
            let signature = Signature::from_str(value.get("signature")?.as_str()?).ok()?;
            let failed = value.get("err").is_some_and(|err| !err.is_null());
            Some(Activity::Transaction {
                address,
                signature,
                failed,
                slot,
            })
        }
        Kind::Account => Some(Activity::Balance {
            address,
            lamports: value.get("lamports")?.as_u64()?,
            slot,
        }),
    }
}
