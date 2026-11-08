//! Jobs on a hardware wallet: find it, get it ready, then read a key, show
//! one, or sign. Each job puts where it stands in `Session::device`, which
//! the device dialog shows, and only one runs at a time.
//!
//! The device decides for its key. What it returns is checked before it is
//! used: the key at the path against the record, the signature against the
//! key and the bytes (`wallet_flows::device`).

use crate::state::device::{DeviceAsk, DeviceRun, DeviceStage, KeystoneTrip};
use crate::state::session::Session;
use dioxus::prelude::*;
use hw_protocol::keystone::{KeyPath, SignType, SolSignRequest, SolSignature, SOL_SIGNATURE};
use hw_protocol::ledger::{AppConfiguration, LedgerError};
use hw_protocol::link::{DeviceHub, DeviceKind, ExchangeError, LinkError, PacketLink};
pub use hw_protocol::seed_vault::Authorize;
use hw_protocol::seed_vault::{Payload, SeedVaultError};
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
            keystone: None,
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
        .ok_or_else(|| {
            if cfg!(target_arch = "wasm32") {
                "This browser cannot open a Ledger or a Trezor. Use Chrome, Edge or Brave, on a secure page. A Keystone works in any browser with a camera.".to_string()
            } else {
                "This build of Airlock cannot open hardware wallets.".to_string()
            }
        })
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

/// Find a Trezor and start a session on it, waiting while the user connects
/// it and Trezor Suite lets go of it.
async fn ready_trezor(run: Run) -> Result<Ready, String> {
    let hub = hub(run.session)?;
    let mut link_errors = 0;
    for _ in 0..PATIENCE {
        if run.cancelled() {
            return Err(CANCELLED.into());
        }
        let found = hub.list().await.map_err(|error| error.to_string())?;
        let Some(trezor) = found
            .into_iter()
            .find(|device| device.kind == DeviceKind::Trezor)
        else {
            run.stage(DeviceStage::Find("Connect your Trezor.".into()));
            crate::rt::sleep(POLL).await;
            continue;
        };
        let mut link = match hub.open(&trezor.id).await {
            Ok(link) => link,
            Err(LinkError::Busy) => {
                run.stage(DeviceStage::Find(
                    "Another app is using your Trezor. Close Trezor Suite, then try again.".into(),
                ));
                crate::rt::sleep(POLL).await;
                continue;
            }
            Err(denied @ LinkError::Denied(_)) => return Err(denied_sentence(&denied)),
            Err(error) => {
                link_errors += 1;
                if link_errors >= LINK_ERRORS_SHOWN {
                    run.stage(DeviceStage::Find(format!(
                        "{error} Unplug your Trezor and plug it in again."
                    )));
                }
                crate::rt::sleep(POLL).await;
                continue;
            }
        };
        let Some(answer) = within(ANSWER, device::trezor_ready(link.as_mut())).await else {
            run.stage(DeviceStage::Find(
                "Your Trezor does not answer. Unplug it and plug it in again.".into(),
            ));
            drop(link);
            crate::rt::sleep(POLL).await;
            continue;
        };
        match answer {
            Ok(ready) => {
                let seen = DeviceSeen {
                    model: ready.features.model.clone().map(|model| model_name(&model)),
                    firmware: Some(ready.version),
                };
                run.update(|shown| shown.model = seen.model.clone());
                tracing::info!(
                    target: "airlock::device",
                    vendor = "trezor",
                    model = seen.model.as_deref().unwrap_or("unknown"),
                    firmware = %ready.version,
                    "ready"
                );
                return Ok(Ready::Trezor {
                    link,
                    seen,
                    device_id: ready.features.device_id,
                });
            }
            Err(DeviceError::Exchange(ExchangeError::Link(_))) => {
                link_errors += 1;
                crate::rt::sleep(POLL).await;
            }
            Err(other) => return Err(said(&other)),
        }
    }
    Err("No Trezor was ready. Connect it, then try again.".into())
}

/// A Trezor's model as people call it: Features says `T`, `Safe 3`.
fn model_name(model: &str) -> String {
    match model {
        "T" | "1" => format!("Model {model}"),
        other => other.to_string(),
    }
}

