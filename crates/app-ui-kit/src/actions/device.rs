//! Jobs on a hardware wallet: find it, get it ready, then read a key, show
//! one, or sign. Each job puts where it stands in `Session::device`, which
//! the device dialog shows, and only one runs at a time.
//!
//! The device decides for its key. What it returns is checked before it is
//! used: the key at the path against the record, the signature against the
//! key and the bytes (`wallet_flows::device`).

use crate::state::device::{DeviceAsk, DeviceRun, DeviceStage};
use crate::state::session::Session;
use dioxus::prelude::*;
use hw_protocol::ledger::{AppConfiguration, LedgerError};
use hw_protocol::link::{DeviceHub, DeviceKind, ExchangeError, LinkError, PacketLink};
use solana_pubkey::Pubkey;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wallet_core::{DeviceSeen, HardwareVendor, PathScheme, Version, WalletId};
use wallet_flows::device::{self, DeviceError, LedgerApp};

/// How often a device that is not ready yet is asked again.
const POLL: Duration = Duration::from_millis(800);
/// How long the dialog waits for the user to connect, unlock and open.
const PATIENCE: u32 = 225;
/// How long a question the user does not answer on the device may take.
const ANSWER: Duration = Duration::from_secs(8);
/// Link errors in a row before the dialog says what they are.
const LINK_ERRORS_SHOWN: u32 = 3;
/// What a job answers when the user closed its dialog. Not an error to show.
pub const CANCELLED: &str = "Cancelled.";

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

/// The job a run belongs to. Writes only while the dialog shows this job.
#[derive(Clone, Copy)]
struct Run {
    session: Session,
    id: u64,
}

impl Run {
    /// Show a new job, in place of any other.
    fn start(session: Session, vendor: HardwareVendor, ask: DeviceAsk) -> Self {
        let id = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
        let mut shown = session.device;
        shown.set(Some(DeviceRun {
            id,
            vendor,
            ask,
            stage: DeviceStage::Find(format!("Connect your {} and unlock it.", vendor.label())),
            model: None,
        }));
        Self { session, id }
    }

    fn update(&self, change: impl FnOnce(&mut DeviceRun)) {
        let mut shown = self.session.device;
        let mut run = shown.write();
        if let Some(run) = run.as_mut().filter(|run| run.id == self.id) {
            change(run);
        }
    }

    fn stage(&self, stage: DeviceStage) {
        let same = self
            .session
            .device
            .peek()
            .as_ref()
            .is_some_and(|run| run.id == self.id && run.stage == stage);
        if !same {
            self.update(|run| run.stage = stage);
        }
    }

    /// The user cancelled, or closed the dialog, or another job took it.
    fn cancelled(&self) -> bool {
        self.session
            .device
            .peek()
            .as_ref()
            .is_none_or(|run| run.id != self.id)
    }

    fn fail(&self, why: &str) {
        self.stage(DeviceStage::Failed(why.to_string()));
    }

    /// Close the dialog: the job is done and nothing is left to read.
    fn close(&self) {
        let mut shown = self.session.device;
        if shown.peek().as_ref().is_some_and(|run| run.id == self.id) {
            shown.set(None);
        }
    }
}

/// The user closed the dialog, or pressed Cancel. It closes at once: a
/// device that is not answering would otherwise hold it open. The job sees
/// it between exchanges, and nothing it gets after that is used.
pub fn cancel(session: Session) {
    let mut shown = session.device;
    shown.set(None);
}

/// `work`, or `None` once `limit` has passed without it finishing.
async fn within<T>(limit: Duration, work: impl Future<Output = T>) -> Option<T> {
    let mut work = std::pin::pin!(work);
    let mut timer = std::pin::pin!(crate::rt::sleep(limit));
    std::future::poll_fn(|cx| {
        if let std::task::Poll::Ready(out) = work.as_mut().poll(cx) {
            return std::task::Poll::Ready(Some(out));
        }
        timer.as_mut().poll(cx).map(|()| None)
    })
    .await
}

fn hub(session: Session) -> Result<Arc<dyn DeviceHub>, String> {
    session
        .core()
        .ports()
        .devices
        .clone()
        .ok_or_else(|| "This build of Airlock cannot open hardware wallets.".to_string())
}

/// What the dialog says for an error of the device.
fn said(error: &DeviceError) -> String {
    crate::actions::sentence(error)
}

/// The open Ledger, with the Solana app ready to sign, and what it is.
struct ReadyLedger {
    link: Box<dyn PacketLink>,
    config: AppConfiguration,
    seen: DeviceSeen,
}

