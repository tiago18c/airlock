//! Scheduler + balances job with a fake node and the manual clock.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use app_core::testing::{kit, settle};
use app_core::{notice_row_text, Clock, NoticeKind, VaultState};
use async_trait::async_trait;
use solana_hash::Hash;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use wallet_core::simulate::{SimulationReply, SimulationRequest};
use wallet_core::stake::{EpochInfo, StakeReward};
use wallet_core::{
    Cluster, DiscoveredTokenAccount, RawAccount, RpcError, SignatureStatus, SolanaRpc,
};
use wallet_flows::testing::{pk, FakeRpc};
use wallet_rpc::Activity;

use super::{
    read_after_landing, register_balances, register_tokens, start_activity_feed, ActivityEvents,
    ConnectFn, Moved, Scheduler, BALANCES_JOB, BEHIND_TRIES, BEHIND_WAIT, CURRENT_TOKENS_JOB,
    FIRST_RETRY, INITIAL_BACKOFF, MAX_BACKOFF, TOKENS_JOB,
};

fn add_contact(kit: &app_core::testing::Kit, pubkey: Pubkey) {
    kit.core.workspace_deferred("add_contact", |ws| {
        ws.book.insert_contact(pubkey, "Alice");
    });
}

/// Wraps [`FakeRpc`] and hangs on the first request until cancelled or released.
struct GateRpc {
    inner: FakeRpc,
    gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl GateRpc {
    fn hanging(inner: FakeRpc) -> (Arc<Self>, tokio::sync::oneshot::Sender<()>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (
            Arc::new(Self {
                inner,
                gate: Mutex::new(Some(rx)),
            }),
            tx,
        )
    }

    /// Hang once; later calls fail so a post-cancel retry cannot write.
    async fn wait_gate(&self) -> Result<(), RpcError> {
        let rx = self.gate.lock().unwrap().take();
        match rx {
            Some(rx) => {
                let _ = rx.await;
                Ok(())
            }
            None => Err(RpcError::Timeout),
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl SolanaRpc for GateRpc {
    async fn get_balance(&self, pubkey: &Pubkey) -> Result<u64, RpcError> {
        self.wait_gate().await?;
        self.inner.get_balance(pubkey).await
    }

    async fn get_account(&self, pubkey: &Pubkey) -> Result<Option<RawAccount>, RpcError> {
        self.wait_gate().await?;
        self.inner.get_account(pubkey).await
    }

    async fn get_lamports(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<u64>>, RpcError> {
        self.wait_gate().await?;
        self.inner.get_lamports(pubkeys).await
    }

    async fn get_token_accounts_by_owner(
        &self,
        owner: &Pubkey,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        self.wait_gate().await?;
        self.inner.get_token_accounts_by_owner(owner).await
    }

    async fn get_program_accounts(
        &self,
        program: &Pubkey,
        data_size: u64,
        offset: usize,
        address: &Pubkey,
    ) -> Result<Vec<(Pubkey, RawAccount)>, RpcError> {
        self.wait_gate().await?;
        self.inner
            .get_program_accounts(program, data_size, offset, address)
            .await
    }

    async fn get_latest_blockhash(&self) -> Result<Hash, RpcError> {
        self.wait_gate().await?;
        self.inner.get_latest_blockhash().await
    }

    async fn get_latest_blockhash_with_height(&self) -> Result<(Hash, u64), RpcError> {
        self.wait_gate().await?;
        self.inner.get_latest_blockhash_with_height().await
    }

    async fn get_block_height(&self) -> Result<u64, RpcError> {
        self.wait_gate().await?;
        self.inner.get_block_height().await
    }

    async fn is_blockhash_valid(&self, blockhash: &Hash) -> Result<bool, RpcError> {
        self.wait_gate().await?;
        self.inner.is_blockhash_valid(blockhash).await
    }

    async fn send_transaction(&self, wire: &[u8]) -> Result<Signature, RpcError> {
        self.wait_gate().await?;
        self.inner.send_transaction(wire).await
    }

    async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<SignatureStatus>>, RpcError> {
        self.wait_gate().await?;
        self.inner.get_signature_statuses(signatures).await
    }

    async fn simulate(&self, request: &SimulationRequest) -> Result<SimulationReply, RpcError> {
        self.wait_gate().await?;
        self.inner.simulate(request).await
    }

    async fn get_epoch_info(&self) -> Result<EpochInfo, RpcError> {
        self.wait_gate().await?;
        self.inner.get_epoch_info().await
    }

    async fn get_rent_exemption(&self, data_len: usize) -> Result<u64, RpcError> {
        self.wait_gate().await?;
        self.inner.get_rent_exemption(data_len).await
    }

    async fn get_stake_minimum_delegation(&self) -> Result<u64, RpcError> {
        self.wait_gate().await?;
        self.inner.get_stake_minimum_delegation().await
    }

    async fn get_stake_rewards(
        &self,
        accounts: &[Pubkey],
        epoch: u64,
    ) -> Result<Vec<StakeReward>, RpcError> {
        self.wait_gate().await?;
        self.inner.get_stake_rewards(accounts, epoch).await
    }
}

fn balances_scheduler(kit: &app_core::testing::Kit, rpc: Arc<dyn SolanaRpc>) -> Scheduler {
    let scheduler = Scheduler::new(kit.core.public());
    let rpc_c = rpc.clone();
    register_balances(
        &scheduler,
        Arc::new(move || Ok(rpc_c.clone())),
        Moved::default(),
    );
    scheduler.start();
    scheduler
}

#[tokio::test]
async fn backoff_doubles_and_resets() {
    let kit = kit();
    add_contact(&kit, pk(1));

    let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let fail_c = fail.clone();
    // Dynamic offline flag via a thin wrapper.
    let rpc: Arc<dyn SolanaRpc> = Arc::new(FailToggleRpc {
        fail: fail_c,
        inner: FakeRpc::default().with_balance(pk(1), 42),
    });

    let scheduler = balances_scheduler(&kit, rpc);
    settle().await;

    scheduler.kick(BALANCES_JOB);
    settle().await;

    let mut expected = INITIAL_BACKOFF;
    for _ in 0..9 {
        let job = kit
            .core
            .slices()
            .sync
            .get()
            .job(BALANCES_JOB)
            .unwrap()
            .clone();
        assert!(job.last_error.is_some(), "expected error while node fails");
        let wait_ms = job.next_run_ms.unwrap().saturating_sub(kit.clock.now_ms());
        assert_eq!(wait_ms, expected.as_millis() as u64);
        kit.clock.advance(expected);
        settle().await;
        expected = (expected.saturating_mul(2)).min(MAX_BACKOFF);
    }

    let wait_ms = kit
        .core
        .slices()
        .sync
        .get()
        .job(BALANCES_JOB)
        .unwrap()
        .next_run_ms
        .unwrap()
        .saturating_sub(kit.clock.now_ms());
    assert_eq!(wait_ms, 300_000);

    fail.store(false, Ordering::SeqCst);
    kit.clock.advance(MAX_BACKOFF);
    settle().await;

    let job = kit
        .core
        .slices()
        .sync
        .get()
        .job(BALANCES_JOB)
        .unwrap()
        .clone();
    assert!(job.last_error.is_none());
    assert!(job.last_success_ms.is_some());
    assert_eq!(
        job.next_run_ms.unwrap().saturating_sub(kit.clock.now_ms()),
        30_000
    );
    assert_eq!(
        kit.core.slices().book.get().get(&pk(1)).unwrap().lamports,
        Some(42)
    );
}

#[tokio::test]
async fn sync_failed_notice_once_until_success() {
    let kit = kit();
    add_contact(&kit, pk(1));

    let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let fail_c = fail.clone();
    let rpc: Arc<dyn SolanaRpc> = Arc::new(FailToggleRpc {
        fail: fail_c,
        inner: FakeRpc::default().with_balance(pk(1), 7),
    });

    let scheduler = balances_scheduler(&kit, rpc);
    settle().await;
    scheduler.kick(BALANCES_JOB);
    settle().await;

    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), 1);
    match &notices.items()[0].kind {
        NoticeKind::SyncFailed { job, error } => {
            assert_eq!(job, BALANCES_JOB);
            assert!(!error.is_empty());
            assert!(notice_row_text(&notices.items()[0]).contains("Sync failed · balances:"));
        }
        other => panic!("expected SyncFailed, got {other:?}"),
    }

    // More failures before success must not push another notice.
    let wait = kit
        .core
        .slices()
        .sync
        .get()
        .job(BALANCES_JOB)
        .unwrap()
        .next_run_ms
        .unwrap()
        .saturating_sub(kit.clock.now_ms());
    kit.clock.advance(std::time::Duration::from_millis(wait));
    settle().await;
    assert_eq!(kit.core.slices().notices.get().len(), 1);

    fail.store(false, Ordering::SeqCst);
    let wait = kit
        .core
        .slices()
        .sync
        .get()
        .job(BALANCES_JOB)
        .unwrap()
        .next_run_ms
        .unwrap()
        .saturating_sub(kit.clock.now_ms());
    kit.clock.advance(std::time::Duration::from_millis(wait));
    settle().await;
    assert!(kit
        .core
        .slices()
        .sync
        .get()
        .job(BALANCES_JOB)
        .unwrap()
        .last_error
        .is_none());

    // After success, a new failure may push again.
    fail.store(true, Ordering::SeqCst);
    scheduler.kick(BALANCES_JOB);
    settle().await;
    assert_eq!(kit.core.slices().notices.get().len(), 2);
}

struct FailToggleRpc {
    fail: Arc<std::sync::atomic::AtomicBool>,
    inner: FakeRpc,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl SolanaRpc for FailToggleRpc {
    async fn get_balance(&self, pubkey: &Pubkey) -> Result<u64, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_balance(pubkey).await
    }

    async fn get_account(&self, pubkey: &Pubkey) -> Result<Option<RawAccount>, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_account(pubkey).await
    }

    async fn get_lamports(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<u64>>, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_lamports(pubkeys).await
    }

    async fn get_token_accounts_by_owner(
        &self,
        owner: &Pubkey,
    ) -> Result<Vec<DiscoveredTokenAccount>, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_token_accounts_by_owner(owner).await
    }

    async fn get_program_accounts(
        &self,
        program: &Pubkey,
        data_size: u64,
        offset: usize,
        address: &Pubkey,
    ) -> Result<Vec<(Pubkey, RawAccount)>, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner
            .get_program_accounts(program, data_size, offset, address)
            .await
    }

