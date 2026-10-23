//! Keeping a blockhash countdown up to date while the app is online.

use crate::actions::unix_now;
use crate::state::forms::{BlockhashClock, BlockhashExpiry};
use crate::state::use_network;
use app_core::Reach;
use dioxus::prelude::*;
use wallet_core::{TxLifetime, FRESH_BLOCKHASH_BLOCKS};

/// Owns the 1 Hz clock. Writes `expired` only when the boolean flips, so
/// parents do not re-render every second. Updates `label` every tick for a
/// leaf that displays it.
#[component]
pub fn BlockhashCountdown(
    clock: BlockhashClock,
    lifetime: Option<TxLifetime>,
    mut expired: Signal<bool>,
    mut label: Signal<String>,
) -> Element {
    // Subscribing here re-renders this leaf every second; parents only see
    // `expired` / `label` when those values change.
    let tick = use_blockhash_clock(clock, lifetime);
    let _ = tick;
    let value = clock.expired(unix_now());
    if *expired.peek() != value {
        expired.set(value);
    }
    let text = countdown_text(clock).unwrap_or_else(|| "…".into());
    if *label.peek() != text {
        label.set(text);
    }
    rsx! {}
}

/// The ticking label written by [`BlockhashCountdown`]. A leaf so only this
/// redraws each second.
#[component]
pub fn CountdownLabel(label: Signal<String>) -> Element {
    rsx! { "{label()}" }
}

/// Poll the node about the blockhash of `lifetime` while the app is online:
/// the height about every 10 seconds, whether the hash is still valid about
/// every 5. Returns a counter that moves every second, so the countdown
/// redraws. There is no clock for a durable nonce, or when nothing is loaded.
///
/// The node is not asked while it is unreachable, and not at all once it has
/// said the hash is no longer valid or the height has passed its expiry: the
/// countdown then stays at "expired".
///
/// A clock without an expiry gets an estimate: the blockhash was fresh when
/// the height was first read.
pub fn use_blockhash_clock(clock: BlockhashClock, lifetime: Option<TxLifetime>) -> u64 {
    let network = use_network();
    let mut tick = use_signal(|| 0u64);
    // Bumped whenever polling starts or stops, so an older loop ends.
    let mut generation = use_signal(|| 0u64);
    let online = network.is_some();
    use_effect(use_reactive!(|(lifetime, online)| {
        let mine = *generation.peek() + 1;
        generation.set(mine);
        let Some(TxLifetime::Blockhash(hash)) = lifetime.filter(|_| online) else {
            return;
        };
        let Some(network) = network else {
            return;
        };
        let current = move || *generation.peek() == mine;
        let BlockhashClock {
            mut expiry,
            mut height,
            mut valid,
        } = clock;
        // An answer about an earlier hash says nothing about this one.
        valid.set(None);
        spawn(async move {
            let mut elapsed = 0u64;
            loop {
                crate::rt::sleep(std::time::Duration::from_secs(1)).await;
                if !current() {
                    break;
                }
                elapsed += 1;
                tick.set(elapsed);
                if network.status().reach == Reach::Unreachable {
                    continue;
                }
                let Ok(rpc) = network.rpc() else {
                    continue;
                };
                if elapsed == 1 || elapsed.is_multiple_of(5) {
                    if let Ok(answer) = rpc.is_blockhash_valid(&hash).await {
                        if !current() {
                            break;
                        }
                        valid.set(Some(answer));
                        if !answer {
                            break;
                        }
                    }
                }
                if elapsed == 1 || elapsed.is_multiple_of(10) {
                    if let Ok(read) = rpc.get_block_height().await {
                        if !current() {
                            break;
                        }
                        if expiry.peek().is_none() {
                            expiry.set(Some(BlockhashExpiry {
                                last_valid_height: read.saturating_add(FRESH_BLOCKHASH_BLOCKS),
                                exact: false,
                            }));
                        }
                        height.set(Some((read, unix_now())));
                        let past = expiry
                            .peek()
                            .is_some_and(|expiry| read > expiry.last_valid_height);
                        if past {
                            break;
                        }
                    }
                }
            }
        });
    }));
    tick()
}

/// "0:42", "about 0:42" for an estimate, or "expired". None before the node
/// has been asked.
pub fn countdown_text(clock: BlockhashClock) -> Option<String> {
    let (reading, exact) = clock.reading(unix_now())?;
    let time = wallet_core::format_countdown(reading.seconds_left);
    Some(if reading.expired {
        "expired".into()
    } else if exact {
        time
    } else {
        format!("about {time}")
    })
}
