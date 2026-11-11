//! Reading what a transaction says, away from the window.
//!
//! A transaction is read with IDLs, which can be large and which a site can have
//! had written. So the reading runs on another thread, and the window waits for it
//! for [`READING_DEADLINE`] at most. After that the screen says what can be said
//! without any IDL: which programs are called and with what bytes. What
//! `wallet_core::risks_in` and `wallet_core::blind_in` look at is all there, since
//! they read bytes too.

use super::*;
use crate::rt::off_the_window_within;
use crate::state::online::Network;
use dioxus::dioxus_core::Task;
use std::time::Duration;
use wallet_core::protocol::TxContext;
use wallet_core::relay::VectorContext;
use wallet_core::{
    display_accounts, explain_instructions, explain_transaction, CatalogView, DisplayNote,
    ExplainedTx, Instruction, NoIdls, WalletSummary,
};

/// How long the window waits for words before it shows what it has without them.
pub const READING_DEADLINE: Duration = Duration::from_secs(4);

/// Where something worked out away from the window stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Worked<T> {
    Waiting,
    Done(T),
    /// It took longer than it may. It goes on to its end, since a thread cannot be
    /// stopped, and what comes of it is dropped.
    GaveUp,
}

/// What came of something worked out away from the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer<T> {
    pub worked: Worked<T>,
    /// It is for something older, and a newer answer is on its way. Words read
    /// before an IDL arrived stand on screen, but nothing is decided on them.
    pub stale: bool,
}

/// Which IDLs a catalog holds, to tell whether an answer was read with these.
fn marks_of(session: &Session) -> Vec<(Pubkey, [u8; 32])> {
    session
        .idls
        .read()
        .records()
        .iter()
        .map(|record| (record.program_id, record.sha256))
        .collect()
}

/// What `work` makes of `key` with the catalog as it is, worked out away from the
/// window, and again when `key` or the catalog changes. What came of an earlier key
/// stands until then if `still_holds` says it may, marked stale. Otherwise it is
/// `Waiting`: what was read of one transaction is never shown for another.
pub fn use_worked_out<K, T>(
    key: K,
    deadline: Duration,
    still_holds: fn(&K, &K) -> bool,
    work: fn(&CatalogView, &K) -> T,
) -> Answer<T>
where
    K: Clone + PartialEq + Send + 'static,
    T: Clone + Send + 'static,
{
    type Marks = Vec<(Pubkey, [u8; 32])>;
    let session = use_context::<Session>();
    let mut answer = use_signal(|| None::<(K, Marks, Worked<T>)>);
    let mut running = use_signal(|| None::<Task>);
    let current = key.clone();
    let marks = marks_of(&session);
    use_effect(use_reactive!(|(key)| {
        let view = session.idls.read().view();
        let read_with = marks_of(&session);
        if let Some(earlier) = *running.peek() {
            earlier.cancel();
        }
        let for_work = key.clone();
        let for_answer = key.clone();
        let task = spawn(async move {
            let done = off_the_window_within(deadline, move || work(&view, &for_work)).await;
            let worked = done.map_or(Worked::GaveUp, Worked::Done);
            answer.set(Some((for_answer, read_with, worked)));
        });
        running.set(Some(task));
    }));
    let answered = answer.read();
    match &*answered {
        Some((asked, read_with, worked)) if still_holds(asked, &current) => Answer {
            worked: worked.clone(),
            stale: *asked != current || *read_with != marks,
        },
        _ => Answer {
            worked: Worked::Waiting,
            stale: false,
        },
    }
}

/// What the screen has of a transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct TxReading {
    /// In words, or what can be said without them until then.
    pub explained: ExplainedTx,
    /// There is nothing to show yet.
    pub waiting: bool,
    /// There is, and a newer reading is on its way.
    pub stale: bool,
    pub gave_up: bool,
}

impl TxReading {
    fn of(answer: Answer<ExplainedTx>, bare: ExplainedTx) -> Self {
        let Answer { worked, stale } = answer;
        let (explained, waiting, gave_up) = match worked {
            Worked::Done(explained) => (explained, false, false),
            Worked::Waiting => (bare, true, false),
            Worked::GaveUp => (bare, false, true),
        };
        Self {
            explained,
            waiting,
            stale,
            gave_up,
        }
    }

