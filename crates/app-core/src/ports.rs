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

/// A key the platform keeps in its secure hardware, usable only after a
/// fresh check of the user's fingerprint or face: Android's Keystore, in
/// StrongBox where the phone has one (Knox Vault, Titan M2). It keeps the
/// vault's key sealed ([`wallet_core::SealKey::to_bytes`]), so the vault opens
/// with a fingerprint. The password stays the way back: a guard is forgotten
/// when the vault's key changes or the phone's fingerprints do.
pub trait KeyGuard: Send + Sync {
    /// A key is kept, and [`open`](Self::open) can be asked.
    fn enrolled(&self) -> bool;
    /// Ask for the fingerprint now, and keep `key` sealed behind it.
    fn enroll(&self, key: zeroize::Zeroizing<Vec<u8>>) -> BoxFuture<Result<(), KeyGuardError>>;
    /// Ask for the fingerprint, and give back the key kept.
    fn open(&self) -> BoxFuture<Result<zeroize::Zeroizing<Vec<u8>>, KeyGuardError>>;
    /// Forget the key kept, and the hardware key that sealed it.
    fn forget(&self);
}

/// Why a [`KeyGuard`] gave nothing back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyGuardError {
    /// The user backed out of the fingerprint prompt.
    Declined,
    /// The phone's fingerprints changed, so the hardware key no longer opens
    /// anything. The guard has forgotten it.
    Invalidated,
    Failed(String),
}

impl std::fmt::Display for KeyGuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declined => write!(f, "Cancelled."),
            Self::Invalidated => write!(
                f,
                "The fingerprints on this phone changed, so fingerprint unlock is off. Unlock with the password, then turn it on again in Settings."
            ),
            Self::Failed(why) => write!(f, "Fingerprint unlock failed: {why}"),
        }
    }
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
    /// Hardware wallets this build can open, when it can: the shell hands in
    /// `hw_transport::hub()`. The core never opens a device itself.
    pub devices: Option<Arc<dyn hw_protocol::link::DeviceHub>>,
}
