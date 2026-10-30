//! The pump: the one place the core's published slices become Dioxus signals.
//!
//! Each slice gets a task that waits on `watch::Receiver::changed` and writes
//! a `Signal<Arc<T>>`. Those signals are private to this module, so nothing
//! else writes a slice. Screens see them as `ReadSignal`s, with a memo on top
//! where a plain value is easier to read. The core never touches Dioxus.
//!
//! A task wakes after the command that changed the slice has returned. A
//! command run from an event handler is usually read again right away, so
//! [`Pump::catch_up`] moves the latest values in at once. A task that wakes
//! after it finds nothing new and writes nothing.

use std::sync::Arc;

use app_core::bridge::{BridgeQueue, SitesView};
use app_core::slices::{NetworkStatus, Settings, SyncStatus, VaultState, WalletsView};
use app_core::{Core, Notices, Slice, Slices};
use dioxus::prelude::*;
use wallet_core::{AddressBook, Holdings};

/// What the screens read of the core, kept up to date by the pump.
#[derive(Clone, Copy)]
pub struct Pump {
    /// Public wallet list. While the vault is locked, from the copy kept
    /// without secrets.
    pub wallets: ReadSignal<Arc<WalletsView>>,
    /// Address book.
    pub book: ReadSignal<Arc<AddressBook>>,
    /// Token-account cache for display.
    pub holdings: ReadSignal<Arc<Holdings>>,
    /// Cluster, mode and probe flag.
    pub network: ReadSignal<Arc<NetworkStatus>>,
    /// What the user chose in Settings.
    pub settings: ReadSignal<Arc<Settings>>,
    /// Notices for toasts and (later) the notification center.
    pub notices: ReadSignal<Arc<Notices>>,
    /// Background sync job status (online shell).
    pub sync: ReadSignal<Arc<SyncStatus>>,
    /// Browser requests waiting for the user (desktop shell).
    pub bridge: ReadSignal<Arc<BridgeQueue>>,
    /// Sites connected, remembered and blocked (desktop shell).
    pub sites: ReadSignal<Arc<SitesView>>,
    /// Whether the vault exists and is unlocked.
    pub vault_state: Memo<VaultState>,
    writes: Writes,
}

/// The writable ends. Not `pub`, and not reachable outside this file.
#[derive(Clone, Copy)]
struct Writes {
    wallets: Signal<Arc<WalletsView>>,
    book: Signal<Arc<AddressBook>>,
    holdings: Signal<Arc<Holdings>>,
    network: Signal<Arc<NetworkStatus>>,
    settings: Signal<Arc<Settings>>,
    notices: Signal<Arc<Notices>>,
    sync: Signal<Arc<SyncStatus>>,
    bridge: Signal<Arc<BridgeQueue>>,
    sites: Signal<Arc<SitesView>>,
    vault: Signal<Arc<VaultState>>,
}

/// Start one task per slice. Call once, from the root component.
pub fn use_core(core: Core) -> Pump {
    let writes = Writes {
        wallets: use_slice(&core, |slices| &slices.wallets),
        book: use_slice(&core, |slices| &slices.book),
        holdings: use_slice(&core, |slices| &slices.holdings),
        network: use_slice(&core, |slices| &slices.network),
        settings: use_slice(&core, |slices| &slices.settings),
        notices: use_slice(&core, |slices| &slices.notices),
        sync: use_slice(&core, |slices| &slices.sync),
        bridge: use_slice(&core, |slices| &slices.bridge),
        sites: use_slice(&core, |slices| &slices.sites),
        vault: use_slice(&core, |slices| &slices.vault),
    };
    let vault = writes.vault;
    let vault_state = use_memo(move || **vault.read());
    Pump {
        wallets: writes.wallets.into(),
        book: writes.book.into(),
        holdings: writes.holdings.into(),
        network: writes.network.into(),
        settings: writes.settings.into(),
        notices: writes.notices.into(),
        sync: writes.sync.into(),
        bridge: writes.bridge.into(),
        sites: writes.sites.into(),
        vault_state,
        writes,
    }
}

impl Pump {
    /// Write whatever the core has published since the tasks last ran.
    pub fn catch_up(&self, core: &Core) {
        let slices = core.slices();
        let mut writes = self.writes;
        write_if_new(&mut writes.wallets, slices.wallets.get());
        write_if_new(&mut writes.book, slices.book.get());
        write_if_new(&mut writes.holdings, slices.holdings.get());
        write_if_new(&mut writes.network, slices.network.get());
        write_if_new(&mut writes.settings, slices.settings.get());
        write_if_new(&mut writes.notices, slices.notices.get());
        write_if_new(&mut writes.sync, slices.sync.get());
        write_if_new(&mut writes.bridge, slices.bridge.get());
        write_if_new(&mut writes.sites, slices.sites.get());
        write_if_new(&mut writes.vault, slices.vault.get());
    }
}

fn use_slice<T: 'static>(core: &Core, pick: fn(&Slices) -> &Slice<T>) -> Signal<Arc<T>> {
    use_hook(|| {
        let mut changes = pick(core.slices()).subscribe();
        let value = Signal::new(changes.borrow_and_update().clone());
        let mut writer = value;
        spawn(async move {
            while changes.changed().await.is_ok() {
                let latest = changes.borrow_and_update().clone();
                write_if_new(&mut writer, latest);
            }
        });
        value
    })
}

fn write_if_new<T: 'static>(signal: &mut Signal<Arc<T>>, latest: Arc<T>) {
    let current: Arc<T> = (*signal.peek()).clone();
    if !Arc::ptr_eq(&current, &latest) {
        signal.set(latest);
    }
}