/// Find a Ledger and get its Solana app open, waiting while the user
/// connects it, unlocks it and opens the app.
async fn ready_ledger(run: Run) -> Result<ReadyLedger, String> {
    let hub = hub(run.session)?;
    let mut asked_to_open = false;
    let mut link_errors = 0;
    let mut link_failed = |run: &Run, error: &dyn std::fmt::Display| {
        link_errors += 1;
        if link_errors >= LINK_ERRORS_SHOWN {
            run.stage(DeviceStage::Find(format!(
                "{error} Unplug your Ledger, plug it in again and unlock it."
            )));
        }
    };
    for _ in 0..PATIENCE {
        if run.cancelled() {
            return Err(CANCELLED.into());
        }
        let found = hub.list().await.map_err(|error| error.to_string())?;
        let Some(ledger) = found
            .into_iter()
            .find(|device| device.kind == DeviceKind::Ledger)
        else {
            run.stage(DeviceStage::Find(
                "Connect your Ledger and unlock it.".into(),
            ));
            crate::rt::sleep(POLL).await;
            continue;
        };
        let model = ledger.model.clone();
        run.update(|shown| shown.model = model.clone());
        let mut link = match hub.open(&ledger.id).await {
            Ok(link) => link,
            Err(LinkError::Busy) => {
                run.stage(DeviceStage::Find(LinkError::Busy.to_string()));
                crate::rt::sleep(POLL).await;
                continue;
            }
            // Gone between the list and the open: it restarts as an app opens.
            Err(error @ (LinkError::Disconnected | LinkError::Io(_))) => {
                link_failed(&run, &error);
                crate::rt::sleep(POLL).await;
                continue;
            }
            Err(denied @ LinkError::Denied(_)) => return Err(denied_sentence(&denied)),
        };
        // A Ledger that is asleep, or busy with something else, may not
        // answer at all: ask again rather than wait on it.
        let Some(app) = within(ANSWER, device::ledger_app(link.as_mut())).await else {
            run.stage(DeviceStage::Find(
                "Your Ledger does not answer. Unlock it, or unplug it and plug it in again.".into(),
            ));
            drop(link);
            crate::rt::sleep(POLL).await;
            continue;
        };
        match app {
            Ok(LedgerApp::Ready(config)) => {
                let (major, minor, patch) = config.version;
                let seen = DeviceSeen {
                    model,
                    firmware: Some(Version::new(major.into(), minor.into(), patch.into())),
                };
                tracing::info!(
                    target: "airlock::device",
                    vendor = "ledger",
                    model = seen.model.as_deref().unwrap_or("unknown"),
                    app = %seen.firmware.map(|v| v.to_string()).unwrap_or_default(),
                    blind_signing = config.blind_signing,
                    "ready"
                );
                return Ok(ReadyLedger { link, config, seen });
            }
            Ok(LedgerApp::Dashboard) if !asked_to_open => {
                asked_to_open = true;
                run.stage(DeviceStage::Find(
                    "Confirm on your Ledger to open the Solana app.".into(),
                ));
                match device::ledger_open_app(link.as_mut()).await {
                    Ok(()) => {}
                    Err(DeviceError::Exchange(ExchangeError::Ledger(
                        refused @ (LedgerError::OpenRefused
                        | LedgerError::Rejected
                        | LedgerError::AppNotInstalled),
                    ))) => return Err(refused.to_string()),
                    // It restarts as the app opens, and is listed again.
                    Err(_) => {}
                }
            }
            Ok(LedgerApp::Dashboard) => run.stage(DeviceStage::Find(
                "Open the Solana app on your Ledger.".into(),
            )),
            Ok(LedgerApp::OtherApp) => run.stage(DeviceStage::Find(
                "Another app is open on your Ledger. Quit it, then open the Solana app.".into(),
            )),
            Err(DeviceError::Exchange(ExchangeError::Ledger(LedgerError::Locked))) => {
                run.stage(DeviceStage::Find("Unlock your Ledger.".into()))
            }
            Err(DeviceError::Exchange(ExchangeError::Link(error))) => link_failed(&run, &error),
            Err(other) => return Err(said(&other)),
        }
        drop(link);
        crate::rt::sleep(POLL).await;
    }
    Err(
        "No Ledger was ready. Connect it, unlock it and open the Solana app, then try again."
            .into(),
    )
}

fn denied_sentence(denied: &LinkError) -> String {
    let mut text = denied.to_string();
    if cfg!(target_os = "linux") {
        text.push_str(" On Linux, install Airlock's udev rules for Ledger first: see the README.");
    }
    text
}

/// Keep what was seen of the device on the wallet's record, when the vault
/// is open to write it.
fn remember_seen(session: Session, wallet: WalletId, seen: &DeviceSeen) {
    if !session.is_unlocked() {
        return;
    }
    let seen = seen.clone();
    let _ = session.update_wallet(wallet, |kind| {
        if kind.device_seen() != Some(&seen) {
            kind.set_device_seen(seen);
        }
    });
}

