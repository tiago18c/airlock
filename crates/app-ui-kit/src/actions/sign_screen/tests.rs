//! What a task that takes time does when the screen has moved on: the real form
//! and the real actions, on a node whose answers a test holds back.

use super::*;
use crate::state::forms::{SignForm, Submission};
use crate::state::session::OnlineHooks;
use crate::state::submit::{SiteSendFut, SubmitFut, SubmitHandle, TxSubmit};
use crate::state::use_session;
use app_core::bridge::{BridgeRequest, SiteCall, SiteTransaction, SiteWire};
use app_core::testing::{kit, Kit};
use app_core::{Core, FlowId};
use dioxus::dioxus_core::{NoOpMutations, ScopeId, VirtualDom};
use serde_json::Value;
use solana_signature::Signature;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use wallet_core::simulate::SimulationReply;
use wallet_core::{apply_local_signatures, compile_v1, Cluster, SolanaRpc};
use wallet_flows::testing::{legacy_transfer, pk, transfer_instruction, FakeRpc, Gate};

#[derive(Clone)]
struct Held {
    core: Core,
    rpc: Arc<FakeRpc>,
    submit: SubmitHandle,
    session: Rc<Cell<Option<Session>>>,
}

fn app(held: Held) -> Element {
    let core = use_hook(|| CopyValue::new(held.core.clone()));
    let rpc = held.rpc.clone();
    let hooks = OnlineHooks::new(
        move || Ok(rpc.clone() as Arc<dyn SolanaRpc>),
        || {},
        || {},
        || Box::pin(async { Ok(String::new()) }),
    );
    let session = use_session(core, hooks, held.submit.clone());
    held.session.set(Some(session));
    rsx! {}
}

/// A running window, with a vault that holds one hot key.
struct Screen {
    kit: Kit,
    dom: VirtualDom,
    session: Session,
    rpc: Arc<FakeRpc>,
    key: Pubkey,
}

const SEED: [u8; 32] = [7; 32];

/// A node that answers a simulation as a transaction that would succeed.
fn good_node() -> FakeRpc {
    FakeRpc {
        simulation: Some(SimulationReply {
            err: None,
            logs: Vec::new(),
            units_consumed: Some(1000),
            loaded_accounts_data_size: Some(1000),
            balances: Vec::new(),
            inner: Vec::new(),
            return_data: None,
        }),
        ..FakeRpc::default()
    }
}

impl Screen {
    async fn new() -> Self {
        Self::on(good_node(), SubmitHandle::none()).await
    }

    async fn submitting(submit: SubmitHandle) -> Self {
        Self::on(good_node(), submit).await
    }

    async fn on(rpc: FakeRpc, submit: SubmitHandle) -> Self {
        let kit = kit();
        kit.core.create_vault("pw").unwrap();
        let wallet = wallet_core::WalletRecord::from_keypair_seed("Hot", SEED);
        let key = wallet.address().unwrap();
        let signers = vec![wallet_core::NewSigner {
            pubkey: key,
            label: wallet.label.clone(),
            origin: wallet_core::Origin::Imported,
            wallet,
        }];
        kit.core
            .enroll(
                "add_signers",
                |data| data.add_signers(&signers),
                |book, _| wallet_core::name_signers(book, &signers),
            )
            .unwrap();
        let rpc = Arc::new(rpc);
        let session = Rc::new(Cell::new(None));
        let mut dom = VirtualDom::new_with_props(
            app,
            Held {
                core: kit.core.clone(),
                rpc: rpc.clone(),
                submit,
                session: session.clone(),
            },
        );
        dom.rebuild_in_place();
        let session = session.get().unwrap();
        let mut screen = Self {
            kit,
            dom,
            session,
            rpc,
            key,
        };
        screen.settle().await;
        screen
    }

    fn form(&self) -> SignForm {
        self.session.forms.sign
    }

    /// Run `then` as the window would, in its scope.
    fn act<R>(&self, then: impl FnOnce() -> R) -> R {
        self.dom.in_scope(ScopeId::ROOT, then)
    }

    /// Let what is under way go as far as it can.
    async fn settle(&mut self) {
        for _ in 0..4 {
            let _ = tokio::time::timeout(Duration::from_millis(40), self.dom.wait_for_work()).await;
            self.dom.render_immediate(&mut NoOpMutations);
        }
    }

