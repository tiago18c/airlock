//! Checked and deferred saving, through the public API and the test doubles.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_core::testing::{kit, settle};
use app_core::{notice_row_text, Event, NoticeKind, SAVE_INTERVAL};
use solana_pubkey::Pubkey;
use tracing_subscriber::fmt::MakeWriter;
use wallet_core::{DiscoveredTokenAccount, Holdings};

fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

fn token(owner: Pubkey, amount: u64) -> DiscoveredTokenAccount {
    DiscoveredTokenAccount {
        pubkey: pk(3),
        owner,
        mint: pk(4),
        amount,
        decimals: 6,
    }
}

/// Collects what `tracing` writes while it is the default subscriber.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

#[tokio::test]
async fn checked_save_failure_rolls_back() {
    let kit = kit();
    let slices = kit.core.slices();
    let book = slices.book.subscribe();
    let network = slices.network.subscribe();
    kit.storage.refuse_saves(true);

    let log = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    let result = tracing::subscriber::with_default(subscriber, || {
        kit.core.workspace_checked("add_contact", |ws| {
            ws.book.insert_contact(pk(1), "Alice");
            ws.close_to_tray = true;
        })
    });

    assert!(result.is_err(), "the command returns the save's error");
    assert!(!book.has_changed().unwrap(), "nothing was published");
    assert!(!network.has_changed().unwrap(), "nothing was published");
    assert!(slices.book.get().get(&pk(1)).is_none());
    assert!(!slices.settings.get().close_to_tray);
    assert!(kit.storage.read("workspace.json").is_none());

    let text = log.text();
    assert!(text.contains("WARN"), "{text}");
    assert!(text.contains("airlock::core"), "{text}");
    assert!(text.contains("save failed"), "{text}");
    assert!(
        text.contains("add_contact"),
        "the command's span names it: {text}"
    );

    // And once saves work, the same command goes through and is published.
    kit.storage.refuse_saves(false);
    kit.core
        .workspace_checked("add_contact", |ws| {
            ws.book.insert_contact(pk(1), "Alice");
        })
        .unwrap();
    assert!(book.has_changed().unwrap());
    assert_eq!(slices.book.get().get(&pk(1)).unwrap().label, "Alice");
    assert!(kit.storage.read("workspace.json").is_some());
}

#[tokio::test]
async fn deferred_saves_coalesce() {
    let kit = kit();
    let owner = pk(2);

    // 100 updates over 10 seconds.
    for i in 1..=100u64 {
        kit.core.holdings_deferred("apply_balances", |holdings| {
            holdings.replace_owners(&[owner], &[token(owner, i)]);
        });
        settle().await;
        kit.clock.advance(Duration::from_millis(100));
        settle().await;
    }
    kit.clock.advance(SAVE_INTERVAL);
    settle().await;

    let writes = kit.storage.writes("holdings.json");
    assert!(
        (1..=11).contains(&writes),
        "100 updates over 10 s wrote {writes} times"
    );
    // The slice had every update at once; the file has the last.
    assert_eq!(
        kit.core.slices().holdings.get().page(&owner, 0, 10)[0].amount,
        100
    );
    let bytes = kit.storage.read("holdings.json").unwrap();
    let on_disk: Holdings = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(on_disk.page(&owner, 0, 10)[0].amount, 100);
}

#[tokio::test]
async fn deferred_save_failure_keeps_change() {
    let kit = kit();
    let owner = pk(2);
    let mut events = kit.core.events();
    kit.storage.refuse_saves(true);

    kit.core.holdings_deferred("apply_balances", |holdings| {
        holdings.replace_owners(&[owner], &[token(owner, 7)]);
    });
    settle().await;
    kit.clock.advance(SAVE_INTERVAL);
    settle().await;

    assert_eq!(
        kit.core.slices().holdings.get().page(&owner, 0, 10)[0].amount,
        7
    );
    match events.try_recv() {
        Ok(Event::SaveFailed { store, error }) => {
            assert_eq!(store, "holdings");
            assert!(error.contains("refused"), "{error}");
        }
        other => panic!("expected SaveFailed, got {other:?}"),
    }
    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), 1);
    match &notices.items()[0].kind {
        NoticeKind::SaveFailed { store, error } => {
            assert_eq!(store, "holdings");
            assert!(error.contains("refused"), "{error}");
        }
        other => panic!("expected SaveFailed notice, got {other:?}"),
    }
    assert_eq!(
        notice_row_text(&notices.items()[0]),
        format!(
            "Save failed · holdings: {}",
            match &notices.items()[0].kind {
                NoticeKind::SaveFailed { error, .. } => error.as_str(),
                _ => unreachable!(),
            }
        )
    );
    assert!(kit.storage.read("holdings.json").is_none());

    // Nothing was lost: a flush after the disk recovers writes it.
    kit.storage.refuse_saves(false);
    kit.core.flush();
    let bytes = kit.storage.read("holdings.json").unwrap();
    let on_disk: Holdings = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(on_disk.page(&owner, 0, 10)[0].amount, 7);
}

