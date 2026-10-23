//! Test doubles for the ports: a manual clock, a memory `Storage` that can be
//! made to fail, and recorders. For the tests of this crate and of the crates
//! that run on the core.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::oneshot;
use wallet_core::{VaultError, VaultStore};

use crate::commands::Core;
use crate::ports::{BoxFuture, Clock, Links, Notification, Notifier, Ports, Storage};
use crate::runtime::TokioSpawner;

/// A core on test doubles, and the doubles.
pub struct Kit {
    pub core: Core,
    pub clock: Arc<ManualClock>,
    pub storage: MemoryStorage,
    pub notifier: Arc<RecordingNotifier>,
    pub links: Arc<RecordingLinks>,
}

/// A core on empty memory storage. Call it inside a tokio runtime.
pub fn kit() -> Kit {
    kit_on(MemoryStorage::default())
}

/// A core on `storage`, which may already hold files.
pub fn kit_on(storage: MemoryStorage) -> Kit {
    let clock = Arc::new(ManualClock::default());
    let notifier = Arc::new(RecordingNotifier::default());
    let links = Arc::new(RecordingLinks::default());
    let core = Core::open(Ports {
        spawner: Arc::new(TokioSpawner::current()),
        clock: clock.clone(),
        notifier: notifier.clone(),
        storage: Arc::new(storage.clone()),
        links: links.clone(),
    });
    Kit {
        core,
        clock,
        storage,
        notifier,
        links,
    }
}

/// Let spawned tasks run until they wait for something. On the current-thread
/// runtime `#[tokio::test]` starts, that is every task the test spawned.
pub async fn settle() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

/// Time that moves only when the test says so.
#[derive(Default)]
pub struct ManualClock {
    state: Mutex<ClockState>,
}

#[derive(Default)]
struct ClockState {
    elapsed: Duration,
    sleepers: Vec<(Duration, oneshot::Sender<()>)>,
}

impl ManualClock {
    /// Move time forward and wake every sleeper whose time has come.
    pub fn advance(&self, by: Duration) {
        let due = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.elapsed += by;
            let now = state.elapsed;
            let (due, waiting) = std::mem::take(&mut state.sleepers)
                .into_iter()
                .partition(|(deadline, _)| *deadline <= now);
            state.sleepers = waiting;
            due
        };
        for (_, wake) in due {
            let _ = wake.send(());
        }
    }
}

/// An arbitrary but fixed start, so `now_ms` is never zero.
const EPOCH_MS: u64 = 1_700_000_000_000;

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        EPOCH_MS + state.elapsed.as_millis() as u64
    }

    fn sleep(&self, duration: Duration) -> BoxFuture {
        let (wake, woken) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let deadline = state.elapsed + duration;
            state.sleepers.push((deadline, wake));
        }
        Box::pin(async move {
            let _ = woken.await;
        })
    }
}

/// Files in memory. Clones share them.
#[derive(Clone, Default)]
pub struct MemoryStorage {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    files: Mutex<HashMap<String, Vec<u8>>>,
    writes: Mutex<HashMap<String, usize>>,
    refusing: Mutex<bool>,
}

impl MemoryStorage {
    /// Put a file there, as if an earlier run had written it.
    pub fn put(&self, name: &str, bytes: &[u8]) {
        let mut files = self
            .shared
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        files.insert(name.to_owned(), bytes.to_vec());
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let files = self
            .shared
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        files.get(name).cloned()
    }

    /// How many saves of `name` succeeded.
    pub fn writes(&self, name: &str) -> usize {
        let writes = self
            .shared
            .writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        writes.get(name).copied().unwrap_or(0)
    }

    /// Fail every save from now on, or stop failing them.
    pub fn refuse_saves(&self, refuse: bool) {
        *self
            .shared
            .refusing
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = refuse;
    }
}

impl Storage for MemoryStorage {
    fn open(&self, name: &str) -> Box<dyn VaultStore> {
        Box::new(MemoryFile {
            name: name.to_owned(),
            shared: self.shared.clone(),
        })
    }
}

struct MemoryFile {
    name: String,
    shared: Arc<Shared>,
}

impl VaultStore for MemoryFile {
    fn load(&self) -> Result<Option<Vec<u8>>, VaultError> {
        let files = self
            .shared
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(files.get(&self.name).cloned())
    }

    fn save(&self, bytes: &[u8]) -> Result<(), VaultError> {
        if *self
            .shared
            .refusing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            return Err(VaultError::Io("the store refused the save".into()));
        }
        let mut files = self
            .shared
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        files.insert(self.name.clone(), bytes.to_vec());
        let mut writes = self
            .shared
            .writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *writes.entry(self.name.clone()).or_default() += 1;
        Ok(())
    }

    fn set_aside(&self, bytes: &[u8]) -> Result<(), VaultError> {
        let mut files = self
            .shared
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        files.insert(format!("{}.unreadable", self.name), bytes.to_vec());
        Ok(())
    }
}

#[derive(Default)]
pub struct RecordingNotifier {
    sent: Mutex<Vec<Notification>>,
}

impl RecordingNotifier {
    pub fn sent(&self) -> Vec<Notification> {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Notifier for RecordingNotifier {
    fn notify(&self, notification: Notification) {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(notification);
    }
}

#[derive(Default)]
pub struct RecordingLinks {
    opened: Mutex<Vec<String>>,
}

impl RecordingLinks {
    pub fn opened(&self) -> Vec<String> {
        self.opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Links for RecordingLinks {
    fn open(&self, url: &str) -> Result<(), String> {
        self.opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(url.to_owned());
        Ok(())
    }
}