    async fn get_latest_blockhash(&self) -> Result<Hash, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_latest_blockhash().await
    }

    async fn get_latest_blockhash_with_height(&self) -> Result<(Hash, u64), RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_latest_blockhash_with_height().await
    }

    async fn get_block_height(&self) -> Result<u64, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_block_height().await
    }

    async fn is_blockhash_valid(&self, blockhash: &Hash) -> Result<bool, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.is_blockhash_valid(blockhash).await
    }

    async fn send_transaction(&self, wire: &[u8]) -> Result<Signature, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.send_transaction(wire).await
    }

    async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<SignatureStatus>>, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_signature_statuses(signatures).await
    }

    async fn simulate(&self, request: &SimulationRequest) -> Result<SimulationReply, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.simulate(request).await
    }

    async fn get_epoch_info(&self) -> Result<EpochInfo, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_epoch_info().await
    }

    async fn get_rent_exemption(&self, data_len: usize) -> Result<u64, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_rent_exemption(data_len).await
    }

    async fn get_stake_minimum_delegation(&self) -> Result<u64, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_stake_minimum_delegation().await
    }

    async fn get_stake_rewards(
        &self,
        accounts: &[Pubkey],
        epoch: u64,
    ) -> Result<Vec<StakeReward>, RpcError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RpcError::Timeout);
        }
        self.inner.get_stake_rewards(accounts, epoch).await
    }
}