/// A device ready for a job: the link, and what it is.
enum Ready {
    Ledger(ReadyLedger),
    Trezor {
        link: Box<dyn PacketLink>,
        seen: DeviceSeen,
        /// What names this Trezor, kept on its wallets.
        device_id: Option<String>,
    },
}

impl Ready {
    fn seen(&self) -> &DeviceSeen {
        match self {
            Self::Ledger(ready) => &ready.seen,
            Self::Trezor { seen, .. } => seen,
        }
    }

    fn vendor(&self) -> &'static str {
        match self {
            Self::Ledger(_) => "Ledger",
            Self::Trezor { .. } => "Trezor",
        }
    }

    async fn pubkey(&mut self, path: &str, show: bool) -> Result<Pubkey, String> {
        match self {
            Self::Ledger(ready) => device::ledger_pubkey(ready.link.as_mut(), path, show).await,
            Self::Trezor { link, .. } => device::trezor_pubkey(link.as_mut(), path, show).await,
        }
        .map_err(|error| said(&error))
    }

    async fn check_key(&mut self, path: &str, account: &Pubkey) -> Result<(), String> {
        match self {
            Self::Ledger(ready) => {
                device::ledger_check_key(ready.link.as_mut(), path, account).await
            }
            Self::Trezor { link, .. } => {
                device::trezor_check_key(link.as_mut(), path, account).await
            }
        }
        .map_err(|error| said(&error))
    }

    /// Sign `bytes`: a transaction's message, or with `offchain` the whole
    /// offchain message.
    async fn sign(
        &mut self,
        path: &str,
        account: &Pubkey,
        bytes: &[u8],
        offchain: bool,
    ) -> Result<solana_signature::Signature, String> {
        match self {
            Self::Ledger(ready) => {
                let signed = if offchain {
                    device::ledger_sign_offchain(ready.link.as_mut(), path, account, bytes).await
                } else {
                    device::ledger_sign(ready.link.as_mut(), path, account, bytes).await
                };
                signed.map_err(|error| {
                    if matches!(
                        error,
                        DeviceError::Exchange(ExchangeError::Ledger(LedgerError::NotSupported))
                    ) && !ready.config.blind_signing
                    {
                        "Your Ledger signs this only with Blind signing on. Turn it on in the Solana app's settings, sign, then turn it off again.".to_string()
                    } else {
                        said(&error)
                    }
                })
            }
            Self::Trezor { link, .. } => {
                let signed = if offchain {
                    device::trezor_sign_offchain(link.as_mut(), path, account, bytes).await
                } else {
                    device::trezor_sign(link.as_mut(), path, account, bytes).await
                };
                signed.map_err(|error| said(&error))
            }
        }
    }
}

/// Get a device of `vendor` ready, over its cable.
async fn ready(run: Run, vendor: HardwareVendor) -> Result<Ready, String> {
    match vendor {
        HardwareVendor::Ledger => ready_ledger(run).await.map(Ready::Ledger),
        HardwareVendor::Trezor => ready_trezor(run).await,
        HardwareVendor::Keystone => Err("A Keystone answers through QR codes.".into()),
        HardwareVendor::SeedVault => Err("The Seed Vault is in this phone.".into()),
    }
}

fn denied_sentence(denied: &LinkError) -> String {
    let mut text = denied.to_string();
    if cfg!(target_os = "linux") {
        text.push_str(" On Linux, install Airlock's udev rules first: see the README.");
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
    /// What names the device: a Keystone's master fingerprint.
    device: Option<String>,
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
                device: wallet.device.clone(),
            })
        })
        .ok_or_else(|| format!("No hardware wallet of this vault holds {account}."))
}

/// The vendors this build signs with.
fn signs_with(vendor: HardwareVendor) -> Result<(), String> {
    if vendor.built() {
        Ok(())
    } else {
        Err(format!(
            "This build of Airlock does not sign with a {} yet.",
            vendor.label()
        ))
    }
}

/// The vendors whose devices answer over a cable.
fn over_cable(vendor: HardwareVendor) -> Result<(), String> {
    match vendor {
        HardwareVendor::Ledger | HardwareVendor::Trezor => Ok(()),
        HardwareVendor::Keystone => Err("A Keystone answers through QR codes.".into()),
        HardwareVendor::SeedVault => Err("The Seed Vault is in this phone.".into()),
    }
}

