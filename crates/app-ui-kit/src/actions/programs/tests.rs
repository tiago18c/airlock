//! What the Programs pane's actions say, and its file dialogs through the
//! real session, on a window with no screen drawn: each dialog is awaited
//! by a task after the click that asked for it returns, one at a time.
//!
//! No test here reaches the crash these dialogs once caused: it took a
//! native dialog's own message loop rendering the window from inside an
//! event handler, which only the desktop app has.

use super::*;
use crate::state::session::OnlineHooks;
use crate::state::submit::SubmitHandle;
use crate::state::use_session;
use app_core::testing::{kit, Kit};
use app_core::Core;
use dioxus::dioxus_core::{NoOpMutations, ScopeId, VirtualDom};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

fn checked(same: bool, source: bool) -> Result<IdlCheck, String> {
    Ok(IdlCheck {
        remote_sha256: None,
        same,
        source,
    })
}

#[test]
fn a_run_of_checks_says_how_many_matched_and_why_the_last_failure_failed() {
    let mut tally = CheckTally::default();
    tally.count(checked(true, false));
    tally.count(checked(true, false));
    tally.count(checked(false, false));
    assert_eq!(
        tally.sentence(),
        "2 match, 1 differ. Stored files were not replaced."
    );

    tally.count(Err::<IdlCheck, _>("first"));
    tally.count(Err::<IdlCheck, _>("Offline"));
    assert_eq!(
        tally.sentence(),
        "2 match, 1 differ, 2 failed (Offline). Stored files were not replaced."
    );
}

/// A file made from what the chain publishes is current, not different.
#[test]
fn a_file_whose_source_the_chain_publishes_is_counted_apart() {
    let mut tally = CheckTally::default();
    tally.count(checked(true, false));
    tally.count(checked(false, true));
    assert_eq!(
        tally.sentence(),
        "1 match, 1 match their source, 0 differ. Stored files were not replaced."
    );
    let check = checked(false, true).unwrap();
    assert!(check_sentence("raydiumClmm", &check).contains("made from"));
}

/// A running window, with a vault, no node and no screen drawn.
struct Screen {
    _kit: Kit,
    dom: VirtualDom,
    session: Session,
}

#[derive(Clone)]
struct Held {
    core: Core,
    session: Rc<Cell<Option<Session>>>,
}

fn app(held: Held) -> Element {
    let core = use_hook(|| CopyValue::new(held.core.clone()));
    let session = use_session(core, OnlineHooks::none(), SubmitHandle::none());
    held.session.set(Some(session));
    rsx! {}
}

impl Screen {
    async fn new() -> Self {
        let kit = kit();
        kit.core.create_vault("pw").unwrap();
        let session = Rc::new(Cell::new(None));
        let mut dom = VirtualDom::new_with_props(
            app,
            Held {
                core: kit.core.clone(),
                session: session.clone(),
            },
        );
        dom.rebuild_in_place();
        let session = session.get().unwrap();
        let mut screen = Self {
            _kit: kit,
            dom,
            session,
        };
        screen.settle().await;
        screen
    }

    fn act<R>(&self, then: impl FnOnce() -> R) -> R {
        self.dom.in_scope(ScopeId::ROOT, then)
    }

    async fn settle(&mut self) {
        for _ in 0..4 {
            let _ = tokio::time::timeout(Duration::from_millis(40), self.dom.wait_for_work()).await;
            self.dom.render_immediate(&mut NoOpMutations);
        }
    }

    /// Runs the window until the dialog's task has let go of the flag, or
    /// fails after a deadline far past what any runner takes. A fixed number
    /// of short waits is not enough on a loaded CI runner: the task that
    /// reads the file may not have run by the last of them.
    async fn returned(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while self.dialog_open() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the dialog's task never returned"
            );
            self.settle().await;
        }
    }

    /// The pane's note and error.
    fn pane(&self) -> (Signal<Option<String>>, Signal<Option<String>>) {
        self.act(|| (Signal::new(None), Signal::new(None)))
    }

    fn idl_records_of_file(&self) -> usize {
        self.act(|| {
            self.session
                .idl_records()
                .into_iter()
                .filter(|record| record.origin == wallet_core::IdlOrigin::File)
                .count()
        })
    }

    fn dialog_open(&self) -> bool {
        self.act(|| *self.session.forms.program.file_dialog.peek())
    }

    fn said(&self, signal: Signal<Option<String>>) -> Option<String> {
        self.act(|| signal.peek().clone())
    }

    fn selected(&self) -> Option<Pubkey> {
        self.act(|| *self.session.forms.program.selected.peek())
    }

    /// What an import that replaces a shipped IDL waits on, by name.
    fn pending(&self) -> Option<Vec<String>> {
        self.act(|| {
            self.session
                .forms
                .program
                .pending_import
                .peek()
                .as_ref()
                .map(|held| held.replaces.clone())
        })
    }

    /// Say yes to the import waiting on the user.
    fn replace(&self, note: Signal<Option<String>>, error: Signal<Option<String>>) {
        self.act(|| confirm_import(&self.session, true, note, error));
    }

    fn memo_id(&self) -> Pubkey {
        self.act(|| {
            self.session
                .idl_records()
                .into_iter()
                .find(|record| {
                    record.name == "memo" && record.origin == wallet_core::IdlOrigin::File
                })
                .map(|record| record.program_id)
                .expect("memo imported")
        })
    }
}

const REPLACED: &str = "Imported memo. It replaces the IDL that ships with Airlock for ";

const MEMO: &[u8] = include_bytes!("../../../../../resources/idls/memo.json");

type Picked<T> = Option<Result<T, IdlFilesError>>;