#[tokio::test]
async fn cluster_switch_cancels_run() {
    let kit = kit();
    add_contact(&kit, pk(1));

    let (rpc, _release) = GateRpc::hanging(FakeRpc::default().with_balance(pk(1), 99));
    let scheduler = balances_scheduler(&kit, rpc);
    settle().await;

    let scheduler_c = scheduler.clone();
    let blocked = tokio::spawn(async move { scheduler_c.run_now(BALANCES_JOB).await });
    settle().await;

    kit.core.workspace_deferred("switch", |ws| {
        ws.use_cluster(Cluster::MainnetBeta);
    });
    settle().await;

    let result = blocked.await.unwrap();
    assert!(result.is_err());
    // Nothing the cancelled run fetched was written.
    assert_eq!(
        kit.core.slices().book.get().get(&pk(1)).unwrap().lamports,
        None
    );
}

#[tokio::test]
async fn runs_while_locked() {
    let kit = kit();
    add_contact(&kit, pk(1));
    assert_ne!(*kit.core.slices().vault.get(), VaultState::Unlocked);

    let rpc: Arc<dyn SolanaRpc> = Arc::new(FakeRpc::default().with_balance(pk(1), 7));
    let scheduler = balances_scheduler(&kit, rpc);
    settle().await;

    let result = scheduler.run_now(BALANCES_JOB).await;
    settle().await;
    assert!(result.is_ok());
    assert_eq!(
        kit.core.slices().book.get().get(&pk(1)).unwrap().lamports,
        Some(7)
    );
}