    fn wire(&self) -> Option<Vec<u8>> {
        self.act(|| self.form().wire.peek().clone())
    }

    fn load(&self, wire: &[u8], from: TxSource) {
        self.act(|| self.form().load(SignLoad::new(wire.to_vec(), from)));
    }

    /// A transaction this window's key can sign, different for each `to`.
    fn transfer(&self, to: u8) -> Vec<u8> {
        compile_v1(
            self.key,
            vec![transfer_instruction(self.key, pk(to))],
            None,
            Some(solana_hash::Hash::new_from_array([3; 32])),
        )
        .unwrap()
        .wire
    }

    /// A busy flag, as a button of the screen has.
    fn busy(&self) -> Signal<bool> {
        self.act(|| Signal::new(false))
    }

    /// A site's request waiting in the core, and where its reply goes.
    fn request(&self, id: &str, origin: &str, wire: &[u8]) -> mpsc::Receiver<Vec<u8>> {
        let (send, reply) = mpsc::channel();
        self.kit
            .core
            .public()
            .bridge_push(
                BridgeRequest {
                    id: id.into(),
                    origin: origin.into(),
                    call: SiteCall::SignTransactions(vec![SiteTransaction {
                        account: self.key,
                        wire: SiteWire::Readable(TxFormat::V1, wire.to_vec()),
                        chain: None,
                        commitment: None,
                    }]),
                },
                Box::new(move |bytes| {
                    let _ = send.send(bytes);
                }),
            )
            .unwrap();
        reply
    }
}

fn browser(id: &str, origin: &str) -> TxSource {
    TxSource::Browser {
        id: id.into(),
        origin: origin.into(),
    }
}

fn is_signed(wire: &[u8]) -> bool {
    let view = parse_wire(wire).unwrap();
    (0..view.num_required_signatures).all(|slot| view.slot_valid(slot))
}

fn signed_by_seed(wire: &[u8]) -> Vec<u8> {
    let key = KeypairSigner::from_seed(&SEED).unwrap();
    let signers: [&dyn WalletSigner; 1] = [&key];
    apply_local_signatures(wire, &signers).unwrap()
}

// ---- The keys a press of Sign builds.