/// A dialog the test answers: what it gives, once sent.
fn dialog<T: 'static>() -> (
    tokio::sync::oneshot::Sender<Result<T, IdlFilesError>>,
    impl std::future::Future<Output = Picked<T>> + 'static,
) {
    let (answer, answered) = tokio::sync::oneshot::channel();
    (answer, async move { answered.await.ok() })
}

/// A dialog that must never open: it says so if it is awaited.
async fn never<T: 'static>(opened: Rc<Cell<bool>>) -> Picked<T> {
    opened.set(true);
    None
}

#[tokio::test]
async fn a_file_is_imported_after_the_click_that_asked_for_it_returns() {
    let mut screen = Screen::new().await;
    let (note, error) = screen.pane();
    let (answer, picked) = dialog::<Vec<u8>>();
    screen.act(|| import_one_from(screen.session, note, error, picked));
    assert!(screen.dialog_open(), "held from the click on");
    screen.settle().await;
    assert_eq!(screen.said(note), None, "nothing is read while it is open");
    assert_eq!(screen.selected(), None);
    assert!(screen.dialog_open(), "held while it is open");

    // Asking again while one is open opens nothing, from any of the three.
    let opened = Rc::new(Cell::new(false));
    screen.act(|| {
        import_one_from(screen.session, note, error, never(opened.clone()));
        import_folder_from(screen.session, note, error, never(opened.clone()));
        export_to(screen.session, note, error, Vec::new(), |_| {
            never::<std::path::PathBuf>(opened.clone())
        });
    });
    screen.settle().await;
    assert!(!opened.get(), "a second dialog opened");

    answer.send(Ok(MEMO.to_vec())).unwrap();
    screen.returned().await;
    // Memo's IDL ships with the app: the import waits for the user's yes,
    // naming it.
    assert_eq!(screen.said(note), None);
    let waiting = screen
        .pending()
        .expect("asks before replacing a shipped IDL");
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert_eq!(screen.idl_records_of_file(), 0, "nothing imported yet");
    screen.replace(note, error);
    let said = screen.said(note).unwrap();
    assert!(said.starts_with(REPLACED), "{said}");
    assert!(said.ends_with(&format!("{}.", waiting[0])), "{said}");
    assert_eq!(screen.pending(), None);
    assert_eq!(screen.said(error), None);
    assert_eq!(screen.selected(), Some(screen.memo_id()));
    assert!(!screen.dialog_open(), "let go once it returns");

    // A cancelled dialog changes nothing and lets go too.
    let (answer, picked) = dialog::<Vec<u8>>();
    screen.act(|| import_one_from(screen.session, note, error, picked));
    drop(answer);
    screen.returned().await;
    assert!(screen.said(note).unwrap().starts_with(REPLACED));
    assert_eq!(screen.pending(), None);
    assert!(!screen.dialog_open());

    // One that cannot be read says why, in place of the note.
    let (answer, picked) = dialog::<IdlFiles>();
    screen.act(|| import_folder_from(screen.session, note, error, picked));
    answer.send(Err(IdlFilesError::NoIndex)).unwrap();
    screen.returned().await;
    assert_eq!(screen.said(note), None);
    assert_eq!(
        screen.said(error).as_deref(),
        Some("That folder has no index.json.")
    );
    assert!(!screen.dialog_open());
}

#[tokio::test]
async fn an_export_says_where_it_went_once_its_dialog_returns() {
    let mut screen = Screen::new().await;
    let (note, error) = screen.pane();
    let files = vec![
        ("index.json".to_string(), b"{}".to_vec()),
        ("devnet.memo.json".to_string(), MEMO.to_vec()),
    ];
    let (answer, picked) = dialog::<std::path::PathBuf>();
    let handed = Rc::new(Cell::new(0));
    screen.act(|| {
        let handed = handed.clone();
        export_to(screen.session, note, error, files, move |files| {
            handed.set(files.len());
            picked
        })
    });
    screen.settle().await;
    assert_eq!(handed.get(), 2, "every file goes to the dialog's task");
    assert!(screen.dialog_open());
    assert_eq!(screen.said(note), None);

    answer.send(Ok(std::path::PathBuf::from("out"))).unwrap();
    screen.returned().await;
    assert_eq!(screen.said(note).as_deref(), Some("Saved 2 files to out."));
    assert!(!screen.dialog_open());
}

/// The pane is left while its dialog is open, taking its note and error
/// with it: the file is still imported, the flag let go, and nothing panics
/// on the signals that are gone.
#[tokio::test]
async fn an_import_finishes_when_the_pane_is_left_while_its_dialog_is_open() {
    let mut screen = Screen::new().await;
    let (note, error) = screen.pane();
    let (answer, picked) = dialog::<Vec<u8>>();
    screen.act(|| import_one_from(screen.session, note, error, picked));
    screen.settle().await;
    screen.act(|| {
        note.manually_drop();
        error.manually_drop();
    });

    answer.send(Ok(MEMO.to_vec())).unwrap();
    screen.returned().await;
    assert!(!screen.dialog_open());
    // The question waits for the pane, and is answered with its new note.
    assert!(screen.pending().is_some());
    let (note, error) = screen.pane();
    screen.replace(note, error);
    assert_eq!(screen.selected(), Some(screen.memo_id()));

    // So does one that fails, and an export.
    let (answer, picked) = dialog::<IdlFiles>();
    screen.act(|| import_folder_from(screen.session, note, error, picked));
    answer.send(Err(IdlFilesError::NoIndex)).unwrap();
    screen.returned().await;
    assert!(!screen.dialog_open());
    let (answer, picked) = dialog::<std::path::PathBuf>();
    screen.act(|| export_to(screen.session, note, error, Vec::new(), |_| picked));
    answer.send(Ok(std::path::PathBuf::from("out"))).unwrap();
    screen.returned().await;
    assert!(!screen.dialog_open());
}