#[tokio::test]
async fn run_now_skips_wait() {
    let kit = kit();
    add_contact(&kit, pk(1));

    let rpc: Arc<dyn SolanaRpc> = Arc::new(FakeRpc::default().with_balance(pk(1), 3));
    let scheduler = balances_scheduler(&kit, rpc);
    settle().await;

    // Interval is 30s; without advancing the clock, run_now still completes.
    let result = scheduler.run_now(BALANCES_JOB).await.unwrap();
    assert_eq!(
        result.note.as_deref(),
        Some("Refreshed 1 owners, 0 token accounts.")
    );
    assert_eq!(
        kit.core.slices().book.get().get(&pk(1)).unwrap().lamports,
        Some(3)
    );
}

#[tokio::test]
async fn run_now_survives_a_network_change_before_it_starts() {
    // `select!` picks among ready branches at random, so repeat the race.
    for _ in 0..20 {
        let kit = kit();
        add_contact(&kit, pk(1));

        let rpc: Arc<dyn SolanaRpc> = Arc::new(FakeRpc::default().with_balance(pk(1), 5));
        let scheduler = balances_scheduler(&kit, rpc);
        settle().await;

        let scheduler_c = scheduler.clone();
        let asked = tokio::spawn(async move { scheduler_c.run_now(BALANCES_JOB).await });
        kit.core.workspace_deferred("switch", |ws| {
            ws.use_cluster(Cluster::MainnetBeta);
        });
        settle().await;

        let result = asked.await.unwrap();
        assert!(result.is_ok(), "{:?}", result.err());
    }
}

fn add_wallet(kit: &app_core::testing::Kit, pubkey: Pubkey) {
    kit.core
        .enroll(
            "add_wallet",
            |data| data.add_offline_wallet("Wallet", pubkey, None),
            |book, _| wallet_core::name_offline_wallet(book, pubkey, "Wallet"),
        )
        .unwrap();
}

fn wait_of(kit: &app_core::testing::Kit, job: &str) -> u64 {
    kit.core
        .slices()
        .sync
        .get()
        .job(job)
        .unwrap()
        .next_run_ms
        .unwrap()
        .saturating_sub(kit.clock.now_ms())
}

type Opened = Arc<
    Mutex<
        Vec<(
            String,
            Vec<Pubkey>,
            futures::channel::mpsc::UnboundedSender<Activity>,
        )>,
    >,
>;

/// A WebSocket that opens at once and passes on what the test sends.
fn fake_feed() -> (ConnectFn, Opened) {
    let opened: Opened = Arc::default();
    let kept = opened.clone();
    let connect: ConnectFn = Arc::new(move |url, addresses| {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        kept.lock().unwrap().push((url, addresses, tx));
        Box::pin(async move { Ok(Box::pin(rx) as ActivityEvents) })
    });
    (connect, opened)
}

fn activity(address: Pubkey) -> Activity {
    landed(address, 0)
}

fn landed(address: Pubkey, slot: u64) -> Activity {
    Activity::Transaction {
        address,
        signature: Signature::from([7; 64]),
        failed: false,
        slot,
    }
}

fn lamports_of(kit: &app_core::testing::Kit, address: Pubkey) -> Option<u64> {
    kit.core.slices().book.get().get(&address).unwrap().lamports
}