    /// Nothing should be decided on what it says yet.
    pub fn unsettled(&self) -> bool {
        self.waiting || self.stale
    }
}

type TxKey = (
    DecodedTx,
    Option<TxContext>,
    Vec<VectorContext>,
    Vec<WalletSummary>,
    Fetched,
);

type SiteKey = (Option<DecodedTx>, Vec<VectorContext>, Fetched);

/// Account bytes, by address, that the node gave for what a reading's
/// sentences read, such as the mint whose decimals turn a raw amount into
/// "Mint 5 USDC". A shell with no network asks nothing, and those sentences
/// show their raw fields.
pub type Fetched = HashMap<Pubkey, Vec<u8>>;

/// What the node gave a reading, for the sentences drawn under it: an amount
/// scaled with an account in it was read from this device's own node.
#[derive(Clone, Copy)]
pub struct FetchedForReading(pub Signal<Fetched>);

fn use_display_accounts() -> Signal<Fetched> {
    let fetched = use_signal(Fetched::new);
    use_context_provider(|| FetchedForReading(fetched));
    fetched
}

/// Ask the node, once each, for the accounts `answer` says its sentences read
/// and that were not asked for yet. What comes back goes into `fetched`, which
/// is part of the reading's key, so the transaction is read again with it.
fn use_fetch_needed(mut fetched: Signal<Fetched>, answer: &Answer<ExplainedTx>) {
    // A window without the network capability, such as a test's, asks nothing.
    let network = try_use_context::<Option<Network>>().flatten();
    let mut asked = use_signal(HashSet::<Pubkey>::new);
    let needed = match &answer.worked {
        Worked::Done(explained) => explained.accounts_needed(),
        _ => Vec::new(),
    };
    use_effect(use_reactive!(|(needed)| {
        let Some(network) = network else { return };
        let needed: Vec<(Pubkey, Pubkey)> = needed
            .into_iter()
            .filter(|(address, _)| !asked.peek().contains(address))
            .collect();
        if needed.is_empty() {
            return;
        }
        let Ok(rpc) = network.rpc() else { return };
        asked
            .write()
            .extend(needed.iter().map(|(address, _)| *address));
        spawn(async move {
            let addresses: Vec<Pubkey> = needed.iter().map(|(address, _)| *address).collect();
            // What the node cannot give stays out, and its sentence shows the
            // raw fields.
            let Ok(accounts) = rpc.get_accounts(&addresses).await else {
                return;
            };
            let found = display_accounts(&needed, &accounts);
            if !found.is_empty() {
                fetched.write().extend(found);
            }
        });
    }));
}

/// `note` with `fetched` added. Where both have an account, this device's
/// own node is taken over what another device wrote into the note.
fn with_fetched(mut note: DisplayNote, fetched: &Fetched) -> DisplayNote {
    note.accounts.extend(
        fetched
            .iter()
            .map(|(address, data)| (*address, data.clone())),
    );
    note
}

/// How the Sign screen reads `tx`. What an Advance does to a hash-based key is read
/// with the accounts' schemes: from a context note, a rebuild, or a Vector wallet in
/// this vault. A pasted or scanned transaction brings only the wallets'. Another
/// reading of the same message may stand until it comes, as its bytes are the same.
pub fn use_explained(tx: DecodedTx) -> TxReading {
    let session = use_context::<Session>();
    let stored = (session.forms.sign.context)();
    let vector = (session.forms.sign.vector)();
    let wallets = (session.wallets)().summaries.clone();
    let fetched = use_display_accounts();
    let for_bare = tx.clone();
    let bare = use_memo(use_reactive!(|(for_bare)| {
        explain_transaction(&NoIdls, &for_bare)
    }))();
    let answer = use_worked_out::<TxKey, ExplainedTx>(
        (tx, stored, vector, wallets, fetched()),
        READING_DEADLINE,
        |asked, now| asked.0.message == now.0.message,
        |view, (tx, stored, vector, wallets, fetched)| {
            let note = display_note(tx, stored.as_ref())
                .with_vector_schemes(vector)
                .with_wallet_schemes(wallets);
            explain_with_note(view, tx, &with_fetched(note, fetched))
        },
    );
    use_fetch_needed(fetched, &answer);
    TxReading::of(answer, bare)
}

