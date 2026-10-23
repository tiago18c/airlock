//! What the core needs from the platform, so it carries no platform code.
//!
//! Native, web and test implementations live in [`crate::runtime`],
//! [`crate::storage`] and [`crate::testing`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

pub use crate::storage::Storage;

/// `Send` on native, nothing on wasm, where futures hold `Rc`s and run on one
/// thread.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// A boxed future that is `Send` where [`MaybeSend`] is. (A trait object can
/// only add auto traits, so it spells the bound out.)
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<T = ()> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<T = ()> = Pin<Box<dyn Future<Output = T> + 'static>>;

/// Runs background work. The core never spawns in a UI scope.
pub trait Spawner: Send + Sync {
    fn spawn(&self, future: BoxFuture);
}

/// Time, so tests can move it.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
    /// A future that is ready after `duration`. The timer starts now, not at
    /// the first poll.
    fn sleep(&self, duration: Duration) -> BoxFuture;
}

/// Something to tell the user outside the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Notice the toast is about; a click opens the panel or the request modal.
    pub notice_id: crate::notices::NoticeId,
}

pub trait Notifier: Send + Sync {
    fn notify(&self, notification: Notification);
}

/// Open a URL outside the app.
pub trait Links: Send + Sync {
    fn open(&self, url: &str) -> Result<(), String>;
}

/// Ports no one uses do nothing.
pub struct NoNotifier;

impl Notifier for NoNotifier {
    fn notify(&self, _notification: Notification) {}
}

pub struct NoLinks;

impl Links for NoLinks {
    fn open(&self, _url: &str) -> Result<(), String> {
        Err("this build cannot open links".into())
    }
}

/// The ports a shell hands to [`crate::Core::open`].
#[derive(Clone)]
pub struct Ports {
    pub spawner: Arc<dyn Spawner>,
    pub clock: Arc<dyn Clock>,
    pub notifier: Arc<dyn Notifier>,
    pub storage: Arc<dyn Storage>,
    pub links: Arc<dyn Links>,
}