/// This build opens a device only once the user chose it for the page.
pub fn needs_request(session: Session) -> bool {
    hub(session).is_ok_and(|hub| hub.needs_request())
}

/// Ask the browser to let this page open a device of `vendor`. Runs from
/// the click on the dialog's button; the job waiting for the device finds
/// it on its next look.
pub fn request_device(session: Session, vendor: HardwareVendor) {
    let Ok(hub) = hub(session) else {
        return;
    };
    let kind = match vendor {
        HardwareVendor::Ledger => DeviceKind::Ledger,
        HardwareVendor::Trezor => DeviceKind::Trezor,
        HardwareVendor::Keystone | HardwareVendor::SeedVault => return,
    };
    spawn(async move {
        if let Err(error) = hub.request(kind).await {
            tracing::info!(target: "airlock::device", %error, "not chosen");
        }
    });
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
        signs_with(held.vendor)?;
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
        let signature = match held.vendor {
            HardwareVendor::Keystone => {
                sign_keystone(run, &held, account, message, SignType::Transaction).await?
            }
            HardwareVendor::SeedVault => {
                sign_seed_vault(run, &held, account, message, Payload::Transaction).await?
            }
            HardwareVendor::Ledger | HardwareVendor::Trezor => {
                sign_one(run, &held, account, message, false).await?
            }
        };
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
    signs_with(held.vendor)?;
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
    match held.vendor {
        // A Keystone reads an authorization as the v1 message it is.
        HardwareVendor::Keystone => {
            sign_keystone(run, &held, &authority, bytes, SignType::Transaction).await
        }
        HardwareVendor::SeedVault => {
            sign_seed_vault(run, &held, &authority, bytes, Payload::Transaction).await
        }
        HardwareVendor::Ledger | HardwareVendor::Trezor => {
            sign_one(run, &held, &authority, bytes, false).await
        }
    }
}

/// Sign a site's message, `bytes` as the site sent them, with the Keystone
/// that holds `account`. A Keystone signs a message as a hot key does: the
/// bytes as they are, after `check_message`.
pub async fn sign_message_as_is(
    session: Session,
    account: Pubkey,
    bytes: &[u8],
) -> Result<[u8; 64], String> {
    let held = on_device(session, &account)?;
    if held.vendor != HardwareVendor::Keystone {
        return Err(format!(
            "A {} signs messages inside an offchain message, not as they are.",
            held.vendor.label()
        ));
    }
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
    sign_keystone(run, &held, &account, bytes, SignType::Message).await
}

/// How often the job looks for the signature the dialog scanned.
const SCAN_POLL: Duration = Duration::from_millis(250);

/// Show a Keystone the `sol-sign-request` for `bytes`, and wait for the
/// user to scan the signature it answers with. The request id must match
/// and the signature must verify.
async fn sign_keystone(
    run: Run,
    held: &OnDevice,
    account: &Pubkey,
    bytes: &[u8],
    sign_type: SignType,
) -> Result<[u8; 64], String> {
    let outcome = async {
        let indices = hw_protocol::parse_path(&held.path).map_err(|error| error.to_string())?;
        let source_fingerprint = held
            .device
            .as_deref()
            .and_then(|hex| u32::from_str_radix(hex, 16).ok());
        // A request id that is this request's: its bytes and this job.
        let mut seed = bytes.to_vec();
        seed.extend_from_slice(&run.id.to_le_bytes());
        let digest = wallet_core::sha256_bytes(&seed);
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest[..16]);
        let request = SolSignRequest {
            request_id: id,
            sign_data: bytes.to_vec(),
            path: KeyPath {
                indices,
                source_fingerprint,
            },
            address: None,
            origin: Some("Airlock".into()),
            sign_type,
        };
        run.update(|shown| {
            shown.keystone = Some(KeystoneTrip {
                request: request.to_cbor(),
                id,
                answer: None,
                problem: None,
            });
        });
        run.stage(DeviceStage::Confirm);
        loop {
            if run.cancelled() {
                return Err(CANCELLED.to_string());
            }
            let answer = run
                .session
                .device
                .peek()
                .as_ref()
                .and_then(|shown| shown.keystone.as_ref()?.answer);
            if let Some(signature) = answer {
                let checked = solana_signature::Signature::from(signature);
                if checked.verify(account.as_ref(), bytes) {
                    tracing::info!(
                        target: "airlock::device",
                        vendor = "keystone",
                        path = %held.path,
                        pubkey = %account,
                        "signed"
                    );
                    return Ok(signature);
                }
                run.update(|shown| {
                    if let Some(trip) = shown.keystone.as_mut() {
                        trip.answer = None;
                        trip.problem = Some(format!(
                            "That signature is not {account}'s over this request. Is the Keystone the one this wallet was added from, with the same passphrase?"
                        ));
                    }
                });
            }
            crate::rt::sleep(SCAN_POLL).await;
        }
    }
    .await;
    match &outcome {
        Ok(_) => run.close(),
        Err(why) if why == CANCELLED => run.close(),
        Err(why) => run.fail(why),
    }
    outcome
}

