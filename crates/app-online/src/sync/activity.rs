//! The activity feed: one WebSocket to the node with a `logsSubscribe` and
//! an `accountSubscribe` for each wallet's address. A balance it pushes is
//! shown as it comes. A transaction that mentions one has the balances job
//! read again, with that address's token accounts, from a node that has
//! seen the transaction's slot. While the feed is up the balances job only
//! reads now and then, in case it missed something.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use app_core::{BoxFuture, PublicCore};
use futures::StreamExt;
use solana_pubkey::Pubkey;
use tracing::{debug, info, warn};
use wallet_core::{ChainSnapshot, Cluster, RpcError};
use wallet_rpc::Activity;

use super::balances::{
    apply_snapshot, wallet_addresses, BALANCES_INTERVAL, FEED_UP_BALANCES_INTERVAL,
};
use super::scheduler::{Scheduler, BALANCES_JOB};

const SYNC: &str = "airlock::sync";

/// How long opening the connection and its subscriptions may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// First wait before connecting again; it doubles up to [`MAX_RETRY`].
pub const FIRST_RETRY: Duration = Duration::from_secs(2);

/// Cap on the doubled wait before connecting again.
pub const MAX_RETRY: Duration = Duration::from_secs(300);

/// What one connection reports, until it ends.
#[cfg(not(target_arch = "wasm32"))]
pub type ActivityEvents = futures::stream::BoxStream<'static, Activity>;
#[cfg(target_arch = "wasm32")]
pub type ActivityEvents = futures::stream::LocalBoxStream<'static, Activity>;

/// Opens a connection to a WebSocket URL watching some addresses.
pub type ConnectFn =
    Arc<dyn Fn(String, Vec<Pubkey>) -> BoxFuture<Result<ActivityEvents, RpcError>> + Send + Sync>;

/// The node's own WebSocket, through `wallet-rpc`.
pub fn connect_to_node() -> ConnectFn {
    Arc::new(|url, addresses| {
        Box::pin(async move {
            let stream = wallet_rpc::subscribe_activity(&url, &addresses).await?;
            #[cfg(not(target_arch = "wasm32"))]
            let events = stream.boxed();
            #[cfg(target_arch = "wasm32")]
            let events = stream.boxed_local();
            Ok(events)
        })
    })
}

/// What the feed and the balances job share: the addresses the feed saw
/// move since the balances job last read their token accounts, the newest
/// slot the node told of, and the slot each balance shown was read at.
#[derive(Clone, Default)]
pub struct Moved(Arc<Mutex<Seen>>);

#[derive(Default)]
struct Seen {
    addresses: Vec<Pubkey>,
    /// The cluster the slots below are on.
    cluster: Option<Cluster>,
    slot: u64,
    balances: HashMap<Pubkey, u64>,
}

impl Seen {
    /// Forget the slots of another cluster.
    fn on(&mut self, cluster: &Cluster) {
        if self.cluster.as_ref() != Some(cluster) {
            self.cluster = Some(cluster.clone());
            self.slot = 0;
            self.balances.clear();
        }
    }
}

impl Moved {
    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn mark(&self, address: Pubkey) {
        let mut seen = self.seen();
        if !seen.addresses.contains(&address) {
            seen.addresses.push(address);
        }
    }

    /// Mark them again, after a read of them failed.
    pub fn mark_all(&self, addresses: &[Pubkey]) {
        for address in addresses {
            self.mark(*address);
        }
    }

    pub fn take(&self) -> Vec<Pubkey> {
        std::mem::take(&mut self.seen().addresses)
    }

    /// The node told of `slot` on `cluster`: reads from now on ask for a
    /// node that has seen it.
    pub fn saw(&self, cluster: &Cluster, slot: u64) {
        let mut seen = self.seen();
        seen.on(cluster);
        seen.slot = seen.slot.max(slot);
    }

    /// The newest slot the node told of on `cluster`, or zero.
    pub fn slot(&self, cluster: &Cluster) -> u64 {
        let mut seen = self.seen();
        seen.on(cluster);
        seen.slot
    }

    /// Of `balances`, read at `slot` on `cluster`, those no older than the
    /// one shown for the same address. They are shown from now on.
    pub fn fresher(
        &self,
        cluster: &Cluster,
        slot: u64,
        balances: Vec<(Pubkey, u64)>,
    ) -> Vec<(Pubkey, u64)> {
        let mut seen = self.seen();
        seen.on(cluster);
        balances
            .into_iter()
            .filter(|(address, _)| {
                let shown = seen.balances.entry(*address).or_default();
                if slot < *shown {
                    return false;
                }
                *shown = slot;
                true
            })
            .collect()
    }
}

/// Start the feed on the core's spawner. It follows the cluster and the
/// wallets' addresses, and connects again when the connection drops.
pub fn start_activity_feed(
    core: PublicCore,
    scheduler: Scheduler,
    moved: Moved,
    connect: ConnectFn,
) {
    let task_core = core.clone();
    core.ports().spawner.spawn(Box::pin(async move {
        Feed {
            core: task_core,
            scheduler,
            moved,
            connect,
        }
        .run()
        .await;
    }));
}

struct Feed {
    core: PublicCore,
    scheduler: Scheduler,
    moved: Moved,
    connect: ConnectFn,
}

/// Why a connection, or the wait for one, ended.
enum Ended {
    /// The cluster or the wallets' addresses changed: connect again now.
    Changed,
    /// The connection dropped, or could not be made: wait, then connect again.
    Lost,
    /// The app is closing.
    Closed,
}