#[tokio::test]
async fn a_press_of_sign_builds_only_the_keys_the_transaction_asks_for() {
    let screen = Screen::new().await;
    let other = wallet_core::WalletRecord::from_keypair_seed("Other", [8; 32]);
    let other_key = other.address().unwrap();
    let signers = vec![wallet_core::NewSigner {
        pubkey: other_key,
        label: other.label.clone(),
        origin: wallet_core::Origin::Imported,
        wallet: other,
    }];
    screen
        .kit
        .core
        .enroll(
            "add_signers",
            |data| data.add_signers(&signers),
            |book, _| wallet_core::name_signers(book, &signers),
        )
        .unwrap();

    let built = |wire: &[u8]| {
        screen.act(|| {
            screen
                .session
                .local_signers_for(wire)
                .iter()
                .map(WalletSigner::pubkey)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(built(&screen.transfer(1)), [screen.key]);
    // A key it does not name, however many it would take to find out.
    let elsewhere = compile_v1(
        pk(50),
        vec![transfer_instruction(pk(50), pk(1))],
        None,
        Some(solana_hash::Hash::new_from_array([3; 32])),
    )
    .unwrap();
    assert!(built(&elsewhere.wire).is_empty());
    assert!(built(b"not a transaction").is_empty());
}

// ---- What is stored behind an approval.

fn some_target() -> wallet_core::StoredTarget {
    let squad = wallet_core::squads::Squad::v4(&pk(50));
    wallet_core::StoredTarget {
        squad,
        proposal: squad.proposal(1),
        transaction: None,
    }
}

#[tokio::test]
async fn a_late_answer_about_what_is_stored_stays_off_what_took_its_place() {
    let mut screen = Screen::new().await;
    screen.load(&screen.transfer(1), TxSource::Pasted);
    screen.settle().await;
    let reading = screen.act(|| Signal::new(false));

    screen.rpc.gate.hold();
    screen.act(|| read_stored_behind(screen.session, vec![some_target()], reading));
    screen.settle().await;
    assert!(screen.act(|| *reading.peek()));

    screen.load(&screen.transfer(2), TxSource::Pasted);
    screen.rpc.gate.open();
    screen.settle().await;
    // Not an answer for this one, and free to ask about it.
    assert!(!screen.act(|| *reading.peek()));
    assert_eq!(
        screen.act(|| screen.form().stored_behind.peek().clone()),
        None
    );
}

#[tokio::test]
async fn a_node_that_cannot_be_asked_has_said_nothing_and_is_not_asked_again() {
    let down = FakeRpc {
        offline: true,
        ..good_node()
    };
    let mut screen = Screen::on(down, SubmitHandle::none()).await;
    screen.load(&screen.transfer(1), TxSource::Pasted);
    screen.settle().await;
    let reading = screen.act(|| Signal::new(false));

    screen.act(|| read_stored_behind(screen.session, vec![some_target()], reading));
    screen.settle().await;
    assert!(!screen.act(|| *reading.peek()));
    assert_eq!(
        screen.act(|| screen.form().stored_behind.peek().clone()),
        Some(Default::default())
    );
}

#[tokio::test]
async fn what_was_checked_or_read_goes_with_the_transaction_and_a_new_note_unsays_it() {
    let mut screen = Screen::new().await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Pasted);
    screen.settle().await;
    let form = screen.form();
    let tx = decode_on_sign(&wire, &wallet_core::LookupTables::new()).unwrap();
    let mark = || {
        screen.act(|| {
            let (mut checked, mut behind) = (form.blind_checked, form.stored_behind);
            checked.set(Some(wallet_core::sha256_bytes(&tx.message)));
            behind.set(Some(Default::default()));
        })
    };

    // A note for this message may say something else of what was checked.
    mark();
    let store = wallet_core::IdlStore::open(
        wallet_vault::store::MemoryStore::new(),
        wallet_core::MemoryBodies::new(),
    );
    let note = wallet_core::context_note(&store, &tx).unwrap();
    screen.act(|| attach_context(screen.session, note));
    assert_eq!(screen.act(|| *form.blind_checked.peek()), None);

    // One for some other message changes nothing.
    mark();
    let other = decode_on_sign(&screen.transfer(2), &wallet_core::LookupTables::new()).unwrap();
    let elsewhere = wallet_core::context_note(&store, &other).unwrap();
    screen.act(|| attach_context(screen.session, elsewhere));
    assert!(screen.act(|| form.blind_checked.peek().is_some()));

    // Another transaction starts over.
    screen.load(&screen.transfer(3), TxSource::Pasted);
    assert_eq!(screen.act(|| *form.blind_checked.peek()), None);
    assert_eq!(screen.act(|| form.stored_behind.peek().clone()), None);
}

// ---- A simulation.

#[tokio::test]
async fn a_simulation_fits_the_limits_of_the_transaction_it_asked_about() {
    let mut screen = Screen::new().await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Pasted);
    let tx = decode_v1_transaction(&wire).unwrap();

    screen.act(|| start_simulation(screen.session, tx));
    screen.settle().await;

    let fitted = screen.wire().unwrap();
    assert_ne!(fitted, wire);
    let form = screen.form();
    screen.act(|| {
        assert!(matches!(*form.simulation.peek(), SimState::Done(_)));
        // It came from outside, so it says what changed in it.
        assert!(form
            .note
            .peek()
            .as_ref()
            .unwrap()
            .plain()
            .contains("compute limits"));
        let digest = sha256_bytes(&decode_v1_transaction(&fitted).unwrap().message);
        assert_eq!(*form.simulated_for.peek(), Some(digest));
    });
}

#[tokio::test]
async fn a_late_simulation_does_not_replace_the_transaction_that_took_its_place() {
    let mut screen = Screen::new().await;
    let (asked, next) = (screen.transfer(1), screen.transfer(2));
    screen.load(&asked, TxSource::Pasted);
    let tx = decode_v1_transaction(&asked).unwrap();

    screen.rpc.gate.hold();
    screen.act(|| start_simulation(screen.session, tx));
    screen.settle().await;
    screen.load(&next, browser("b", "https://b.test"));
    screen.rpc.gate.open();
    screen.settle().await;

    assert_eq!(screen.wire().unwrap(), next);
    let form = screen.form();
    screen.act(|| {
        assert!(matches!(*form.simulation.peek(), SimState::Idle));
        assert_eq!(*form.source.peek(), Some(browser("b", "https://b.test")));
    });
}

#[tokio::test]
async fn a_late_simulation_does_not_take_a_signature_off_the_transaction() {
    let mut screen = Screen::new().await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Pasted);
    let tx = decode_v1_transaction(&wire).unwrap();

    screen.rpc.gate.hold();
    screen.act(|| start_simulation(screen.session, tx));
    screen.settle().await;
    // Signed while the node was answering: a fit would void the signature.
    let signed = signed_by_seed(&wire);
    let mut stored = screen.form().wire;
    screen.act(|| stored.set(Some(signed.clone())));
    screen.rpc.gate.open();
    screen.settle().await;

    assert_eq!(screen.wire().unwrap(), signed);
    assert!(is_signed(&signed));
}