/// One wallet at `pk(1)`, the balances job over `rpc` and the feed over a
/// fake WebSocket.
fn feed_kit(rpc: Arc<FakeRpc>) -> (app_core::testing::Kit, Scheduler, Moved, Opened) {
    let kit = kit();
    kit.core.create_vault("password").unwrap();
    add_wallet(&kit, pk(1));
    let scheduler = Scheduler::new(kit.core.public());
    let moved = Moved::default();
    let shared: Arc<dyn SolanaRpc> = rpc;
    register_balances(
        &scheduler,
        Arc::new(move || Ok(shared.clone())),
        moved.clone(),
    );
    scheduler.start();
    let (connect, opened) = fake_feed();
    start_activity_feed(kit.core.public(), scheduler.clone(), moved.clone(), connect);
    (kit, scheduler, moved, opened)
}

fn push(opened: &Opened, activity: Activity) {
    opened.lock().unwrap()[0]
        .2
        .unbounded_send(activity)
        .unwrap();
}

#[tokio::test]
async fn a_pushed_balance_is_shown_without_asking_the_node() {
    let rpc = Arc::new(FakeRpc::default());
    let (kit, _scheduler, _moved, opened) = feed_kit(rpc.clone());
    settle().await;

    push(
        &opened,
        Activity::Balance {
            address: pk(1),
            lamports: 42,
            slot: 10,
        },
    );
    settle().await;
    assert_eq!(lamports_of(&kit, pk(1)), Some(42));
    assert_eq!(rpc.asked(), 0);
}

#[tokio::test]
async fn a_read_waits_for_a_node_that_has_seen_the_transaction() {
    let rpc = Arc::new(FakeRpc::default().with_balance(pk(1), 5));
    rpc.slot.store(9, Ordering::Relaxed);
    let (kit, _scheduler, _moved, opened) = feed_kit(rpc.clone());
    settle().await;

    push(&opened, landed(pk(1), 10));
    settle().await;
    assert_eq!(*rpc.since.lock().unwrap(), vec![10], "the node is behind");
    assert_eq!(lamports_of(&kit, pk(1)), None);

    rpc.slot.store(10, Ordering::Relaxed);
    kit.clock.advance(BEHIND_WAIT);
    settle().await;
    assert_eq!(lamports_of(&kit, pk(1)), Some(5));
    assert_eq!(
        *rpc.since.lock().unwrap(),
        vec![10, 10, 10],
        "balances, then the wallet's tokens, from a node that has seen it"
    );
}

#[tokio::test]
async fn a_read_older_than_a_pushed_balance_does_not_hide_it() {
    let rpc = Arc::new(FakeRpc::default().with_balance(pk(1), 5));
    rpc.slot.store(8, Ordering::Relaxed);
    let (kit, _scheduler, _moved, opened) = feed_kit(rpc.clone());
    settle().await;

    push(
        &opened,
        Activity::Balance {
            address: pk(1),
            lamports: 42,
            slot: 10,
        },
    );
    push(&opened, landed(pk(1), 10));
    settle().await;
    for _ in 0..BEHIND_TRIES {
        kit.clock.advance(BEHIND_WAIT);
        settle().await;
    }
    let asked = rpc.since.lock().unwrap().clone();
    assert_eq!(asked[..BEHIND_TRIES as usize], [10; BEHIND_TRIES as usize]);
    assert_eq!(
        asked[BEHIND_TRIES as usize], 0,
        "a node that stays behind is read from as it is"
    );
    assert_eq!(
        lamports_of(&kit, pk(1)),
        Some(42),
        "what it read at slot 8 is older than the balance pushed at 10"
    );
}

#[tokio::test]
async fn a_landed_transaction_reads_the_wallet_in_use_on_its_cluster() {
    let rpc = Arc::new(FakeRpc::default().with_balance(pk(1), 5));
    let (kit, scheduler, moved, _opened) = feed_kit(rpc.clone());
    settle().await;
    let on_landed = read_after_landing(kit.core.public(), scheduler, moved);

    on_landed(&Cluster::MainnetBeta, Some(12));
    settle().await;
    assert_eq!(rpc.asked(), 0, "another cluster's transaction");

    on_landed(&Cluster::Devnet, Some(12));
    settle().await;
    assert_eq!(lamports_of(&kit, pk(1)), Some(5));
    assert_eq!(
        *rpc.since.lock().unwrap(),
        vec![12, 12],
        "balances, then the tokens of the wallet in use"
    );
}