/// How History reads `tx`, a transaction that already ran: with what the
/// wallets say of their own accounts, and nothing a context note brought.
pub fn use_explained_landed(tx: DecodedTx) -> TxReading {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    let fetched = use_display_accounts();
    let for_bare = tx.clone();
    let bare = use_memo(use_reactive!(|(for_bare)| {
        explain_transaction(&NoIdls, &for_bare)
    }))();
    let answer = use_worked_out::<(DecodedTx, Vec<WalletSummary>, Fetched), ExplainedTx>(
        (tx, wallets, fetched()),
        READING_DEADLINE,
        |asked, now| asked.0.message == now.0.message,
        |view, (tx, wallets, fetched)| {
            let note = DisplayNote::default().with_wallet_schemes(wallets);
            explain_with_note(view, tx, &with_fetched(note, fetched))
        },
    );
    use_fetch_needed(fetched, &answer);
    TxReading::of(answer, bare)
}

/// How the request modal reads `tx`, with what a rebuild knew of its Vector
/// accounts. `None` with no transaction.
pub fn use_explained_for_site(
    tx: Option<DecodedTx>,
    schemes: Vec<VectorContext>,
) -> Option<TxReading> {
    let fetched = use_display_accounts();
    let for_bare = tx.clone();
    let bare = use_memo(use_reactive!(|(for_bare)| {
        for_bare.as_ref().map(|tx| explain_transaction(&NoIdls, tx))
    }))();
    let answer = use_worked_out::<SiteKey, Option<ExplainedTx>>(
        (tx, schemes, fetched()),
        READING_DEADLINE,
        // What the node gave since does not make it another request.
        |asked, now| asked.0 == now.0 && asked.1 == now.1,
        |view, (tx, schemes, fetched)| {
            tx.as_ref().map(|tx| {
                let note = DisplayNote::default().with_vector_schemes(schemes);
                explain_with_note(view, tx, &with_fetched(note, fetched))
            })
        },
    );
    let worked = match answer.worked {
        Worked::Done(Some(explained)) => Worked::Done(explained),
        Worked::Done(None) | Worked::Waiting => Worked::Waiting,
        Worked::GaveUp => Worked::GaveUp,
    };
    let answer = Answer {
        worked,
        stale: answer.stale,
    };
    use_fetch_needed(fetched, &answer);
    Some(TxReading::of(answer, bare?))
}

/// Says that a reading is under way, or took too long.
#[component]
pub fn ReadingNote(waiting: bool, gave_up: bool) -> Element {
    rsx! {
        if waiting {
            p { class: "muted", role: "status", "Reading it…" }
        }
        if gave_up {
            p { class: "warn", role: "alert",
                "Reading this took too long, so it is shown as its programs and bytes, not in words. A program's IDL can be written to make this happen."
            }
        }
    }
}