#[tokio::test]
async fn fitting_what_this_app_made_says_nothing() {
    let mut screen = Screen::new().await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Build);
    let tx = decode_v1_transaction(&wire).unwrap();
    screen.act(|| start_simulation(screen.session, tx));
    screen.settle().await;

    assert_ne!(screen.wire().unwrap(), wire);
    let form = screen.form();
    screen.act(|| assert!(form.note.peek().is_none()));
}

// ---- A signature.

#[tokio::test]
async fn a_signature_goes_to_the_request_it_was_made_for() {
    let mut screen = Screen::new().await;
    let wire = screen.transfer(1);
    let reply = screen.request("a", "https://a.test", &wire);
    screen.settle().await;
    screen.load(&wire, browser("a", "https://a.test"));

    let busy = screen.busy();
    screen.act(|| sign_loaded(screen.session, busy, BlockhashOnSign::Keep));
    screen.settle().await;

    let answer: Value = serde_json::from_slice(&reply.try_recv().unwrap()).unwrap();
    assert_eq!(answer["id"], "a");
    assert!(answer["result"]["signedTransactions"].is_array());
    assert!(is_signed(&screen.wire().unwrap()));
    screen.act(|| assert!(!*busy.peek()));
}

#[tokio::test]
async fn a_signature_never_reaches_a_request_that_replaced_its_own_on_screen() {
    let mut screen = Screen::new().await;
    let (for_a, for_b) = (screen.transfer(1), screen.transfer(2));
    let reply_a = screen.request("a", "https://a.test", &for_a);
    let reply_b = screen.request("b", "https://b.test", &for_b);
    screen.settle().await;
    screen.load(&for_a, browser("a", "https://a.test"));

    screen.rpc.gate.hold();
    let busy = screen.busy();
    screen.act(|| sign_loaded(screen.session, busy, BlockhashOnSign::Fresh));
    screen.settle().await;
    // Site B's request comes up on Review while A's signature is being made.
    screen.load(&for_b, browser("b", "https://b.test"));
    screen.rpc.gate.open();
    screen.settle().await;

    assert!(
        reply_b.try_recv().is_err(),
        "site B got a transaction signed for site A"
    );
    assert!(reply_a.try_recv().is_err());
    assert_eq!(screen.wire().unwrap(), for_b);
    assert_eq!(screen.kit.core.slices().bridge.get().len(), 2);
    screen.act(|| assert!(!*busy.peek()));
}

#[tokio::test]
async fn a_signature_finished_after_the_screen_cleared_stays_off_it() {
    let mut screen = Screen::new().await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Pasted);

    screen.rpc.gate.hold();
    let busy = screen.busy();
    screen.act(|| sign_loaded(screen.session, busy, BlockhashOnSign::Fresh));
    screen.settle().await;
    let form = screen.form();
    screen.act(|| form.clear());
    screen.rpc.gate.open();
    screen.settle().await;

    assert_eq!(screen.wire(), None);
    screen.act(|| assert_eq!(*form.error.peek(), None));
}