#[tokio::test]
async fn a_transaction_on_the_feed_reads_balances_and_that_wallets_tokens() {
    let kit = kit();
    kit.core.create_vault("password").unwrap();
    add_wallet(&kit, pk(1));
    add_contact(&kit, pk(2));

    let rpc = Arc::new(FakeRpc::default().with_balance(pk(1), 5));
    let scheduler = Scheduler::new(kit.core.public());
    let moved = Moved::default();
    let shared: Arc<dyn SolanaRpc> = rpc.clone();
    register_balances(
        &scheduler,
        Arc::new(move || Ok(shared.clone())),
        moved.clone(),
    );
    scheduler.start();
    let (connect, opened) = fake_feed();
    start_activity_feed(kit.core.public(), scheduler.clone(), moved, connect);
    settle().await;

    {
        let opened = opened.lock().unwrap();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].0, Cluster::Devnet.ws_url());
        assert_eq!(opened[0].1, vec![pk(1)], "only wallets are watched");
    }
    // While the feed is up, balances are read only now and then.
    assert_eq!(wait_of(&kit, BALANCES_JOB), 300_000);
    assert_eq!(rpc.asked(), 0);

    opened.lock().unwrap()[0]
        .2
        .unbounded_send(activity(pk(1)))
        .unwrap();
    settle().await;
    // One request for every balance, one for the moved wallet's tokens.
    assert_eq!(rpc.asked(), 2);
    assert_eq!(
        kit.core.slices().book.get().get(&pk(1)).unwrap().lamports,
        Some(5)
    );
}

#[tokio::test]
async fn a_dropped_feed_falls_back_to_polling_and_catches_up_when_back() {
    let kit = kit();
    kit.core.create_vault("password").unwrap();
    add_wallet(&kit, pk(1));

    let rpc = Arc::new(FakeRpc::default());
    let scheduler = Scheduler::new(kit.core.public());
    let moved = Moved::default();
    let shared: Arc<dyn SolanaRpc> = rpc.clone();
    register_balances(
        &scheduler,
        Arc::new(move || Ok(shared.clone())),
        moved.clone(),
    );
    scheduler.start();
    let (connect, opened) = fake_feed();
    start_activity_feed(kit.core.public(), scheduler.clone(), moved, connect);
    settle().await;
    assert_eq!(wait_of(&kit, BALANCES_JOB), 300_000);

    // The connection drops.
    opened.lock().unwrap().clear();
    settle().await;
    assert_eq!(wait_of(&kit, BALANCES_JOB), 30_000);
    assert_eq!(rpc.asked(), 0);

    kit.clock.advance(FIRST_RETRY);
    settle().await;
    assert_eq!(opened.lock().unwrap().len(), 1, "connected again");
    assert_eq!(wait_of(&kit, BALANCES_JOB), 300_000);
    assert_eq!(rpc.asked(), 1, "balances read once for what was missed");
}

#[tokio::test]
async fn the_feed_follows_the_wallets() {
    let kit = kit();
    kit.core.create_vault("password").unwrap();

    let scheduler = Scheduler::new(kit.core.public());
    let rpc: Arc<dyn SolanaRpc> = Arc::new(FakeRpc::default());
    register_balances(
        &scheduler,
        Arc::new(move || Ok(rpc.clone())),
        Moved::default(),
    );
    scheduler.start();
    let (connect, opened) = fake_feed();
    start_activity_feed(
        kit.core.public(),
        scheduler.clone(),
        Moved::default(),
        connect,
    );
    settle().await;
    assert!(
        opened.lock().unwrap().is_empty(),
        "no wallet, no connection"
    );

    add_wallet(&kit, pk(1));
    settle().await;
    add_wallet(&kit, pk(2));
    settle().await;
    let opened = opened.lock().unwrap();
    assert_eq!(opened.len(), 2);
    assert_eq!(opened[1].1, vec![pk(1), pk(2)]);
    assert!(opened[0].2.is_closed(), "the old connection was dropped");
}

