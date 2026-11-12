//! What commands and saves log, read back from `tracing`.
//!
//! These tests have a binary of their own because of how `tracing` decides
//! whether a log line has a reader. It decides once for the whole process,
//! on the thread that reaches the line first, and keeps the answer. In a
//! binary shared with other tests, another test's thread reaches a line
//! first with no subscriber of its own, answers "none", and may store that
//! answer after the test reading the log has started: the line is then
//! never written, and the test fails now and then. Here every test calls
//! [`Capture::start`] before it does anything else, which gives the process
//! a subscriber that takes every line, so no thread can answer "none".

use std::io;
use std::sync::{Arc, Mutex, Once};

use app_core::testing::kit;
use app_core::NoticeKind;
use solana_pubkey::Pubkey;
use tracing_subscriber::fmt::MakeWriter;

/// Collects what `tracing` writes while its subscriber is the default.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    /// A new collector. The first one made also gives the process a
    /// subscriber that takes every line and writes none, so every line has
    /// a reader from then on. Call it first in a test.
    fn start() -> Self {
        static EVERY_LINE_HAS_A_READER: Once = Once::new();
        EVERY_LINE_HAS_A_READER.call_once(|| {
            tracing::subscriber::set_global_default(tracing_subscriber::registry())
                .expect("no test sets a global subscriber of its own");
        });
        Self::default()
    }

    /// Writes into this collector, down to debug lines, with no times.
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        tracing_subscriber::fmt()
            .with_writer(self.clone())
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::DEBUG)
            .finish()
    }

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
async fn a_checked_save_that_fails_warns_and_names_its_command() {
    let log = Capture::start();
    let kit = kit();
    kit.storage.refuse_saves(true);

    let result = tracing::subscriber::with_default(log.subscriber(), || {
        kit.core.workspace_checked("add_contact", |ws| {
            ws.book
                .insert_contact(Pubkey::new_from_array([1; 32]), "Alice");
        })
    });

    assert!(result.is_err(), "the command returns the save's error");
    let text = log.text();
    assert!(text.contains("WARN"), "{text}");
    assert!(text.contains("airlock::core"), "{text}");
    assert!(text.contains("save failed"), "{text}");
    assert!(
        text.contains("add_contact"),
        "the command's span names it: {text}"
    );
}

#[tokio::test]
async fn deferred_commands_and_saves_log_what_they_did() {
    let log = Capture::start();
    let kit = kit();
    tracing::subscriber::with_default(log.subscriber(), || {
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