#[tokio::test]
async fn a_press_that_waited_for_the_vault_signs_nothing_if_the_screen_moved_on() {
    let mut screen = Screen::new().await;
    let (pressed, next) = (screen.transfer(1), screen.transfer(2));
    screen.load(&pressed, TxSource::Pasted);
    screen.kit.core.lock();
    screen.settle().await;

    let busy = screen.busy();
    screen.act(|| sign_loaded(screen.session, busy, BlockhashOnSign::Keep));
    screen.load(&next, TxSource::Pasted);
    screen.kit.core.unlock("pw").unwrap();
    screen.settle().await;
    screen.act(|| screen.session.unlock_prompt_done());
    screen.settle().await;

    assert_eq!(
        screen.wire().unwrap(),
        next,
        "signed something nobody pressed Sign on"
    );
}

#[tokio::test]
async fn a_press_that_waited_for_the_vault_signs_what_it_was_for() {
    let mut screen = Screen::new().await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Pasted);
    screen.kit.core.lock();
    screen.settle().await;

    let busy = screen.busy();
    screen.act(|| sign_loaded(screen.session, busy, BlockhashOnSign::Keep));
    screen.kit.core.unlock("pw").unwrap();
    screen.settle().await;
    screen.act(|| screen.session.unlock_prompt_done());
    screen.settle().await;

    assert!(is_signed(&screen.wire().unwrap()));
}

#[tokio::test]
async fn a_failed_signature_for_what_left_the_screen_says_nothing_on_it() {
    // A node that fails: a press on an expired blockhash can sign nothing.
    let down = FakeRpc {
        offline: true,
        ..good_node()
    };
    let mut screen = Screen::on(down, SubmitHandle::none()).await;
    let (pressed, next) = (screen.transfer(1), screen.transfer(2));
    screen.load(&pressed, TxSource::Pasted);
    screen.rpc.gate.hold();
    let busy = screen.busy();
    screen.act(|| sign_loaded(screen.session, busy, BlockhashOnSign::Required));
    screen.settle().await;
    screen.load(&next, TxSource::Pasted);
    screen.rpc.gate.open();
    screen.settle().await;

    let form = screen.form();
    screen.act(|| {
        assert_eq!(*form.error.peek(), None);
        assert!(!*busy.peek());
    });
    assert_eq!(screen.wire().unwrap(), next);
}

#[tokio::test]
async fn a_failed_signature_for_what_is_on_screen_says_why() {
    let down = FakeRpc {
        offline: true,
        ..good_node()
    };
    let mut screen = Screen::on(down, SubmitHandle::none()).await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Pasted);
    let busy = screen.busy();
    screen.act(|| sign_loaded(screen.session, busy, BlockhashOnSign::Required));
    screen.settle().await;

    let form = screen.form();
    screen.act(|| assert!(form.error.peek().is_some()));
    assert_eq!(screen.wire().unwrap(), wire);
}

// ---- A blockhash.

#[tokio::test]
async fn a_late_blockhash_leaves_alone_the_transaction_that_took_its_place() {
    let mut screen = Screen::new().await;
    let (old, next) = (screen.transfer(1), screen.transfer(2));
    screen.load(&old, TxSource::Pasted);

    screen.rpc.gate.hold();
    let busy = screen.busy();
    screen.act(|| refresh_loaded(screen.session, busy));
    screen.settle().await;
    screen.load(&next, TxSource::Pasted);
    screen.rpc.gate.open();
    screen.settle().await;

    assert_eq!(screen.wire().unwrap(), next);
    let form = screen.form();
    screen.act(|| {
        // Its clock is its own: the height of the other blockhash is not on it.
        assert_eq!(*form.clock.expiry.peek(), None);
        assert_eq!(*form.note.peek(), None);
        assert!(!*busy.peek());
    });
}

#[tokio::test]
async fn a_new_blockhash_lands_with_its_expiry_when_nothing_else_happened() {
    let mut screen = Screen::new().await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Pasted);
    let busy = screen.busy();
    screen.act(|| refresh_loaded(screen.session, busy));
    screen.settle().await;

    assert_ne!(screen.wire().unwrap(), wire);
    let form = screen.form();
    screen.act(|| {
        assert_eq!(*form.clock.expiry.peek(), Some(BlockhashExpiry::exact(150)));
        assert!(form.note.peek().is_some());
    });
}

// ---- A submit.

/// Sends once the gate is open.
struct HeldSubmit(Arc<Gate>);

