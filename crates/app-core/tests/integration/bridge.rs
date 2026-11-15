//! The browser request queue: one queue in the core, answered by the window,
//! expired by the transport.

use std::sync::mpsc;

use app_core::bridge::{
    BridgeAnswer, BridgeRefused, BridgeReply, BridgeRequest, SiteCall, SiteTransaction, SiteWire,
};
use app_core::testing::{kit, kit_on, settle, MemoryStorage};
use app_core::{BridgeRequestOutcome, Event, NoticeKind};
use serde_json::Value;

fn unsigned() -> SiteTransaction {
    SiteTransaction {
        account: solana_pubkey::Pubkey::new_from_array([1; 32]),
        wire: SiteWire::Unreadable(1),
        chain: None,
        commitment: None,
    }
}

fn request(id: &str, method: &str) -> BridgeRequest {
    let call = match method {
        "connect" => SiteCall::Connect,
        "signIn" => SiteCall::SignIn(Default::default()),
        "signMessage" => SiteCall::SignMessage {
            account: solana_pubkey::Pubkey::new_from_array([1; 32]),
            message: vec![1],
        },
        "signTransaction" => SiteCall::SignTransactions(vec![unsigned()]),
        other => panic!("no such request: {other}"),
    };
    BridgeRequest {
        id: id.into(),
        origin: "https://example.org".into(),
        call,
    }
}

/// A reply that hands its bytes to the returned receiver.
fn reply() -> (BridgeReply, mpsc::Receiver<Vec<u8>>) {
    let (tx, rx) = mpsc::channel();
    (
        Box::new(move |bytes| {
            let _ = tx.send(bytes);
        }),
        rx,
    )
}

fn read(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

fn outcome_of(kit: &app_core::testing::Kit, id: &str) -> Option<BridgeRequestOutcome> {
    kit.core
        .slices()
        .notices
        .get()
        .items()
        .iter()
        .find_map(|notice| match &notice.kind {
            NoticeKind::BridgeRequest {
                id: notice_id,
                outcome,
                ..
            } if notice_id == id => Some(*outcome),
            _ => None,
        })
}

#[tokio::test]
async fn a_service_queues_requests_through_public_core() {
    let kit = kit();
    let public = kit.core.public();
    let mut events = kit.core.events();
    let mut queue = kit.core.slices().bridge.subscribe();

    let (send, _answer) = reply();
    public
        .bridge_push(request("7f3ac0de-11", "connect"), send)
        .unwrap();

    assert!(queue.has_changed().unwrap());
    let queue = queue.borrow_and_update().clone();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.head().unwrap().code(), "7F3A");
    assert_eq!(
        events.try_recv().unwrap(),
        Event::RequestArrived {
            id: "7f3ac0de-11".into()
        }
    );
}

#[tokio::test]
async fn an_answer_reaches_the_site_and_leaves_the_queue() {
    let kit = kit();
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request("c1", "connect"), send)
        .unwrap();

    assert!(kit.core.bridge_answer(
        "c1",
        &BridgeAnswer::Connect(vec![bridge_protocol::reply::SiteAccount {
            pubkey: [1; 32],
            label: "Treasury".into(),
            signs_messages: true,
        }])
    ));

    let reply = read(&answer.try_recv().unwrap());
    assert_eq!(reply["id"], "c1");
    assert_eq!(reply["result"]["accounts"][0]["label"], "Treasury");
    assert!(kit.core.slices().bridge.get().is_empty());
    assert_eq!(outcome_of(&kit, "c1"), Some(BridgeRequestOutcome::Answered));

    // Answered once: a second answer finds nothing waiting.
    assert!(!kit.core.bridge_answer("c1", &BridgeAnswer::Rejected));
}

#[tokio::test]
async fn a_deferred_save_can_start_on_a_thread_with_no_runtime() {
    use app_core::runtime::{TokioClock, TokioSpawner};
    let testing = kit();
    let core = app_core::Core::open(app_core::ports::Ports {
        spawner: std::sync::Arc::new(TokioSpawner::current()),
        clock: std::sync::Arc::new(TokioClock),
        notifier: testing.notifier.clone(),
        storage: std::sync::Arc::new(MemoryStorage::default()),
        links: testing.links.clone(),
        devices: None,
    });
    let public = core.public();
    // How a request that expires reaches the notices, on the bridge's thread.
    std::thread::spawn(move || {
        public.notices_set_bridge_outcome("x", ORIGIN, "connect", BridgeRequestOutcome::Expired);
    })
    .join()
    .unwrap();
}

const ORIGIN: &str = "https://example.org";

fn account(byte: u8) -> wallet_core::ShareableAccount {
    wallet_core::ShareableAccount {
        pubkey: solana_pubkey::Pubkey::new_from_array([byte; 32]),
        label: format!("Wallet {byte}"),
        contract: false,
        summary: String::new(),
        on_device: None,
    }
}

