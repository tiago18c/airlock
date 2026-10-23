//! Jobs with an interval, exponential backoff, cancellation and "run now".

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use app_core::{BoxFuture, JobSyncStatus, PublicCore, SyncStatus};
use tokio::sync::{oneshot, watch};
use tracing::{debug, info, info_span, warn, Instrument};

const SYNC: &str = "airlock::sync";

/// Job id for the SOL balances of every address read, and the token
/// accounts of an address the activity feed saw move.
pub const BALANCES_JOB: &str = "balances";

/// Job id for the token accounts of the wallet in use.
pub const CURRENT_TOKENS_JOB: &str = "tokens-current";

/// Job id for the token accounts of every other address read.
pub const TOKENS_JOB: &str = "tokens";

/// First wait after a failure before retrying.
pub const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Cap on the doubled backoff.
pub const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Why a run started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Schedule,
    User,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::User => "user",
        }
    }
}

/// What a job returns to the scheduler and to a waiting `run_now`.
pub type JobResult = Result<JobOk, String>;

/// Success payload: optional note for a user-triggered run (Refresh).
#[derive(Clone, Debug, Default)]
pub struct JobOk {
    pub note: Option<String>,
}

/// Async work a job performs for one run. The future is `Send` on native
/// only, so a job can await the wasm RPC client.
pub type JobFn = Arc<dyn Fn(JobCtx) -> BoxFuture<JobResult> + Send + Sync>;

/// Context passed into a job run.
#[derive(Clone)]
pub struct JobCtx {
    pub core: PublicCore,
    pub trigger: Trigger,
    /// Bumped when this run must stop without writing.
    cancel: watch::Receiver<u64>,
    epoch: u64,
}

impl JobCtx {
    /// True when a cluster / RPC / offline change cancelled this run.
    pub fn is_cancelled(&self) -> bool {
        *self.cancel.borrow() != self.epoch
    }

    /// Wait until `fut` completes or this run is cancelled.
    pub async fn run_cancellable<T, F>(&self, fut: F) -> Result<T, Cancelled>
    where
        F: Future<Output = T>,
    {
        let mut cancel = self.cancel.clone();
        let epoch = self.epoch;
        tokio::pin!(fut);
        loop {
            tokio::select! {
                value = &mut fut => return Ok(value),
                result = cancel.changed() => {
                    if result.is_err() || *cancel.borrow() != epoch {
                        return Err(Cancelled);
                    }
                }
            }
        }
    }
}

/// The run was stopped by a cluster / RPC / offline change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

/// Shared handle that registers jobs, kicks them and watches the network.
#[derive(Clone)]
pub struct Scheduler {
    core: PublicCore,
    inner: Arc<Inner>,
}

struct Inner {
    /// Bumped to cancel every in-flight run and restart waits.
    epoch: watch::Sender<u64>,
    jobs: Mutex<HashMap<&'static str, JobEntry>>,
    /// Monotonic run id for `sync_job` spans.
    runs: AtomicU64,
}

struct JobEntry {
    /// How long to wait between runs. Changing it restarts the wait.
    interval: watch::Sender<Duration>,
    work: JobFn,
    /// Bumped to wake a waiting loop for an immediate run.
    kick: watch::Sender<Kick>,
    /// Latest status row for this job.
    status: JobSyncStatus,
    /// True after a SyncFailed notice until the job succeeds again.
    failed_notice: bool,
}

#[derive(Clone, Debug)]
struct Kick {
    trigger: Trigger,
    /// Filled when a caller awaits `run_now`.
    reply: Option<Arc<Mutex<Option<oneshot::Sender<JobResult>>>>>,
}

impl Scheduler {
    pub fn new(core: PublicCore) -> Self {
        let (epoch, _) = watch::channel(0u64);
        Self {
            core,
            inner: Arc::new(Inner {
                epoch,
                jobs: Mutex::new(HashMap::new()),
                runs: AtomicU64::new(0),
            }),
        }
    }

    /// Register `name` and start its loop on the core's spawner.
    pub fn register(&self, name: &'static str, interval: Duration, work: JobFn) {
        let (kick, _) = watch::channel(Kick {
            trigger: Trigger::Schedule,
            reply: None,
        });
        let (interval, _) = watch::channel(interval);
        let status = JobSyncStatus {
            job: name.to_string(),
            last_success_ms: None,
            last_error: None,
            next_run_ms: None,
        };
        {
            let mut jobs = self.jobs();
            if jobs.contains_key(name) {
                return;
            }
            jobs.insert(
                name,
                JobEntry {
                    interval,
                    work,
                    kick,
                    status,
                    failed_notice: false,
                },
            );
        }
        self.publish_status();
        let scheduler = self.clone();
        self.core.ports().spawner.spawn(Box::pin(async move {
            scheduler.job_loop(name).await;
        }));
    }