impl TxSubmit for HeldSubmit {
    fn submit(
        &self,
        _rpc: Arc<dyn SolanaRpc>,
        _wire: Vec<u8>,
        _flow: FlowId,
        _label: String,
        _cluster: Cluster,
        _last_valid_height: Option<u64>,
    ) -> SubmitFut {
        let gate = self.0.clone();
        Box::pin(async move {
            gate.pass().await;
            Ok(Signature::from([1; 64]))
        })
    }

    fn watch_site_send(
        &self,
        _rpc: Arc<dyn SolanaRpc>,
        _returned: Vec<(Vec<u8>, FlowId)>,
        _origin: String,
        _cluster: Cluster,
    ) -> SiteSendFut {
        Box::pin(async { Vec::new() })
    }
}

#[tokio::test]
async fn a_submit_keeps_what_it_sent_on_screen_and_says_it_went() {
    let gate = Arc::new(Gate::default());
    let mut screen = Screen::submitting(SubmitHandle::new(HeldSubmit(gate.clone()))).await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Pasted);
    let busy = screen.busy();
    let form = screen.form();

    gate.hold();
    screen.act(|| submit_loaded(screen.session, busy));
    screen.settle().await;
    screen.act(|| assert_eq!(*form.submission.peek(), Some(Submission::Sending)));

    gate.open();
    screen.settle().await;
    // The user goes on from there: to History, a clear screen, or back.
    assert_eq!(screen.wire().unwrap(), wire);
    screen.act(|| {
        assert!(form.last_signature.peek().is_some());
        assert_eq!(
            *form.submission.peek(),
            Some(Submission::Sent(Signature::from([1; 64])))
        );
    });
    screen.act(|| form.clear());
    screen.act(|| assert_eq!(*form.submission.peek(), None));
}

#[tokio::test]
async fn a_refused_submit_keeps_the_transaction_for_another_try() {
    let mut screen = Screen::new().await;
    let wire = screen.transfer(1);
    screen.load(&wire, TxSource::Pasted);
    let busy = screen.busy();
    let form = screen.form();
    screen.act(|| submit_loaded(screen.session, busy));
    screen.settle().await;

    assert_eq!(screen.wire().unwrap(), wire);
    screen.act(|| {
        assert!(matches!(
            &*form.submission.peek(),
            Some(Submission::Refused(_))
        ));
        assert!(!*busy.peek());
    });
}

#[tokio::test]
async fn a_late_submit_does_not_clear_what_took_its_place() {
    let gate = Arc::new(Gate::default());
    let mut screen = Screen::submitting(SubmitHandle::new(HeldSubmit(gate.clone()))).await;
    let (sent, next) = (screen.transfer(1), screen.transfer(2));
    screen.load(&sent, TxSource::Pasted);

    gate.hold();
    let busy = screen.busy();
    screen.act(|| submit_loaded(screen.session, busy));
    screen.settle().await;
    screen.load(&next, TxSource::Pasted);
    gate.open();
    screen.settle().await;

    assert_eq!(screen.wire().unwrap(), next);
    screen.act(|| assert!(!*busy.peek()));
}

// ---- Arrivals.

/// A legacy transaction asking a signer unknown here to sign, which makes
/// preparing it ask the node to simulate it.
fn slow_arrival(signer: u8, to: u8) -> Vec<u8> {
    legacy_transfer(pk(signer), pk(to))
}

fn names(wire: &[u8], key: Pubkey) -> bool {
    parse_wire(wire).unwrap().static_keys.contains(&key)
}

#[tokio::test]
async fn a_slow_arrival_lands_when_nothing_overtakes_it() {
    let mut screen = Screen::new().await;
    let slow = slow_arrival(200, 1);
    screen.rpc.gate.hold();
    screen.act(|| {
        accept(
            screen.session,
            QrObject::Transaction(slow),
            TxSource::Pasted,
        )
    });
    screen.settle().await;
    assert_eq!(screen.wire(), None);

    screen.rpc.gate.open();
    screen.settle().await;
    assert!(names(&screen.wire().unwrap(), pk(200)));
}