/// A `connect` the window answers with `accounts`.
fn approve(
    kit: &app_core::testing::Kit,
    id: &str,
    accounts: Vec<wallet_core::ShareableAccount>,
    remember: bool,
) -> Value {
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request(id, "connect"), send)
        .unwrap();
    assert!(kit.core.bridge_connect(id, accounts, remember));
    read(&answer.try_recv().unwrap())
}

fn grant_of(kit: &app_core::testing::Kit) -> Option<wallet_core::SiteGrant> {
    kit.core
        .with_workspace(|workspace| workspace.connections.grant(ORIGIN).cloned())
}

#[tokio::test]
async fn approving_sends_the_accounts_in_the_order_chosen() {
    let kit = kit();
    let reply = approve(&kit, "c", vec![account(2), account(1)], false);
    assert_eq!(reply["result"]["accounts"][0]["label"], "Wallet 2");
    assert_eq!(reply["result"]["accounts"][1]["label"], "Wallet 1");
}

/// A `signIn` the window answers with what `signer` signed.
fn sign_in(
    kit: &app_core::testing::Kit,
    id: &str,
    signer: &wallet_core::ShareableAccount,
    remember: bool,
) -> Value {
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request(id, "signIn"), send)
        .unwrap();
    assert!(kit
        .core
        .bridge_sign_in(id, signer.pubkey, b"signed".to_vec(), [9; 64], remember));
    read(&answer.try_recv().unwrap())
}

#[tokio::test]
async fn signing_in_grants_the_account_and_puts_it_first() {
    let kit = kit();
    approve(&kit, "c", vec![account(1)], true);
    let reply = sign_in(&kit, "s", &account(2), true);
    let accounts = reply["result"]["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(
        reply["result"]["accounts"][0]["publicKey"],
        reply["result"]["account"]
    );
    assert_eq!(
        grant_of(&kit).unwrap().accounts,
        vec![account(2).pubkey, account(1).pubkey]
    );
    assert_eq!(
        kit.core.public().site_sign(ORIGIN, &[account(2).pubkey]),
        app_core::bridge::SiteSign::Allowed
    );
    assert!(reply["result"]["signature"].is_string());
}

#[tokio::test]
async fn signing_in_without_remembering_lasts_this_run() {
    let kit = kit();
    sign_in(&kit, "s", &account(1), false);
    assert!(grant_of(&kit).is_none());
    assert_eq!(
        kit.core.public().site_sign(ORIGIN, &[account(1).pubkey]),
        app_core::bridge::SiteSign::Allowed
    );
}

#[tokio::test]
async fn a_message_answer_fits_only_its_method() {
    let kit = kit();
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request("c", "connect"), send)
        .unwrap();
    let signed = BridgeAnswer::SignedMessage {
        signed: b"hi".to_vec(),
        signature: [1; 64],
    };
    assert!(!kit.core.bridge_answer("c", &signed));
    assert!(kit.core.bridge_answer("c", &BridgeAnswer::Rejected));
    drop(answer);

    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request("m", "signMessage"), send)
        .unwrap();
    assert!(kit.core.bridge_answer("m", &signed));
    let reply = read(&answer.try_recv().unwrap());
    assert_eq!(reply["result"]["signedMessage"], "aGk=");
}

#[tokio::test]
async fn a_remembered_site_is_saved_and_survives_a_restart() {
    let storage = MemoryStorage::default();
    {
        let kit = kit_on(storage.clone());
        approve(&kit, "c", vec![account(1)], true);
        assert!(grant_of(&kit).is_some());
    }
    let kit = kit_on(storage);
    let grant = grant_of(&kit).expect("the grant was saved");
    assert_eq!(grant.accounts, vec![account(1).pubkey]);
}

#[tokio::test]
async fn a_site_not_remembered_lasts_only_this_run() {
    let storage = MemoryStorage::default();
    {
        let kit = kit_on(storage.clone());
        approve(&kit, "c", vec![account(1)], false);
        assert!(grant_of(&kit).is_none());
        assert_eq!(
            kit.core.public().site_sign(ORIGIN, &[account(1).pubkey]),
            app_core::bridge::SiteSign::Allowed
        );
    }
    let kit = kit_on(storage);
    assert_eq!(
        kit.core.public().site_sign(ORIGIN, &[account(1).pubkey]),
        app_core::bridge::SiteSign::NotConnected
    );
}

#[tokio::test]
async fn unticking_remember_forgets_a_site() {
    let kit = kit();
    approve(&kit, "c", vec![account(1)], true);
    approve(&kit, "d", vec![account(1)], false);
    assert!(grant_of(&kit).is_none());
    assert!(!kit.core.site_choice(ORIGIN).remembered);
}