    /// Start watching the cluster: another cluster or RPC URL cancels runs.
    pub fn start(&self) {
        let scheduler = self.clone();
        self.core.ports().spawner.spawn(Box::pin(async move {
            scheduler.watch_network().await;
        }));
    }

    /// Run `name` as soon as possible. Waits for that run's result.
    pub async fn run_now(&self, name: &'static str) -> JobResult {
        let (tx, rx) = oneshot::channel();
        {
            let jobs = self.jobs();
            let Some(entry) = jobs.get(name) else {
                return Err(format!("unknown sync job: {name}"));
            };
            entry.kick.send_replace(Kick {
                trigger: Trigger::User,
                reply: Some(Arc::new(Mutex::new(Some(tx)))),
            });
        }
        match rx.await {
            Ok(result) => result,
            Err(_) => Err("sync job was cancelled.".into()),
        }
    }

    /// Fire-and-forget kick (post-probe refresh, wallet added).
    pub fn kick(&self, name: &'static str) {
        let jobs = self.jobs();
        let Some(entry) = jobs.get(name) else {
            return;
        };
        entry.kick.send_replace(Kick {
            trigger: Trigger::Schedule,
            reply: None,
        });
    }

    /// Kick every job, as after the node answered a probe.
    pub fn kick_all(&self) {
        let jobs = self.jobs();
        for entry in jobs.values() {
            entry.kick.send_replace(Kick {
                trigger: Trigger::Schedule,
                reply: None,
            });
        }
    }

    /// Wait `interval` between runs of `name` from now on. A wait already
    /// under way starts again with it. Nothing runs because of the change.
    pub fn set_interval(&self, name: &'static str, interval: Duration) {
        let jobs = self.jobs();
        if let Some(entry) = jobs.get(name) {
            entry.interval.send_if_modified(|current| {
                let changed = *current != interval;
                *current = interval;
                changed
            });
        }
    }