#[tokio::test]
async fn a_slow_arrival_does_not_replace_a_newer_one() {
    let mut screen = Screen::new().await;
    let slow = slow_arrival(200, 1);
    let quick = screen.transfer(2);

    screen.rpc.gate.hold();
    screen.act(|| {
        accept(
            screen.session,
            QrObject::Transaction(slow),
            TxSource::Pasted,
        )
    });
    screen.settle().await;
    screen.act(|| {
        accept(
            screen.session,
            QrObject::Transaction(quick.clone()),
            TxSource::Pasted,
        )
    });
    screen.rpc.gate.open();
    screen.settle().await;

    assert_eq!(screen.wire().unwrap(), quick);
    let form = screen.form();
    screen.act(|| assert_eq!(*form.error.peek(), None));
}

#[tokio::test]
async fn the_last_of_two_slow_arrivals_is_the_one_that_stays() {
    let mut screen = Screen::new().await;
    let first = slow_arrival(200, 1);
    let last = slow_arrival(201, 2);

    screen.rpc.gate.hold();
    screen.act(|| {
        accept(
            screen.session,
            QrObject::Transaction(first),
            TxSource::Pasted,
        )
    });
    screen.settle().await;
    screen.act(|| {
        accept(
            screen.session,
            QrObject::Transaction(last),
            TxSource::Pasted,
        )
    });
    screen.settle().await;
    screen.rpc.gate.open();
    screen.settle().await;

    let shown = screen.wire().unwrap();
    assert!(names(&shown, pk(201)));
    assert!(!names(&shown, pk(200)));
}

// ---- The form.

#[tokio::test]
async fn loading_or_clearing_leaves_nothing_of_the_last_transaction() {
    let screen = Screen::new().await;
    let form = screen.form();
    let wire = screen.transfer(1);
    screen.act(|| {
        form.load(
            SignLoad::new(wire.clone(), TxSource::Pasted)
                .with_note("a note")
                .expiring_at(Some(9)),
        );
        let (mut error, mut rebuilt, mut returned, mut marked, mut state) = (
            form.error,
            form.rebuilt,
            form.returned,
            form.simulated_for,
            form.simulation,
        );
        error.set(Some("wrong".into()));
        rebuilt.set(true);
        returned.set(true);
        marked.set(Some([1; 32]));
        state.set(SimState::Running);
    });
    let settled = |wire_expected: Option<&[u8]>, note: bool, expiry: Option<u64>| {
        screen.act(|| {
            assert_eq!(form.wire.peek().as_deref(), wire_expected);
            assert_eq!(*form.error.peek(), None);
            assert_eq!(form.note.peek().is_some(), note);
            assert!(!*form.rebuilt.peek());
            assert!(!*form.returned.peek());
            assert_eq!(*form.simulated_for.peek(), None);
            assert!(matches!(*form.simulation.peek(), SimState::Idle));
            assert_eq!(
                (*form.clock.expiry.peek()).map(|at| at.last_valid_height),
                expiry
            );
        });
    };

    let next = screen.transfer(2);
    screen.act(|| form.load(SignLoad::new(next.clone(), TxSource::Pasted)));
    settled(Some(&next), false, None);

    screen.act(|| {
        let (mut error, mut rebuilt) = (form.error, form.rebuilt);
        error.set(Some("wrong".into()));
        rebuilt.set(true);
        form.clear();
    });
    settled(None, false, None);
}

#[tokio::test]
async fn an_empty_note_says_nothing() {
    let screen = Screen::new().await;
    let form = screen.form();
    let wire = screen.transfer(1);
    screen.act(|| {
        form.load(SignLoad::new(wire, TxSource::Pasted).with_note(""));
        assert!(form.note.peek().is_none());
    });
}

#[tokio::test]
async fn each_replacement_is_a_new_generation_and_a_signature_is_not() {
    let screen = Screen::new().await;
    let form = screen.form();
    let (one, two) = (screen.transfer(1), screen.transfer(2));
    screen.act(|| {
        let start = form.generation();
        form.load(SignLoad::new(one.clone(), TxSource::Pasted));
        let loaded = form.generation();
        assert_ne!(loaded, start);

        let mut stored = form.wire;
        stored.set(Some(signed_by_seed(&one)));
        assert!(form.still_on_screen(loaded));

        form.load(SignLoad::new(two.clone(), TxSource::Pasted));
        assert!(!form.still_on_screen(loaded));
        let again = form.generation();
        form.clear();
        assert!(!form.still_on_screen(again));
    });
}