/// Where a hardware wallet of this vault keeps `account`.
struct OnDevice {
    wallet: WalletId,
    vendor: HardwareVendor,
    path: String,
}

fn on_device(session: Session, account: &Pubkey) -> Result<OnDevice, String> {
    (session.wallets)()
        .summaries
        .iter()
        .find(|wallet| wallet.address == Some(*account) && wallet.on_device.is_some())
        .and_then(|wallet| {
            Some(OnDevice {
                wallet: wallet.id,
                vendor: wallet.on_device?,
                path: wallet.derivation_path.clone()?,
            })
        })
        .ok_or_else(|| format!("No hardware wallet of this vault holds {account}."))
}

fn only_ledger(vendor: HardwareVendor) -> Result<(), String> {
    if vendor == HardwareVendor::Ledger {
        Ok(())
    } else {
        Err(format!(
            "This build of Airlock does not sign with a {} yet.",
            vendor.label()
        ))
    }
}

/// A device job is on screen, so another one waits.
pub fn busy(session: Session) -> bool {
    session
        .device
        .peek()
        .as_ref()
        .is_some_and(|run| !run.ended())
}

/// Sign `message`, the bytes a transaction's signatures cover, with each
/// key of `accounts` on the hardware wallet that holds it, in order. Each
/// signature is checked before it is answered.
pub async fn sign_message_bytes(
    session: Session,
    accounts: &[Pubkey],
    message: &[u8],
) -> Result<Vec<(Pubkey, [u8; 64])>, String> {
    let mut out = Vec::new();
    let total = accounts.len();
    for (at, account) in accounts.iter().enumerate() {
        let held = on_device(session, account)?;
        only_ledger(held.vendor)?;
        let run = Run::start(
            session,
            held.vendor,
            DeviceAsk::Sign {
                account: *account,
                path: held.path.clone(),
                of: (total > 1).then_some((at + 1, total)),
                hash: Some(wallet_core::hardware::message_hash(message)),
                message: false,
                authorization: false,
            },
        );
        let signature = sign_one(run, &held, account, message, false).await?;
        out.push((*account, signature));
    }
    Ok(out)
}

/// Sign a programmatic signer's authorization, `bytes`, with the Ledger that
/// holds `authority`. It is a v1 message with no blockhash, signed blind:
/// what runs is inside it, so the Ledger shows only its hash.
pub async fn sign_authorization(
    session: Session,
    authority: Pubkey,
    bytes: &[u8],
    of: Option<(usize, usize)>,
) -> Result<[u8; 64], String> {
    let held = on_device(session, &authority)?;
    only_ledger(held.vendor)?;
    let run = Run::start(
        session,
        held.vendor,
        DeviceAsk::Sign {
            account: authority,
            path: held.path.clone(),
            of,
            hash: Some(wallet_core::hardware::message_hash(bytes)),
            message: false,
            authorization: true,
        },
    );
    sign_one(run, &held, &authority, bytes, false).await
}

/// Sign a site's message with the Ledger that holds `account`: `ocms` is the
/// whole offchain message, header and text, which the site gets back.
pub async fn sign_offchain(
    session: Session,
    account: Pubkey,
    ocms: &[u8],
) -> Result<[u8; 64], String> {
    let held = on_device(session, &account)?;
    only_ledger(held.vendor)?;
    let run = Run::start(
        session,
        held.vendor,
        DeviceAsk::Sign {
            account,
            path: held.path.clone(),
            of: None,
            hash: None,
            message: true,
            authorization: false,
        },
    );
    sign_one(run, &held, &account, ocms, true).await
}