/// `instructions` that are not a transaction's own, read away from the window.
/// `risks` also says what they do that hands over control.
#[component]
pub fn ReadInstructions(
    instructions: Vec<Instruction>,
    inspect: bool,
    #[props(default)] risks: bool,
) -> Element {
    let for_bare = instructions.clone();
    let bare = use_memo(use_reactive!(|(for_bare)| {
        explain_instructions(&NoIdls, &for_bare)
    }))();
    let answer = use_worked_out::<Vec<Instruction>, Vec<ExplainedIx>>(
        instructions,
        READING_DEADLINE,
        |asked, now| asked == now,
        |view, instructions| explain_instructions(view, instructions),
    );
    let (shown, waiting, gave_up) = match answer.worked {
        Worked::Done(explained) => (explained, false, false),
        Worked::Waiting => (bare, true, false),
        Worked::GaveUp => (bare, false, true),
    };
    rsx! {
        ReadingNote { waiting, gave_up }
        // They run in a transaction of their own, so a check among them pins
        // no vote of this one.
        if risks {
            for risk in wallet_core::risks_told_carried(&shown) {
                p { key: "{risk}", class: "warn", role: "alert", "{risk}" }
            }
        }
        if !waiting {
            InstructionList { instructions: shown, inspect }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::session::OnlineHooks;
    use crate::state::submit::SubmitHandle;
    use crate::state::use_session;
    use app_core::testing::{kit, Kit};
    use dioxus::dioxus_core::{NoOpMutations, ScopeId, VirtualDom};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone)]
    struct Held {
        core: app_core::Core,
        /// What the window has now.
        seen: Rc<RefCell<Answer<u32>>>,
        session: Rc<RefCell<Option<Session>>>,
        key: Rc<RefCell<Option<Signal<u32>>>>,
    }

    /// How long the window in these tests waits. Long enough that a machine
    /// that is busy does not give up on what is quick.
    const WAITS: Duration = Duration::from_millis(2000);

    /// A key above 1000 takes longer than the window waits. Key 999 takes long
    /// enough to look at what stands meanwhile, and less than the window waits.
    fn work(_: &CatalogView, key: &u32) -> u32 {
        if *key > 1000 {
            std::thread::sleep(WAITS + Duration::from_millis(400));
        }
        if *key == 999 {
            std::thread::sleep(Duration::from_millis(600));
        }
        key * 2
    }

    fn app(held: Held) -> Element {
        let core = use_hook(|| CopyValue::new(held.core.clone()));
        let hooks = OnlineHooks::new(
            || Err(wallet_core::RpcError::Offline),
            || {},
            || {},
            || Box::pin(async { Ok(String::new()) }),
        );
        let session = use_session(core, hooks, SubmitHandle::none());
        use_context_provider(|| session);
        *held.session.borrow_mut() = Some(session);
        let key = use_signal(|| 1u32);
        *held.key.borrow_mut() = Some(key);
        *held.seen.borrow_mut() =
            use_worked_out::<u32, u32>(key(), WAITS, |asked, now| asked == now, work);
        rsx! {}
    }

    struct Window {
        dom: VirtualDom,
        seen: Rc<RefCell<Answer<u32>>>,
        session: Rc<RefCell<Option<Session>>>,
        key: Signal<u32>,
        _kit: Kit,
    }

    impl Window {
        fn new() -> Self {
            let kit = kit();
            let seen = Rc::new(RefCell::new(Answer {
                worked: Worked::Waiting,
                stale: false,
            }));
            let session = Rc::new(RefCell::new(None));
            let key = Rc::new(RefCell::new(None));
            let mut dom = VirtualDom::new_with_props(
                app,
                Held {
                    core: kit.core.clone(),
                    seen: seen.clone(),
                    session: session.clone(),
                    key: key.clone(),
                },
            );
            dom.rebuild_in_place();
            let key = key.borrow().unwrap();
            Self {
                dom,
                seen,
                session,
                key,
                _kit: kit,
            }
        }

        /// Let things run for `millis`.
        async fn settle(&mut self, millis: u64) {
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_millis(millis) {
                self.turn().await;
            }
        }

        async fn turn(&mut self) {
            let _ = tokio::time::timeout(Duration::from_millis(5), self.dom.wait_for_work()).await;
            self.dom.render_immediate(&mut NoOpMutations);
        }

        /// Let things run until the window has `wanted`, however busy the machine.
        async fn until(&mut self, wanted: Worked<u32>, stale: bool) {
            let start = std::time::Instant::now();
            while (self.seen(), self.stale()) != (wanted.clone(), stale) {
                assert!(
                    start.elapsed() < Duration::from_secs(30),
                    "never got {wanted:?}, stale {stale}: has {:?}",
                    self.seen.borrow()
                );
                self.turn().await;
            }
        }

        fn seen(&self) -> Worked<u32> {
            self.seen.borrow().worked.clone()
        }

        fn stale(&self) -> bool {
            self.seen.borrow().stale
        }

        fn set(&mut self, value: u32) {
            let mut key = self.key;
            self.dom.in_scope(ScopeId::ROOT, || key.set(value));
            self.dom.render_immediate(&mut NoOpMutations);
        }
    }

    #[tokio::test]
    async fn the_window_has_what_was_worked_out_for_what_is_on_it_and_nothing_else() {
        let mut window = Window::new();
        assert_eq!(window.seen(), Worked::Waiting);
        window.until(Worked::Done(2), false).await;

        // What came of the last key is not shown for this one.
        window.set(3);
        assert_eq!(window.seen(), Worked::Waiting);
        window.until(Worked::Done(6), false).await;
    }

    #[tokio::test]
    async fn what_takes_too_long_is_given_up_on_and_the_next_thing_is_read_anyway() {
        let mut window = Window::new();
        window.until(Worked::Done(2), false).await;

        window.set(2000);
        assert_eq!(window.seen(), Worked::Waiting);
        window.until(Worked::GaveUp, false).await;

        // It does not come back, even once it has finished.
        window.settle(600).await;
        assert_eq!(window.seen(), Worked::GaveUp);

        window.set(4);
        window.until(Worked::Done(8), false).await;
    }

    #[tokio::test]
    async fn a_late_answer_stays_off_what_took_its_place() {
        let mut window = Window::new();
        window.until(Worked::Done(2), false).await;

        // Under way, and slow enough to finish after the next one has.
        window.set(1500);
        window.settle(100).await;
        window.set(5);
        window.until(Worked::Done(10), false).await;
        window.settle(2600).await;
        assert_eq!(window.seen(), Worked::Done(10));
    }

    #[tokio::test]
    async fn a_window_under_test_keeps_away_from_the_real_data_folder() {
        let _window = Window::new();
        let folder = crate::platform::storage::data_dir().unwrap();
        assert!(
            folder.starts_with(std::env::temp_dir()),
            "{}",
            folder.display()
        );
    }

    #[tokio::test]
    async fn an_answer_read_before_an_idl_arrived_stands_but_is_marked_until_the_new_one_comes() {
        let mut window = Window::new();
        window.set(999);
        window.until(Worked::Done(1998), false).await;

        let session = window.session.borrow().unwrap();
        let idl = br#"{"name":"tiny","instructions":[]}"#;
        window.dom.in_scope(ScopeId::ROOT, || {
            session
                .import_idl(idl, Some(Pubkey::new_from_array([9; 32])))
                .unwrap();
        });
        // The new reading takes a while. Nothing is decided on the old one meanwhile.
        window.turn().await;
        assert_eq!((window.seen(), window.stale()), (Worked::Done(1998), true));

        window.until(Worked::Done(1998), false).await;
    }

    #[tokio::test]
    async fn the_real_catalog_reads_a_transaction_and_says_what_it_can_without_it() {
        use wallet_core::{compile_v1, decode_v1_transaction, explain_transaction};
        use wallet_flows::testing::{pk, transfer_instruction};

        let wire = compile_v1(
            pk(1),
            vec![transfer_instruction(pk(1), pk(2))],
            None,
            Some(solana_hash::Hash::new_from_array([3; 32])),
        )
        .unwrap()
        .wire;
        let tx = decode_v1_transaction(&wire).unwrap();
        // Without any IDL, nothing is in words, and every instruction is there.
        let bare = explain_transaction(&NoIdls, &tx);
        assert_eq!(bare.instructions.len(), tx.instructions.len());
        assert!(bare.instructions.iter().all(|ix| ix.idl.is_none()));
        assert_eq!(
            bare.instructions[0].program_id,
            tx.instructions[0].program_id
        );
    }

    #[test]
    fn a_reading_says_how_it_stands_and_shows_the_bare_one_until_it_has_words() {
        let bare = ExplainedTx {
            programs: Vec::new(),
            instructions: Vec::new(),
        };
        let words = ExplainedTx {
            programs: vec![],
            instructions: vec![ExplainedIx {
                program_id: Pubkey::new_from_array([1; 32]),
                accounts: Vec::new(),
                data: Vec::new(),
                idl: None,
                inner: None,
            }],
        };
        let of = |worked, stale| TxReading::of(Answer { worked, stale }, bare.clone());

        let read = of(Worked::Done(words.clone()), false);
        assert_eq!(read.explained, words);
        assert!(!read.waiting && !read.gave_up && !read.unsettled());

        // Words from before a new IDL stand, and nothing is decided on them.
        let stale = of(Worked::Done(words.clone()), true);
        assert_eq!(stale.explained, words);
        assert!(!stale.waiting && stale.unsettled());

        let waiting = of(Worked::Waiting, false);
        assert_eq!(waiting.explained, bare);
        assert!(waiting.waiting && waiting.unsettled());

        let gave_up = of(Worked::GaveUp, false);
        assert_eq!(gave_up.explained, bare);
        assert!(gave_up.gave_up && !gave_up.unsettled());
    }
}