#[tokio::test]
async fn a_failed_save_still_connects_the_site_until_the_app_closes() {
    let kit = kit();
    kit.storage.refuse_saves(true);
    let mut events = kit.core.events();
    approve(&kit, "c", vec![account(1)], true);
    assert!(grant_of(&kit).is_none());
    assert_eq!(
        kit.core.public().site_sign(ORIGIN, &[account(1).pubkey]),
        app_core::bridge::SiteSign::Allowed
    );
    let mut failed = false;
    while let Ok(event) = events.try_recv() {
        failed |= matches!(event, Event::SaveFailed { .. });
    }
    assert!(failed);
}

#[tokio::test]
async fn an_answer_must_fit_the_request() {
    let kit = kit();
    let (send, answer) = reply();
    kit.core
        .public()
        .bridge_push(request("s1", "connect"), send)
        .unwrap();

    // A signed transaction does not answer a connect.
    assert!(!kit
        .core
        .bridge_answer("s1", &BridgeAnswer::Signed(vec![vec![0x81]])));
    assert_eq!(kit.core.slices().bridge.get().len(), 1);
    assert!(answer.try_recv().is_err());

    assert!(kit.core.bridge_answer("s1", &BridgeAnswer::Rejected));
    assert_eq!(
        read(&answer.try_recv().unwrap())["error"]["code"],
        "rejected"
    );
    assert_eq!(outcome_of(&kit, "s1"), Some(BridgeRequestOutcome::Rejected));
}

#[tokio::test]
async fn a_batch_goes_back_only_whole() {
    let kit = kit();
    let (send, answer) = reply();
    let mut batch = request("b1", "signTransaction");
    batch.call = SiteCall::SignTransactions(vec![unsigned(); 3]);
    kit.core.public().bridge_push(batch, send).unwrap();
    assert_eq!(
        kit.core
            .slices()
            .bridge
            .get()
            .head()
            .unwrap()
            .transaction_count(),
        3
    );

    // Two of three signed is not an answer, and the request keeps waiting.
    let two = BridgeAnswer::Signed(vec![vec![0x81, 1], vec![0x81, 2]]);
    assert!(!kit.core.bridge_answer("b1", &two));
    assert!(answer.try_recv().is_err());
    assert_eq!(kit.core.slices().bridge.get().len(), 1);

    let all = BridgeAnswer::Signed(vec![vec![0x81, 1], vec![0x81, 2], vec![0x81, 3]]);
    assert!(kit.core.bridge_answer("b1", &all));
    let reply = read(&answer.try_recv().unwrap());
    let signed = reply["result"]["signedTransactions"].as_array().unwrap();
    assert_eq!(signed.len(), 3);
    assert_eq!(signed[2], "gQM=");
    assert_eq!(outcome_of(&kit, "b1"), Some(BridgeRequestOutcome::Answered));
}

fn from_site(id: &str, origin: &str) -> BridgeRequest {
    BridgeRequest {
        origin: origin.into(),
        ..request(id, "connect")
    }
}

#[tokio::test]
async fn the_queue_limits_a_site_and_every_site_together() {
    use bridge_protocol::{MAX_PENDING, MAX_PENDING_PER_ORIGIN};
    let kit = kit();
    let public = kit.core.public();
    let mut replies = Vec::new();
    let mut push = |id: String, origin: &str| {
        let (send, answer) = reply();
        replies.push(answer);
        public.bridge_push(from_site(&id, origin), send)
    };

    for n in 0..MAX_PENDING_PER_ORIGIN {
        push(format!("a{n}"), "https://a.test").unwrap();
    }
    assert_eq!(
        push("a-over".into(), "https://a.test"),
        Err(BridgeRefused::Full)
    );
    // Sites that have room take the queue up to its own limit.
    let mut queued = MAX_PENDING_PER_ORIGIN;
    let mut site = 0;
    while queued < MAX_PENDING {
        for n in 0..MAX_PENDING_PER_ORIGIN.min(MAX_PENDING - queued) {
            push(format!("s{site}-{n}"), &format!("https://s{site}.test")).unwrap();
            queued += 1;
        }
        site += 1;
    }
    assert_eq!(
        push("late".into(), "https://new.test"),
        Err(BridgeRefused::Full)
    );
    assert_eq!(kit.core.slices().bridge.get().len(), MAX_PENDING);

    // Room again once one is answered, for its site or any other.
    assert!(kit.core.bridge_answer("a0", &BridgeAnswer::Rejected));
    push("a-next".into(), "https://a.test").unwrap();
}