async fn sign_one(
    run: Run,
    held: &OnDevice,
    account: &Pubkey,
    bytes: &[u8],
    offchain: bool,
) -> Result<[u8; 64], String> {
    let outcome = async {
        let mut ready = ready_ledger(run).await?;
        remember_seen(run.session, held.wallet, &ready.seen);
        run.stage(DeviceStage::Account);
        device::ledger_check_key(ready.link.as_mut(), &held.path, account)
            .await
            .map_err(|error| said(&error))?;
        if run.cancelled() {
            return Err(CANCELLED.to_string());
        }
        run.stage(DeviceStage::Confirm);
        let signed = if offchain {
            device::ledger_sign_offchain(ready.link.as_mut(), &held.path, account, bytes).await
        } else {
            device::ledger_sign(ready.link.as_mut(), &held.path, account, bytes).await
        };
        let signature = signed.map_err(|error| {
            if matches!(
                error,
                DeviceError::Exchange(ExchangeError::Ledger(LedgerError::NotSupported))
            ) && !ready.config.blind_signing
            {
                "Your Ledger signs this only with Blind signing on. Turn it on in the Solana app's settings, sign, then turn it off again.".to_string()
            } else {
                said(&error)
            }
        })?;
        // Closed while the device was showing it: the user did not wait for it.
        if run.cancelled() {
            return Err(CANCELLED.to_string());
        }
        tracing::info!(
            target: "airlock::device",
            vendor = "ledger",
            path = %held.path,
            pubkey = %account,
            offchain,
            "signed"
        );
        Ok(<[u8; 64]>::from(signature))
    }
    .await;
    match &outcome {
        Ok(_) => run.close(),
        Err(why) if why == CANCELLED => run.close(),
        Err(why) => {
            tracing::info!(target: "airlock::device", vendor = "ledger", reason = %why, "refused");
            run.fail(why);
        }
    }
    outcome
}

/// Show `account` on the hardware wallet of this vault that holds it, for
/// the user to compare with the screen. The dialog says how it went.
pub fn show_on_device(session: Session, account: Pubkey) {
    match on_device(session, &account) {
        Ok(held) => show_address(session, held.vendor, held.path, account, Some(held.wallet)),
        Err(why) => {
            let run = Run::start(
                session,
                HardwareVendor::Ledger,
                DeviceAsk::Show {
                    account,
                    path: String::new(),
                },
            );
            run.fail(&why);
        }
    }
}

/// Show the key at `path` on a device of `vendor`, which has to be
/// `account`. `wallet` is the record to keep what was seen on, once there
/// is one.
pub fn show_address(
    session: Session,
    vendor: HardwareVendor,
    path: String,
    account: Pubkey,
    wallet: Option<WalletId>,
) {
    if busy(session) {
        return;
    }
    let run = Run::start(
        session,
        vendor,
        DeviceAsk::Show {
            account,
            path: path.clone(),
        },
    );
    if let Err(why) = only_ledger(vendor) {
        run.fail(&why);
        return;
    }
    crate::actions::spawn_app_task(async move {
        let outcome = async {
            let mut ready = ready_ledger(run).await?;
            if let Some(wallet) = wallet {
                remember_seen(session, wallet, &ready.seen);
            }
            run.stage(DeviceStage::Confirm);
            let shown = device::ledger_pubkey(ready.link.as_mut(), &path, true)
                .await
                .map_err(|error| said(&error))?;
            if shown != account {
                return Err(said(&DeviceError::WrongKey {
                    vendor: "Ledger",
                    expected: account,
                    path: path.clone(),
                }));
            }
            Ok(())
        }
        .await;
        match outcome {
            Ok(()) => run.stage(DeviceStage::Done),
            Err(why) if why == CANCELLED => run.close(),
            Err(why) => run.fail(&why),
        }
    });
}

/// One key read from the device for Add wallet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceKey {
    pub index: u32,
    pub path: String,
    pub pubkey: Pubkey,
}

/// What Add wallet read from a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceKeys {
    pub vendor: HardwareVendor,
    pub keys: Vec<DeviceKey>,
    pub seen: DeviceSeen,
}

/// Read the keys `start..start + count` of `scheme` from a hardware wallet
/// of `vendor`, without showing them on it.
pub async fn read_keys(
    session: Session,
    vendor: HardwareVendor,
    scheme: PathScheme,
    start: u32,
    count: u32,
) -> Result<DeviceKeys, String> {
    let run = Run::start(
        session,
        vendor,
        DeviceAsk::Read {
            scheme: scheme.template.to_string(),
        },
    );
    if let Err(why) = only_ledger(vendor) {
        run.fail(&why);
        return Err(why);
    }
    let outcome = async {
        let mut ready = ready_ledger(run).await?;
        run.stage(DeviceStage::Account);
        // A scheme without an index has one account.
        let indices = if scheme.template.contains('*') {
            start..start + count
        } else {
            0..1
        };
        let mut keys = Vec::new();
        for index in indices {
            if run.cancelled() {
                return Err(CANCELLED.to_string());
            }
            let path = scheme.path(index);
            let pubkey = device::ledger_pubkey(ready.link.as_mut(), &path, false)
                .await
                .map_err(|error| said(&error))?;
            keys.push(DeviceKey {
                index,
                path,
                pubkey,
            });
        }
        Ok(DeviceKeys {
            vendor,
            keys,
            seen: ready.seen,
        })
    }
    .await;
    match &outcome {
        Ok(_) => run.close(),
        Err(why) if why == CANCELLED => run.close(),
        Err(why) => run.fail(why),
    }
    outcome
}