impl Feed {
    async fn run(self) {
        let mut network = self.core.slices().network.subscribe();
        let mut wallets = self.core.slices().wallets.subscribe();
        let mut retry = FIRST_RETRY;
        // Kick the balances job once a dropped connection is back, for what
        // happened while it was down.
        let mut catch_up = false;
        loop {
            let cluster = network.borrow_and_update().cluster.clone();
            let addresses = wallet_addresses(&wallets.borrow_and_update());
            let ended = if addresses.is_empty() {
                self.feed_down();
                self.until_changed(&mut network, &mut wallets, &cluster, &addresses, None)
                    .await
            } else {
                self.follow(
                    &mut network,
                    &mut wallets,
                    &cluster,
                    &addresses,
                    &mut catch_up,
                    &mut retry,
                )
                .await
            };
            match ended {
                Ended::Closed => return,
                Ended::Changed => {
                    // What was marked belongs to the old addresses or cluster.
                    self.moved.take();
                    catch_up = false;
                    retry = FIRST_RETRY;
                }
                Ended::Lost => {
                    catch_up = true;
                    let waited = self
                        .until_changed(
                            &mut network,
                            &mut wallets,
                            &cluster,
                            &addresses,
                            Some(retry),
                        )
                        .await;
                    match waited {
                        Ended::Closed => return,
                        Ended::Changed => {
                            self.moved.take();
                            catch_up = false;
                            retry = FIRST_RETRY;
                        }
                        Ended::Lost => retry = retry.saturating_mul(2).min(MAX_RETRY),
                    }
                }
            }
        }
    }

    /// Connect, then pass each transaction on until the connection ends.
    async fn follow(
        &self,
        network: &mut tokio::sync::watch::Receiver<Arc<app_core::NetworkStatus>>,
        wallets: &mut tokio::sync::watch::Receiver<Arc<app_core::WalletsView>>,
        cluster: &Cluster,
        addresses: &[Pubkey],
        catch_up: &mut bool,
        retry: &mut Duration,
    ) -> Ended {
        let url = cluster.ws_url();
        let redacted = app_core::diagnostics::redact_url(&url);
        let clock = self.core.ports().clock.clone();
        let opened = tokio::select! {
            opened = (self.connect)(url, addresses.to_vec()) => opened,
            _ = clock.sleep(CONNECT_TIMEOUT) => Err(RpcError::Timeout),
            ended = self.until_changed(network, wallets, cluster, addresses, None) => return ended,
        };
        let mut events = match opened {
            Ok(events) => events,
            Err(err) => {
                warn!(target: SYNC, ws = %redacted, error = %err, "activity feed could not connect");
                self.feed_down();
                return Ended::Lost;
            }
        };
        info!(
            target: SYNC,
            ws = %redacted,
            addresses = addresses.len(),
            "activity feed up"
        );
        self.feed_up();
        *retry = FIRST_RETRY;
        if std::mem::take(catch_up) {
            self.scheduler.kick(BALANCES_JOB);
        }
        let ended = loop {
            tokio::select! {
                next = events.next() => match next {
                    Some(activity) => self.on_activity(cluster, activity),
                    None => {
                        warn!(target: SYNC, ws = %redacted, "activity feed dropped");
                        break Ended::Lost;
                    }
                },
                ended = self.until_changed(network, wallets, cluster, addresses, None) => break ended,
            }
        };
        self.feed_down();
        ended
    }

    /// A transaction has the balances job read again, with that address's
    /// token accounts. A balance is shown as it comes, unless one read at a
    /// later slot already is.
    fn on_activity(&self, cluster: &Cluster, activity: Activity) {
        self.moved.saw(cluster, activity.slot());
        match activity {
            Activity::Transaction {
                address,
                signature,
                failed,
                slot,
            } => {
                debug!(target: SYNC, %address, %signature, failed, slot, "activity");
                self.moved.mark(address);
                self.scheduler.kick(BALANCES_JOB);
            }
            Activity::Balance {
                address,
                lamports,
                slot,
            } => {
                debug!(target: SYNC, %address, lamports, slot, "balance");
                let balances = self.moved.fresher(cluster, slot, vec![(address, lamports)]);
                if !balances.is_empty() {
                    apply_snapshot(
                        &self.core,
                        &ChainSnapshot {
                            balances,
                            ..ChainSnapshot::default()
                        },
                    );
                }
            }
        }
    }

    /// Wait until the cluster or the wallets' addresses change, or `wait`
    /// passes when one is given.
    async fn until_changed(
        &self,
        network: &mut tokio::sync::watch::Receiver<Arc<app_core::NetworkStatus>>,
        wallets: &mut tokio::sync::watch::Receiver<Arc<app_core::WalletsView>>,
        cluster: &Cluster,
        addresses: &[Pubkey],
        wait: Option<Duration>,
    ) -> Ended {
        let clock = self.core.ports().clock.clone();
        let timer = async move {
            match wait {
                Some(wait) => clock.sleep(wait).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(timer);
        loop {
            tokio::select! {
                () = &mut timer => return Ended::Lost,
                changed = network.changed() => {
                    if changed.is_err() {
                        return Ended::Closed;
                    }
                    if network.borrow_and_update().cluster != *cluster {
                        return Ended::Changed;
                    }
                }
                changed = wallets.changed() => {
                    if changed.is_err() {
                        return Ended::Closed;
                    }
                    if wallet_addresses(&wallets.borrow_and_update()) != addresses {
                        return Ended::Changed;
                    }
                }
            }
        }
    }

    fn feed_up(&self) {
        self.scheduler
            .set_interval(BALANCES_JOB, FEED_UP_BALANCES_INTERVAL);
    }

    fn feed_down(&self) {
        self.scheduler.set_interval(BALANCES_JOB, BALANCES_INTERVAL);
    }
}