    fn jobs(&self) -> std::sync::MutexGuard<'_, HashMap<&'static str, JobEntry>> {
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn publish_status(&self) {
        let jobs = self.jobs();
        let mut rows: Vec<JobSyncStatus> = jobs.values().map(|e| e.status.clone()).collect();
        rows.sort_by(|a, b| a.job.cmp(&b.job));
        self.core.set_sync_status(SyncStatus { jobs: rows });
    }

    fn update_status(&self, name: &str, f: impl FnOnce(&mut JobSyncStatus)) {
        {
            let mut jobs = self.jobs();
            if let Some(entry) = jobs.get_mut(name) {
                f(&mut entry.status);
            }
        }
        self.publish_status();
    }

    async fn watch_network(&self) {
        let mut net = self.core.slices().network.subscribe();
        let mut last = net.borrow().cluster.clone();
        loop {
            if net.changed().await.is_err() {
                return;
            }
            let cluster = net.borrow_and_update().cluster.clone();
            if cluster != last {
                debug!(
                    target: SYNC,
                    cluster = %cluster.name(),
                    "sync cancel (cluster change)"
                );
                let next = self.inner.epoch.borrow().saturating_add(1);
                self.inner.epoch.send_replace(next);
                last = cluster;
            }
        }
    }

    async fn job_loop(&self, name: &'static str) {
        let mut epoch_rx = self.inner.epoch.subscribe();
        let (mut interval_rx, mut kick_rx) = {
            let jobs = self.jobs();
            let entry = jobs.get(name).expect("job registered");
            (entry.interval.subscribe(), entry.kick.subscribe())
        };
        // Drain the initial kick value so the first wait is not a false wake.
        let _ = kick_rx.borrow_and_update();

        let mut backoff = INITIAL_BACKOFF;
        let mut pending: Option<Kick> = None;

        loop {
            let epoch = *epoch_rx.borrow_and_update();

            let interval = *interval_rx.borrow_and_update();
            if pending.is_none() {
                let now = self.core.ports().clock.now_ms();
                self.update_status(name, |s| {
                    s.next_run_ms = Some(now.saturating_add(interval.as_millis() as u64));
                });
                let wait = interval;
                tokio::select! {
                    _ = self.core.ports().clock.sleep(wait) => {
                        pending = Some(Kick {
                            trigger: Trigger::Schedule,
                            reply: None,
                        });
                    }
                    result = kick_rx.changed() => {
                        if result.is_err() {
                            return;
                        }
                        pending = Some(kick_rx.borrow_and_update().clone());
                    }
                    result = epoch_rx.changed() => {
                        if result.is_err() {
                            return;
                        }
                        if *epoch_rx.borrow() != epoch {
                            continue;
                        }
                    }
                    result = interval_rx.changed() => {
                        if result.is_err() {
                            return;
                        }
                        continue;
                    }
                }
            }

            if *epoch_rx.borrow() != epoch {
                // The network changed before this run started. It has fetched
                // nothing yet, so it keeps its place and runs on the new one.
                continue;
            }

            let kick = pending.take().unwrap_or(Kick {
                trigger: Trigger::Schedule,
                reply: None,
            });
            let run = self.inner.runs.fetch_add(1, Ordering::SeqCst) + 1;
            let ctx = JobCtx {
                core: self.core.clone(),
                trigger: kick.trigger,
                cancel: epoch_rx.clone(),
                epoch: *epoch_rx.borrow(),
            };
            let work = {
                let jobs = self.jobs();
                jobs.get(name).expect("job registered").work.clone()
            };

            let span = info_span!(
                target: SYNC,
                "sync_job",
                job = name,
                run,
                trigger = kick.trigger.as_str(),
            );
            let result = async {
                info!(target: SYNC, job = name, run, "job start");
                let outcome = work(ctx).await;
                match &outcome {
                    Ok(_) => info!(target: SYNC, job = name, run, "job ok"),
                    Err(err) => warn!(target: SYNC, job = name, run, error = %err, "job failed"),
                }
                outcome
            }
            .instrument(span)
            .await;

            // A cancel during the run: do not record success/error from a stale epoch.
            if *epoch_rx.borrow() != epoch {
                reply_cancelled(kick.reply);
                continue;
            }

            let now = self.core.ports().clock.now_ms();
            match &result {
                Ok(_) => {
                    backoff = INITIAL_BACKOFF;
                    let next = now.saturating_add(interval.as_millis() as u64);
                    self.update_status(name, |s| {
                        s.last_success_ms = Some(now);
                        s.last_error = None;
                        s.next_run_ms = Some(next);
                    });
                    {
                        let mut jobs = self.jobs();
                        if let Some(entry) = jobs.get_mut(name) {
                            entry.failed_notice = false;
                        }
                    }
                    if let Some(reply) = kick.reply {
                        if let Some(tx) =
                            reply.lock().unwrap_or_else(PoisonError::into_inner).take()
                        {
                            let _ = tx.send(result);
                        }
                    }
                }
                Err(err) => {
                    let wait = backoff;
                    let next = now.saturating_add(wait.as_millis() as u64);
                    self.update_status(name, |s| {
                        s.last_error = Some(err.clone());
                        s.next_run_ms = Some(next);
                    });
                    let push_notice = {
                        let mut jobs = self.jobs();
                        match jobs.get_mut(name) {
                            Some(entry) if !entry.failed_notice => {
                                entry.failed_notice = true;
                                true
                            }
                            _ => false,
                        }
                    };
                    if push_notice {
                        self.core.report_sync_failed(name, err);
                    }
                    if let Some(reply) = kick.reply {
                        if let Some(tx) =
                            reply.lock().unwrap_or_else(PoisonError::into_inner).take()
                        {
                            let _ = tx.send(result);
                        }
                    }
                    // Backoff wait, then retry (unless kicked or cancelled).
                    tokio::select! {
                        _ = self.core.ports().clock.sleep(wait) => {
                            pending = Some(Kick {
                                trigger: Trigger::Schedule,
                                reply: None,
                            });
                        }
                        result = kick_rx.changed() => {
                            if result.is_ok() {
                                pending = Some(kick_rx.borrow_and_update().clone());
                            }
                        }
                        result = epoch_rx.changed() => {
                            if result.is_err() {
                                return;
                            }
                        }
                    }
                    backoff = (backoff.saturating_mul(2)).min(MAX_BACKOFF);
                }
            }
        }
    }
}

fn reply_cancelled(reply: Option<Arc<Mutex<Option<oneshot::Sender<JobResult>>>>>) {
    let Some(reply) = reply else {
        return;
    };
    let tx = reply.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(tx) = tx {
        let _ = tx.send(Err("sync job was cancelled.".into()));
    }
}