#[tokio::test]
async fn tokens_are_read_for_the_wallet_in_use_apart_from_the_rest() {
    let kit = kit();
    kit.core.create_vault("password").unwrap();
    add_wallet(&kit, pk(1));
    add_wallet(&kit, pk(2));
    add_contact(&kit, pk(3));

    let rpc = Arc::new(FakeRpc::default());
    let scheduler = Scheduler::new(kit.core.public());
    let shared: Arc<dyn SolanaRpc> = rpc.clone();
    register_tokens(&scheduler, Arc::new(move || Ok(shared.clone())));
    scheduler.start();
    settle().await;
    assert_eq!(wait_of(&kit, CURRENT_TOKENS_JOB), 15_000);
    assert_eq!(wait_of(&kit, TOKENS_JOB), 30_000);

    scheduler.run_now(CURRENT_TOKENS_JOB).await.unwrap();
    assert_eq!(rpc.asked(), 1, "the first wallet is the one in use");
    scheduler.run_now(TOKENS_JOB).await.unwrap();
    assert_eq!(rpc.asked(), 3, "the other wallet and the contact");
}

/// Prices every mint asked at one dollar.
struct DollarSource;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl wallet_core::portfolio::MarketSource for DollarSource {
    async fn quotes(
        &self,
        mints: &[Pubkey],
    ) -> Result<
        Vec<(Pubkey, Option<wallet_core::portfolio::MarketQuote>)>,
        wallet_core::portfolio::MarketError,
    > {
        Ok(mints
            .iter()
            .map(|mint| {
                (
                    *mint,
                    Some(wallet_core::portfolio::MarketQuote {
                        price_usd: Some(1.0),
                        ..Default::default()
                    }),
                )
            })
            .collect())
    }

    async fn groups(
        &self,
    ) -> Result<
        Vec<(Pubkey, wallet_core::portfolio::AssetGroup)>,
        wallet_core::portfolio::MarketError,
    > {
        Ok(Vec::new())
    }

    async fn check(
        &self,
        _mint: &Pubkey,
    ) -> Result<wallet_core::portfolio::TokenCheck, wallet_core::portfolio::MarketError> {
        Ok(Default::default())
    }
}

fn prices_kit(cluster: Cluster, watched: bool) -> app_core::testing::Kit {
    let kit = kit();
    kit.core.workspace_deferred("setup", |ws| {
        ws.cluster = cluster;
        ws.tokens_api_key = Some("test-key".into());
        ws.book.insert_contact(pk(1), "Alice");
        ws.book.get_mut(&pk(1)).unwrap().watched = watched;
    });
    kit.core.holdings_deferred("setup", |holdings| {
        holdings.replace_owners(
            &[pk(1)],
            &[DiscoveredTokenAccount {
                pubkey: pk(2),
                owner: pk(1),
                mint: pk(9),
                amount: 5,
                decimals: 0,
            }],
        );
    });
    kit
}

async fn run_prices_once(kit: &app_core::testing::Kit) -> super::JobResult {
    let scheduler = Scheduler::new(kit.core.public());
    super::register_prices_with(&scheduler, Arc::new(|_key| Arc::new(DollarSource)));
    scheduler.start();
    settle().await;
    scheduler.run_now(super::PRICES_JOB).await
}

#[tokio::test]
async fn prices_land_in_the_holdings_cache_for_watched_addresses() {
    let kit = prices_kit(Cluster::MainnetBeta, true);
    run_prices_once(&kit).await.unwrap();
    settle().await;
    let holdings = kit.core.slices().holdings.get();
    assert!(holdings.market.quote(&pk(9)).is_some());
    assert!(holdings
        .market
        .quote(&wallet_core::portfolio::WRAPPED_SOL_MINT)
        .is_some());
}

#[tokio::test]
async fn an_address_that_is_not_watched_sends_no_mint() {
    let kit = prices_kit(Cluster::MainnetBeta, false);
    run_prices_once(&kit).await.unwrap();
    settle().await;
    let holdings = kit.core.slices().holdings.get();
    assert!(holdings.market.quote(&pk(9)).is_none());
}

#[tokio::test]
async fn prices_wait_for_mainnet() {
    let kit = prices_kit(Cluster::Devnet, true);
    let ok = run_prices_once(&kit).await.unwrap();
    assert!(ok.note.unwrap().contains("mainnet"));
    assert!(kit.core.slices().holdings.get().market.quotes.is_empty());
}