#[tokio::test]
async fn an_id_already_waiting_is_refused_and_keeps_its_reply() {
    let kit = kit();
    let public = kit.core.public();
    let (send, first) = reply();
    public
        .bridge_push(request("same", "connect"), send)
        .unwrap();
    let (send, _second) = reply();
    assert_eq!(
        public.bridge_push(from_site("same", "https://other.test"), send),
        Err(BridgeRefused::SameId)
    );
    assert_eq!(kit.core.slices().bridge.get().len(), 1);

    assert!(kit.core.bridge_answer("same", &BridgeAnswer::Rejected));
    assert_eq!(
        read(&first.try_recv().unwrap())["error"]["code"],
        "rejected"
    );
}

#[tokio::test]
async fn the_published_queue_shares_each_request() {
    let kit = kit();
    let public = kit.core.public();
    let (send, _first) = reply();
    public.bridge_push(request("a", "connect"), send).unwrap();
    let before = kit.core.slices().bridge.get();
    let (send, _second) = reply();
    public.bridge_push(request("b", "connect"), send).unwrap();
    let after = kit.core.slices().bridge.get();
    assert!(std::sync::Arc::ptr_eq(
        &before.pending()[0],
        &after.pending()[0]
    ));
}

#[tokio::test]
async fn the_transport_expires_a_request_from_its_own_thread() {
    let kit = kit();
    let public = kit.core.public();
    let (send, answer) = reply();
    public
        .bridge_push(request("late", "signTransaction"), send)
        .unwrap();

    // The bridge serves its socket on a plain thread, outside the runtime.
    // Expiring records a notice, which schedules a save on the spawner.
    let expired = std::thread::spawn(move || public.bridge_expire("late"))
        .join()
        .unwrap();
    assert!(expired);
    settle().await;

    assert!(kit.core.slices().bridge.get().is_empty());
    // The reply was dropped unsent; the transport answers the site itself.
    assert!(matches!(
        answer.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    assert_eq!(
        outcome_of(&kit, "late"),
        Some(BridgeRequestOutcome::Expired)
    );
}

// Resolutions (`docs/intents.md`): a `resolve` queued for the user, whose
// answer goes to its resolution rather than to a reply.

mod resolutions {
    use super::*;
    use app_core::bridge::ResolveCall;
    use app_core::resolutions::{ResolutionCommandError, ResolutionRefused, MAX_RESOLUTIONS};
    use app_core::storage::RESOLUTIONS_FILE;
    use bridge_protocol::reply::SendError;
    use solana_pubkey::Pubkey;
    use std::time::Duration;
    use wallet_core::intent::{
        memo_transaction, Delivery, Execution, MessageMode, NewAccountKey, Notes, Order, Payer,
        Plan, Resolution, ResolutionError, ResolutionFailure, SendResult, SigningProfile, State,
        Timing, TxRole,
    };
    use wallet_core::{apply_local_signatures, parse_wire, KeypairSigner, TxFormat, WalletSigner};

    const SITE: &str = "https://app.example";

    /// The wallet's key, which acts and pays, and the app's, which signs a
    /// memo of its own beside it when the app cosigns.
    struct Keys {
        wallet: KeypairSigner,
        app: KeypairSigner,
    }

    impl Keys {
        fn new() -> Self {
            Self {
                wallet: KeypairSigner::from_seed(&[7; 32]).unwrap(),
                app: KeypairSigner::from_seed(&[8; 32]).unwrap(),
            }
        }

        /// A memo of `signer`'s, paid by the wallet, unsigned.
        fn wire(&self, signer: Pubkey) -> Vec<u8> {
            memo_transaction(
                signer,
                "hello",
                self.wallet.pubkey(),
                None,
                wallet_core::TxFormat::Legacy,
            )
            .unwrap()
        }

        /// `wire` signed by the wallet alone.
        fn signed(&self, wire: &[u8]) -> Vec<u8> {
            apply_local_signatures(wire, &[&self.wallet as &dyn WalletSigner]).unwrap()
        }
    }

    fn profile(account: Pubkey) -> SigningProfile {
        SigningProfile {
            account,
            timing: Timing::Sync,
            execution: Execution::TopLevel,
            carrier: None,
            fee_payer: vec![Payer::Account, Payer::Wallet, Payer::App],
            new_accounts: vec![NewAccountKey::WalletKey, NewAccountKey::AppKey],
            delivery: vec![Delivery::Send, Delivery::Cosign, Delivery::Return],
            messages: vec![MessageMode::Signature, MessageMode::Memo],
            max_transactions: 16,
        }
    }

    /// Request `queue_id` from `origin` for resolution `id`, its
    /// transactions `wires`, and that resolution, awaiting approval.
    fn resolve(
        queue_id: &str,
        origin: &str,
        id: &str,
        account: Pubkey,
        delivery: Delivery,
        wires: &[Vec<u8>],
    ) -> (BridgeRequest, Resolution) {
        let transactions = wires
            .iter()
            .map(|wire| SiteTransaction {
                account,
                wire: SiteWire::Readable(TxFormat::V1, wire.clone()),
                chain: Some("solana:devnet".into()),
                commitment: None,
            })
            .collect();
        let call = ResolveCall {
            resolution: id.into(),
            account,
            chain: "solana:devnet".into(),
            transactions,
            notes: Notes::default(),
            order: Order::Serial,
            delivery,
            profile: profile(account),
            plan: Plan::default(),
            notes_checked: true,
            message: None,
        };
        let request = BridgeRequest {
            id: queue_id.into(),
            origin: origin.into(),
            call: SiteCall::Resolve(Box::new(call)),
        };
        let resolution = Resolution::new(
            id,
            account,
            "solana:devnet",
            Timing::Sync,
            delivery,
            wires.len(),
            1_700_000_000,
        );
        (request, resolution)
    }

    fn state_of(kit: &app_core::testing::Kit, origin: &str, id: &str) -> Option<State> {
        kit.core
            .resolution(origin, id)
            .map(|resolution| resolution.state)
    }

    #[tokio::test]
    async fn a_resolve_joins_the_queue_with_its_resolution_and_belongs_to_its_site() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let mut events = kit.core.events();
        let (request, resolution) = resolve(
            "q1",
            SITE,
            "mint-1",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        let made = kit
            .core
            .public()
            .resolution_create(request, resolution)
            .unwrap();
        assert_eq!(made.state, State::AwaitingApproval);

        let queue = kit.core.slices().bridge.get();
        let head = queue.head().unwrap();
        assert_eq!(head.method(), "resolve");
        assert!(head.sends());
        assert_eq!(head.send_plan(), app_core::bridge::SendPlan::Serial);
        assert_eq!(head.chains(), ["solana:devnet"]);
        assert_eq!(head.resolve().unwrap().resolution, "mint-1");
        assert_eq!(
            events.try_recv().unwrap(),
            Event::RequestArrived { id: "q1".into() }
        );
        let kept = kit.core.slices().resolutions.get();
        assert_eq!(kept.items()[0].origin, SITE);

        // The site's own id once, whatever became of the first.
        let (again, resolution) = resolve(
            "q2",
            SITE,
            "mint-1",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        assert_eq!(
            kit.core.public().resolution_create(again, resolution),
            Err(ResolutionRefused::DuplicateId)
        );
        assert_eq!(kit.core.slices().bridge.get().len(), 1);

        // Another site may use the same id, and neither finds the other's.
        let (other, resolution) = resolve(
            "q3",
            "https://other.example",
            "mint-1",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        kit.core
            .public()
            .resolution_create(other, resolution)
            .unwrap();
        assert!(kit
            .core
            .resolution("https://third.example", "mint-1")
            .is_none());
        assert_eq!(
            kit.core
                .public()
                .resolution_cancel("https://third.example", "mint-1"),
            Err(ResolutionCommandError::NotFound)
        );
        assert_eq!(
            state_of(&kit, SITE, "mint-1"),
            Some(State::AwaitingApproval)
        );
    }

    #[tokio::test]
    async fn the_queues_limits_hold_for_a_resolve_and_refuse_its_resolution_too() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        for n in 0..bridge_protocol::MAX_PENDING_PER_ORIGIN {
            let (request, resolution) = resolve(
                &format!("q{n}"),
                SITE,
                &format!("r{n}"),
                account,
                Delivery::Send,
                &[keys.wire(account)],
            );
            kit.core
                .public()
                .resolution_create(request, resolution)
                .unwrap();
        }
        let (request, resolution) = resolve(
            "over",
            SITE,
            "over",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        assert_eq!(
            kit.core.public().resolution_create(request, resolution),
            Err(ResolutionRefused::Full)
        );
        assert!(kit.core.resolution(SITE, "over").is_none());
    }

    #[tokio::test]
    async fn declined_in_the_window_the_resolution_says_so_and_nothing_is_replied() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let (request, resolution) = resolve(
            "q1",
            SITE,
            "r1",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        kit.core
            .public()
            .resolution_create(request, resolution)
            .unwrap();
        // A signed transaction is not what a resolution delivered send takes.
        assert!(!kit.core.bridge_answer(
            "q1",
            &BridgeAnswer::Signed(vec![keys.signed(&keys.wire(account))])
        ));
        assert!(kit.core.bridge_answer("q1", &BridgeAnswer::Rejected));
        assert_eq!(state_of(&kit, SITE, "r1"), Some(State::Declined));
        assert!(kit.core.slices().bridge.get().is_empty());
        assert_eq!(outcome_of(&kit, "q1"), Some(BridgeRequestOutcome::Rejected));
    }

    #[tokio::test]
    async fn sent_from_the_window_the_resolution_takes_the_wires_and_their_results() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let signed = keys.signed(&keys.wire(account));
        let start = |id: &str| {
            let (request, resolution) =
                resolve(id, SITE, id, account, Delivery::Send, &[keys.wire(account)]);
            kit.core
                .public()
                .resolution_create(request, resolution)
                .unwrap();
            let sending = kit
                .core
                .bridge_begin_send(id, std::slice::from_ref(&signed))
                .unwrap();
            let resolution = kit.core.resolution(SITE, id).unwrap();
            assert_eq!(resolution.state, State::Sending);
            assert_eq!(resolution.transactions[0].role, TxRole::Direct);
            assert_eq!(resolution.transactions[0].of, vec![0]);
            // Whoever began the send sends it; nobody else is handed it.
            assert!(kit
                .core
                .slices()
                .resolutions
                .get()
                .ready_to_send()
                .next()
                .is_none());
            assert!(kit.core.resolution_begin_send(SITE, id).is_none());
            assert!(kit.core.slices().bridge.get().is_empty());
            sending
        };

        let sending = start("landed");
        kit.core.bridge_send_finished(sending, vec![Ok([1; 64])]);
        let done = kit.core.resolution(SITE, "landed").unwrap();
        assert_eq!(done.state, State::Done);
        assert_eq!(done.ran[0].transaction, 0);
        assert_eq!(
            done.ran[0].signature,
            solana_signature::Signature::from([1; 64])
        );
        assert!(done.held.is_empty());
        assert_eq!(
            outcome_of(&kit, "landed"),
            Some(BridgeRequestOutcome::Answered)
        );

        let sending = start("failed");
        kit.core.bridge_send_finished(
            sending,
            vec![Err(SendError::Failed("the node said no".into()))],
        );
        let failed = kit.core.resolution(SITE, "failed").unwrap();
        assert_eq!(failed.state, State::Failed);
        assert_eq!(failed.error.unwrap().message, "the node said no");

        let sending = start("unknown");
        kit.core
            .bridge_send_finished(sending, vec![Err(SendError::Unknown("maybe".into()))]);
        assert_eq!(state_of(&kit, SITE, "unknown"), Some(State::Unknown));
    }

    #[tokio::test]
    async fn cosigned_the_app_owes_its_signature_and_the_window_sends_once_it_is_in() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let app = keys.app.pubkey();
        let wire = keys.wire(app);
        let (request, resolution) = resolve(
            "q1",
            SITE,
            "co",
            account,
            Delivery::Cosign,
            std::slice::from_ref(&wire),
        );
        kit.core
            .public()
            .resolution_create(request, resolution)
            .unwrap();
        // A send is not what a cosigned resolution takes.
        assert!(kit
            .core
            .bridge_begin_send("q1", &[keys.signed(&wire)])
            .is_none());
        assert!(kit
            .core
            .bridge_answer("q1", &BridgeAnswer::Signed(vec![keys.signed(&wire)])));

        let waiting = kit.core.resolution(SITE, "co").unwrap();
        assert_eq!(waiting.state, State::AwaitingApp);
        let awaiting = waiting.awaiting.clone().unwrap();
        assert_eq!(awaiting[0].signers, vec![app]);
        // What the site sees has every slot empty, and not the wires held.
        let public = waiting.public();
        assert!(public.get("held").is_none());
        assert!(public["transactions"][0]["wire"].is_string());

        // A signature that does not verify changes nothing.
        let wrong =
            kit.core
                .public()
                .resolution_provide_signatures(SITE, "co", vec![(0, app, [9; 64])]);
        assert!(matches!(
            wrong,
            Err(ResolutionCommandError::Refused(
                ResolutionError::BadSignature(_)
            ))
        ));
        assert_eq!(state_of(&kit, SITE, "co"), Some(State::AwaitingApp));

        let message = parse_wire(&wire).unwrap().message;
        let signature = keys.app.try_sign_message(&message).unwrap();
        let sending = kit
            .core
            .public()
            .resolution_provide_signatures(SITE, "co", vec![(0, app, *signature.as_array())])
            .unwrap();
        assert_eq!(sending.state, State::Sending);
        let ready: Vec<String> = kit
            .core
            .slices()
            .resolutions
            .get()
            .ready_to_send()
            .map(|kept| kept.resolution.id.clone())
            .collect();
        assert_eq!(ready, ["co"]);
        let wires = kit.core.public().resolution_begin_send(SITE, "co").unwrap();
        assert_eq!(wires.len(), 1);
        // Both signatures are in what goes out.
        let view = parse_wire(&wires[0]).unwrap();
        assert!((0..view.num_required_signatures).all(|slot| view.slot_valid(slot)));
        // Handed once.
        assert!(kit
            .core
            .public()
            .resolution_begin_send(SITE, "co")
            .is_none());
        let landed = kit
            .core
            .public()
            .resolution_sent(
                SITE,
                "co",
                vec![SendResult::Landed {
                    signature: solana_signature::Signature::from([2; 64]),
                }],
            )
            .unwrap();
        assert_eq!(landed.state, State::Done);
    }

    #[tokio::test]
    async fn cancelled_by_its_site_it_leaves_the_queue_and_cannot_be_cancelled_twice() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let (request, resolution) = resolve(
            "q1",
            SITE,
            "c1",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        kit.core
            .public()
            .resolution_create(request, resolution)
            .unwrap();
        let cancelled = kit.core.public().resolution_cancel(SITE, "c1").unwrap();
        assert_eq!(cancelled.state, State::Cancelled);
        assert!(kit.core.slices().bridge.get().is_empty());
        assert_eq!(outcome_of(&kit, "q1"), Some(BridgeRequestOutcome::Expired));
        let again = kit.core.public().resolution_cancel(SITE, "c1").unwrap_err();
        assert_eq!(again.code(), "bad_state");
        // Nor answered once it is out of the queue.
        assert!(!kit.core.bridge_answer("q1", &BridgeAnswer::Rejected));
    }

    #[tokio::test]
    async fn a_check_that_fails_later_ends_it_failed_and_takes_it_out_of_the_queue() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let (request, resolution) = resolve(
            "q1",
            SITE,
            "f1",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        kit.core
            .public()
            .resolution_create(request, resolution)
            .unwrap();
        let failed = kit
            .core
            .public()
            .resolution_fail(
                SITE,
                "f1",
                ResolutionFailure {
                    code: "bad_notes".into(),
                    message: "seen before".into(),
                    requirement: None,
                },
            )
            .unwrap();
        assert_eq!(failed.state, State::Failed);
        assert_eq!(failed.error.unwrap().code, "bad_notes");
        assert!(kit.core.slices().bridge.get().is_empty());
    }

    #[tokio::test]
    async fn approved_for_another_device_it_waits_for_their_signatures() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let wire = keys.wire(account);
        let (request, resolution) = resolve(
            "q1",
            SITE,
            "e1",
            account,
            Delivery::Send,
            std::slice::from_ref(&wire),
        );
        kit.core
            .public()
            .resolution_create(request, resolution)
            .unwrap();
        assert!(kit.core.bridge_approved_elsewhere("q1"));
        assert_eq!(state_of(&kit, SITE, "e1"), Some(State::AwaitingSignatures));
        assert!(kit.core.slices().bridge.get().is_empty());
        // It no longer expires.
        kit.clock.advance(Duration::from_secs(601));
        settle().await;
        assert_eq!(state_of(&kit, SITE, "e1"), Some(State::AwaitingSignatures));

        let composed = vec![wallet_core::intent::Composed {
            role: TxRole::Direct,
            of: vec![0],
            wire: keys.signed(&wire),
        }];
        let sending = kit
            .core
            .resolution_signed_elsewhere(SITE, "e1", composed)
            .unwrap();
        assert_eq!(sending.state, State::Sending);
        assert_eq!(
            kit.core.slices().resolutions.get().ready_to_send().count(),
            1
        );
    }

    #[tokio::test]
    async fn nobody_deciding_it_expires_after_its_window() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let (request, resolution) = resolve(
            "q1",
            SITE,
            "x1",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        kit.core
            .public()
            .resolution_create(request, resolution)
            .unwrap();
        settle().await;
        kit.clock.advance(Duration::from_secs(9 * 60));
        settle().await;
        assert_eq!(state_of(&kit, SITE, "x1"), Some(State::AwaitingApproval));
        kit.clock.advance(Duration::from_secs(60));
        settle().await;
        assert_eq!(state_of(&kit, SITE, "x1"), Some(State::Expired));
        assert!(kit.core.slices().bridge.get().is_empty());
        assert_eq!(outcome_of(&kit, "q1"), Some(BridgeRequestOutcome::Expired));
    }

    #[tokio::test]
    async fn an_async_account_waits_a_day() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let (request, mut resolution) = resolve(
            "q1",
            SITE,
            "x1",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        resolution.timing = Timing::Async;
        kit.core
            .public()
            .resolution_create(request, resolution)
            .unwrap();
        settle().await;
        kit.clock.advance(Duration::from_secs(23 * 60 * 60));
        settle().await;
        assert_eq!(state_of(&kit, SITE, "x1"), Some(State::AwaitingApproval));
        kit.clock.advance(Duration::from_secs(60 * 60));
        settle().await;
        assert_eq!(state_of(&kit, SITE, "x1"), Some(State::Expired));
    }

    #[tokio::test]
    async fn the_wires_held_are_never_written_and_what_needed_them_ends_on_restart() {
        let storage = MemoryStorage::default();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let app = keys.app.pubkey();
        {
            let kit = kit_on(storage.clone());
            // One cosigned and waiting for the app, one sending, one undecided,
            // and one done.
            let wire = keys.wire(app);
            let (request, resolution) = resolve(
                "q1",
                SITE,
                "owed",
                account,
                Delivery::Cosign,
                std::slice::from_ref(&wire),
            );
            kit.core
                .public()
                .resolution_create(request, resolution)
                .unwrap();
            assert!(kit
                .core
                .bridge_answer("q1", &BridgeAnswer::Signed(vec![keys.signed(&wire)])));
            assert!(!kit.core.resolution(SITE, "owed").unwrap().held.is_empty());

            let own = keys.wire(account);
            let (request, resolution) = resolve(
                "q2",
                SITE,
                "out",
                account,
                Delivery::Send,
                std::slice::from_ref(&own),
            );
            kit.core
                .public()
                .resolution_create(request, resolution)
                .unwrap();
            drop(
                kit.core
                    .bridge_begin_send("q2", &[keys.signed(&own)])
                    .unwrap(),
            );

            let (request, resolution) = resolve(
                "q3",
                SITE,
                "undecided",
                account,
                Delivery::Send,
                std::slice::from_ref(&own),
            );
            kit.core
                .public()
                .resolution_create(request, resolution)
                .unwrap();

            let (request, resolution) =
                resolve("q4", SITE, "declined", account, Delivery::Send, &[own]);
            kit.core
                .public()
                .resolution_create(request, resolution)
                .unwrap();
            assert!(kit.core.bridge_answer("q4", &BridgeAnswer::Rejected));

            kit.core.flush();
            let file = String::from_utf8(storage.read(RESOLUTIONS_FILE).unwrap()).unwrap();
            assert!(!file.contains("\"held\""), "{file}");
            assert!(file.contains(SITE));
        }

        let kit = kit_on(storage.clone());
        let after = |id: &str| kit.core.resolution(SITE, id).unwrap();
        let held = after("owed");
        assert_eq!(held.state, State::Failed);
        assert!(held.error.unwrap().message.contains("restarted"));
        assert_eq!(after("out").state, State::Unknown);
        let undecided = after("undecided");
        assert_eq!(undecided.state, State::Failed);
        assert!(undecided.error.unwrap().message.contains("restarted"));
        assert_eq!(after("declined").state, State::Declined);
        // Saved like that, once the saver runs.
        kit.core.flush();
        let file = String::from_utf8(storage.read(RESOLUTIONS_FILE).unwrap()).unwrap();
        assert!(!file.contains("awaiting-app"));
    }

    #[tokio::test]
    async fn only_the_latest_are_kept() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        for n in 0..=MAX_RESOLUTIONS {
            let (request, resolution) = resolve(
                &format!("q{n}"),
                SITE,
                &format!("r{n}"),
                account,
                Delivery::Send,
                &[keys.wire(account)],
            );
            kit.core
                .public()
                .resolution_create(request, resolution)
                .unwrap();
            // Out of the queue at once, so it has room for the next.
            assert!(kit
                .core
                .bridge_answer(&format!("q{n}"), &BridgeAnswer::Rejected));
        }
        let kept = kit.core.slices().resolutions.get();
        assert_eq!(kept.len(), MAX_RESOLUTIONS);
        assert!(kit.core.resolution(SITE, "r0").is_none());
        assert!(kit
            .core
            .resolution(SITE, &format!("r{MAX_RESOLUTIONS}"))
            .is_some());
    }

    #[tokio::test]
    async fn the_replaced_addresses_are_recorded_only_before_it_is_decided() {
        let kit = kit();
        let keys = Keys::new();
        let account = keys.wallet.pubkey();
        let (request, resolution) = resolve(
            "q1",
            SITE,
            "m1",
            account,
            Delivery::Send,
            &[keys.wire(account)],
        );
        kit.core
            .public()
            .resolution_create(request, resolution)
            .unwrap();
        let replaced = std::collections::BTreeMap::from([(
            Pubkey::new_from_array([3; 32]),
            Pubkey::new_from_array([4; 32]),
        )]);
        let recorded = kit
            .core
            .resolution_set_replaced(SITE, "m1", replaced.clone())
            .unwrap();
        assert_eq!(recorded.replaced, replaced);
        assert!(kit.core.bridge_answer("q1", &BridgeAnswer::Rejected));
        assert_eq!(
            kit.core
                .resolution_set_replaced(SITE, "m1", replaced)
                .unwrap_err()
                .code(),
            "bad_state"
        );
    }
}