/// The dialog scanned a UR while a Keystone job waits for its answer.
/// Only a `sol-signature` for this request is taken.
pub fn keystone_scanned(session: Session, ur_type: &str, message: &[u8]) {
    let mut shown = session.device;
    let mut run = shown.write();
    let Some(trip) = run.as_mut().and_then(|run| run.keystone.as_mut()) else {
        return;
    };
    if ur_type != SOL_SIGNATURE {
        trip.problem = Some(format!(
            "That code is a {ur_type}, not the signature. On the Keystone, sign the request, then scan the code it shows."
        ));
        return;
    }
    match SolSignature::from_cbor(message) {
        Ok(signature) if signature.request_id.is_none_or(|id| id == trip.id) => {
            trip.problem = None;
            trip.answer = Some(signature.signature);
        }
        Ok(_) => {
            trip.problem = Some(
                "That signature answers another request. Scan the one for this request.".into(),
            );
        }
        Err(_) => trip.problem = Some("That code could not be read as a signature.".into()),
    }
}

/// Sign a site's message with the Ledger or Trezor that holds `account`:
/// `ocms` is the whole offchain message, header and text, which the site
/// gets back.
pub async fn sign_offchain(
    session: Session,
    account: Pubkey,
    ocms: &[u8],
) -> Result<[u8; 64], String> {
    let held = on_device(session, &account)?;
    if held.vendor != HardwareVendor::SeedVault {
        over_cable(held.vendor)?;
    }
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
    if held.vendor == HardwareVendor::SeedVault {
        return sign_seed_vault(run, &held, &account, ocms, Payload::Message).await;
    }
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
        let mut ready = ready(run, held.vendor).await?;
        remember_seen(run.session, held.wallet, ready.seen());
        run.stage(DeviceStage::Account);
        ready.check_key(&held.path, account).await?;
        if run.cancelled() {
            return Err(CANCELLED.to_string());
        }
        run.stage(DeviceStage::Confirm);
        let signature = ready.sign(&held.path, account, bytes, offchain).await?;
        // Closed while the device was showing it: the user did not wait for it.
        if run.cancelled() {
            return Err(CANCELLED.to_string());
        }
        tracing::info!(
            target: "airlock::device",
            vendor = ready.vendor(),
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
            tracing::info!(target: "airlock::device", vendor = held.vendor.label(), reason = %why, "refused");
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
    // A Keystone shows an address on its own Receive screen: the dialog
    // says where to look.
    // A Seed Vault has no screen of its own to show an address on.
    if matches!(vendor, HardwareVendor::Keystone | HardwareVendor::SeedVault) {
        run.stage(DeviceStage::Done);
        return;
    }
    if let Err(why) = over_cable(vendor).and(signs_with(vendor)) {
        run.fail(&why);
        return;
    }
    crate::actions::spawn_app_task(async move {
        let outcome = async {
            let mut ready = ready(run, vendor).await?;
            if let Some(wallet) = wallet {
                remember_seen(session, wallet, ready.seen());
            }
            run.stage(DeviceStage::Confirm);
            let shown = ready.pubkey(&path, true).await?;
            if shown != account {
                return Err(said(&DeviceError::WrongKey {
                    vendor: ready.vendor(),
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
    /// What names the device, kept on its wallets: a Trezor's device id, a
    /// Keystone's master fingerprint.
    pub device: Option<String>,
}

/// The accounts in a Keystone's `crypto-multi-accounts` export, with its
/// master fingerprint, in hex, as the device. Only Ed25519 keys under
/// `44'/501'` are taken.
/// A firmware below the first that reads v1 is refused.
pub fn keystone_export(ur_type: &str, message: &[u8]) -> Result<DeviceKeys, String> {
    if ur_type != hw_protocol::keystone::CRYPTO_MULTI_ACCOUNTS {
        return Err(format!(
            "That code is a {ur_type}. On your Keystone, open Connect Software Wallet and choose Solflare, then scan the codes it shows."
        ));
    }
    let export = hw_protocol::keystone::MultiAccounts::from_cbor(message)
        .map_err(|_| "That export could not be read.".to_string())?;
    let firmware = export
        .firmware
        .as_deref()
        .and_then(|text| text.parse::<Version>().ok());
    if let Some(refusal) = firmware.and_then(|version| HardwareVendor::Keystone.refusal(version)) {
        return Err(refusal);
    }
    let keys: Vec<DeviceKey> = export
        .keys
        .iter()
        .filter_map(|key| {
            // Some exports put a zero byte before an Ed25519 key.
            let bytes = match key.key.as_slice() {
                [0, rest @ ..] if rest.len() == 32 => rest,
                bytes => bytes,
            };
            let pubkey = Pubkey::new_from_array(<[u8; 32]>::try_from(bytes).ok()?);
            let indices = &key.path.as_ref()?.indices;
            let path = hw_protocol::format_path(indices);
            hw_protocol::parse_path(&path).ok()?;
            Some((pubkey, path))
        })
        .enumerate()
        .map(|(index, (pubkey, path))| DeviceKey {
            index: index as u32,
            path,
            pubkey,
        })
        .collect();
    if keys.is_empty() {
        return Err("That export holds no Solana account. Choose Solflare on the Keystone, then scan again.".into());
    }
    Ok(DeviceKeys {
        vendor: HardwareVendor::Keystone,
        keys,
        seen: DeviceSeen {
            model: export.device.clone(),
            firmware,
        },
        device: Some(format!("{:08x}", export.master_fingerprint)),
    })
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
    if vendor == HardwareVendor::SeedVault {
        return read_seed_vault_keys(run, Authorize::Existing, scheme, start, count).await;
    }
    if let Err(why) = over_cable(vendor).and(signs_with(vendor)) {
        run.fail(&why);
        return Err(why);
    }
    let outcome = async {
        let mut ready = ready(run, vendor).await?;
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
            let pubkey = ready.pubkey(&path, false).await?;
            keys.push(DeviceKey {
                index,
                path,
                pubkey,
            });
        }
        let device = match &ready {
            Ready::Trezor { device_id, .. } => device_id.clone(),
            Ready::Ledger(_) => None,
        };
        Ok(DeviceKeys {
            vendor,
            keys,
            seen: ready.seen().clone(),
            device,
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

/// What the dialog says for a Seed Vault error. Backing out of the Seed
/// Vault's own screen is the user's answer, not an error: the job ends as
/// a closed dialog does.
fn vault_said(error: SeedVaultError) -> String {
    match error {
        SeedVaultError::Declined => CANCELLED.to_string(),
        error => error.to_string(),
    }
}

/// Whether this phone has a Seed Vault Add wallet can read keys from.
pub fn has_seed_vault(session: Session) -> bool {
    hub(session).is_ok_and(|hub| hub.seed_vault().is_some())
}

/// The seed the last Add wallet read was authorized for, so reading the
/// next accounts does not ask the user again.
static SEED_TOKEN: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

fn remembered_token() -> Option<u64> {
    SEED_TOKEN.lock().ok().and_then(|token| *token)
}

fn remember_token(token: Option<u64>) {
    if let Ok(mut held) = SEED_TOKEN.lock() {
        *held = token;
    }
}

/// Seed Vault keys for Add wallet: get the use of a seed (`how`: pick one,
/// import a phrase into the Seed Vault, or have it make one), then read the
/// keys `start..start + count` of `scheme` from it. Reading the next page
/// asks again only if the Seed Vault no longer knows the seed.
pub async fn read_seed_vault_keys_for(
    session: Session,
    how: Authorize,
    scheme: PathScheme,
    start: u32,
    count: u32,
) -> Result<DeviceKeys, String> {
    let run = Run::start(
        session,
        HardwareVendor::SeedVault,
        DeviceAsk::Read {
            scheme: scheme.template.to_string(),
        },
    );
    read_seed_vault_keys(run, how, scheme, start, count).await
}

async fn read_seed_vault_keys(
    run: Run,
    how: Authorize,
    scheme: PathScheme,
    start: u32,
    count: u32,
) -> Result<DeviceKeys, String> {
    let outcome = async {
        let hub = hub(run.session)?;
        let vault = hub
            .seed_vault()
            .ok_or_else(|| "This phone has no Seed Vault.".to_string())?;
        let indices = if scheme.template.contains('*') {
            start..start + count
        } else {
            0..1
        };
        let paths: Vec<String> = indices.clone().map(|index| scheme.path(index)).collect();
        let reuse = if how == Authorize::Existing && start > 0 {
            remembered_token()
        } else {
            None
        };
        run.stage(DeviceStage::Confirm);
        let (token, keys) = match reuse {
            Some(token) => match vault.public_keys(token, &paths).await {
                Ok(keys) => (token, keys),
                Err(SeedVaultError::NotAuthorized) => {
                    let token = vault.authorize(how).await.map_err(vault_said)?;
                    (
                        token,
                        vault.public_keys(token, &paths).await.map_err(vault_said)?,
                    )
                }
                Err(err) => return Err(vault_said(err)),
            },
            None => {
                let token = vault.authorize(how).await.map_err(vault_said)?;
                (
                    token,
                    vault.public_keys(token, &paths).await.map_err(vault_said)?,
                )
            }
        };
        if run.cancelled() {
            return Err(CANCELLED.to_string());
        }
        remember_token(Some(token));
        tracing::info!(
            target: "airlock::device",
            vendor = "Seed Vault",
            secure = vault.secure(),
            keys = keys.len(),
            "keys read"
        );
        Ok(DeviceKeys {
            vendor: HardwareVendor::SeedVault,
            keys: indices
                .zip(paths)
                .zip(keys)
                .map(|((index, path), key)| DeviceKey {
                    index,
                    path,
                    pubkey: Pubkey::new_from_array(key),
                })
                .collect(),
            seen: DeviceSeen {
                model: Some(
                    if vault.secure() {
                        "Seed Vault"
                    } else {
                        "Seed Vault simulator"
                    }
                    .into(),
                ),
                firmware: None,
            },
            device: Some(token.to_string()),
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

/// Sign `bytes` with the key a Seed Vault wallet names, on this phone's
/// Seed Vault. The user confirms on its screen; the signature is checked
/// against the key before it is answered.
async fn sign_seed_vault(
    run: Run,
    held: &OnDevice,
    account: &Pubkey,
    bytes: &[u8],
    kind: Payload,
) -> Result<[u8; 64], String> {
    let outcome = async {
        let token: u64 = held
            .device
            .as_deref()
            .and_then(|token| token.parse().ok())
            .ok_or_else(|| {
                "This wallet does not name its Seed Vault seed. Add it again from Seed Vault."
                    .to_string()
            })?;
        let hub = hub(run.session)?;
        let vault = hub.seed_vault().ok_or_else(|| {
            "This wallet's key is in a Seed Vault, and this device has none.".to_string()
        })?;
        run.stage(DeviceStage::Confirm);
        let signature = device::seed_vault_sign(vault, token, &held.path, account, bytes, kind)
            .await
            .map_err(|err| match err {
                device::SeedVaultSignError::Vault(err) => vault_said(err),
                err => err.to_string(),
            })?;
        if run.cancelled() {
            return Err(CANCELLED.to_string());
        }
        tracing::info!(
            target: "airlock::device",
            vendor = "Seed Vault",
            path = %held.path,
            pubkey = %account,
            message = kind == Payload::Message,
            "signed"
        );
        Ok(<[u8; 64]>::from(signature))
    }
    .await;
    match &outcome {
        Ok(_) => run.close(),
        Err(why) if why == CANCELLED => run.close(),
        Err(why) => {
            tracing::info!(target: "airlock::device", vendor = "Seed Vault", reason = %why, "refused");
            run.fail(why);
        }
    }
    outcome
}