#[tokio::test]
async fn a_failed_flush_is_a_notice_at_once_even_from_a_thread_with_no_runtime() {
    let kit = kit();
    let owner = pk(2);
    let mut events = kit.core.events();
    kit.storage.refuse_saves(true);
    kit.core.holdings_deferred("apply_balances", |holdings| {
        holdings.replace_owners(&[owner], &[token(owner, 7)]);
    });

    // The bridge's threads have no tokio runtime.
    let core = kit.core.clone();
    std::thread::spawn(move || core.flush()).join().unwrap();

    let notices = kit.core.slices().notices.get();
    assert_eq!(notices.len(), 1, "one notice, for the holdings");
    assert!(matches!(
        &notices.items()[0].kind,
        NoticeKind::SaveFailed { store, .. } if store == "holdings"
    ));
    // The notice made the notices dirty, and their save failed too: that is
    // an event, but not a notice of its own.
    let stores: Vec<&str> = std::iter::from_fn(|| match events.try_recv() {
        Ok(Event::SaveFailed { store, .. }) => Some(store),
        _ => None,
    })
    .collect();
    assert_eq!(stores, ["holdings", "notices"]);
}

#[tokio::test]
async fn dropping_the_core_with_a_failed_save_only_sends_the_event() {
    let kit = kit();
    let owner = pk(2);
    let mut events = kit.core.events();
    kit.storage.refuse_saves(true);
    kit.core.holdings_deferred("apply_balances", |holdings| {
        holdings.replace_owners(&[owner], &[token(owner, 7)]);
    });

    drop(kit.core);

    match events.try_recv() {
        Ok(Event::SaveFailed { store, .. }) => assert_eq!(store, "holdings"),
        other => panic!("expected SaveFailed, got {other:?}"),
    }
    // No notice was pushed, so the notices were not saved either.
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
}

#[tokio::test]
async fn deferred_commands_and_saves_log_what_they_did() {
    let kit = kit();
    let log = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let id = kit.core.notices_push(NoticeKind::VaultRecovered);
        kit.core.notices_mark_read(&[id]);
        kit.core.notices_mark_read(&[id]);
        kit.core.notices_dismiss_toast(id);
        kit.core.notices_remove(id);
        kit.core.notices_remove(id);
        kit.core.notices_dismiss_toast(id);
        kit.core.history_deferred("history_nothing", |_| false);
        kit.core
            .public()
            .workspace_deferred("set_rpc", |workspace| {
                workspace.close_to_tray = true;
            });
        kit.core.flush();
        kit.storage.refuse_saves(true);
        kit.core.notices_clear();
        kit.core.flush();
    });
    let text = log.text();
    let expected = "\
DEBUG command{name=\"notices_push\"}: airlock::core: command outcome=\"ok\"
DEBUG command{name=\"notices_mark_read\"}: airlock::core: command outcome=\"ok\"
DEBUG command{name=\"notices_mark_read\"}: airlock::core: command outcome=\"unchanged\"
DEBUG command{name=\"notices_dismiss_toast\"}: airlock::core: command outcome=\"ok\"
DEBUG command{name=\"notices_remove\"}: airlock::core: command outcome=\"ok\"
DEBUG command{name=\"notices_remove\"}: airlock::core: command outcome=\"unchanged\"
DEBUG command{name=\"notices_dismiss_toast\"}: airlock::core: command outcome=\"unknown id\"
DEBUG command{name=\"history_nothing\"}: airlock::core: command outcome=\"unchanged\"
DEBUG command{name=\"set_rpc\"}: airlock::core: command outcome=\"ok\"
DEBUG airlock::core: save store=\"workspace\" policy=\"deferred\" result=\"ok\"
DEBUG airlock::core: save store=\"notices\" policy=\"deferred\" result=\"ok\"
DEBUG command{name=\"notices_clear\"}: airlock::core: command outcome=\"ok\"
 WARN airlock::core: save failed; the change is kept in memory store=\"notices\" policy=\"deferred\" error=";
    assert!(text.starts_with(expected), "{text}");
    assert_eq!(text.lines().count(), 13, "{text}");
}
