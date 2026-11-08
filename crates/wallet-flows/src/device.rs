//! What Airlock asks a hardware wallet, over a link a transport opened: get
//! the Solana app open, read a key, sign. Each use case takes the link and
//! plain data, so the same code runs on a real device, on an emulator, and
//! on the scripted fake in the tests.
//!
//! The device decides for its key. A signature that comes back is checked
//! against the key and the bytes before anything uses it.

use hw_protocol::ledger::{self, AppConfiguration, LedgerError};
use hw_protocol::link::{exchange_apdu, exchange_message, ExchangeError, PacketLink};
use hw_protocol::trezor::{self, Features, Reply, TrezorError};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use thiserror::Error;
use wallet_core::{HardwareVendor, Version};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DeviceError {
    #[error("{0}")]
    Exchange(#[from] ExchangeError),
    /// Below the first version that reads v1.
    #[error("{0}")]
    TooOld(String),
    /// The device holds another key at this path: another device, or another
    /// passphrase.
    #[error("This {vendor} does not hold {expected} at {path}. Is it another device, or another passphrase?")]
    WrongKey {
        vendor: &'static str,
        expected: Pubkey,
        path: String,
    },
    /// The signature does not verify against the key and the bytes.
    #[error("The {0} returned a signature that does not match. Nothing was signed with it.")]
    BadSignature(&'static str),
    #[error("{0}")]
    Path(String),
}

impl From<TrezorError> for DeviceError {
    fn from(error: TrezorError) -> Self {
        Self::Exchange(ExchangeError::Trezor(error))
    }
}

impl From<LedgerError> for DeviceError {
    fn from(error: LedgerError) -> Self {
        Self::Exchange(ExchangeError::Ledger(error))
    }
}

impl DeviceError {
    /// The user said no on the device.
    pub fn is_rejection(&self) -> bool {
        matches!(
            self,
            Self::Exchange(ExchangeError::Ledger(LedgerError::Rejected))
                | Self::Exchange(ExchangeError::Trezor(
                    hw_protocol::trezor::TrezorError::Cancelled
                ))
        )
    }
}

fn indices(path: &str) -> Result<Vec<u32>, DeviceError> {
    hw_protocol::parse_path(path).map_err(|error| DeviceError::Path(error.to_string()))
}

async fn ledger_call(
    link: &mut dyn PacketLink,
    apdu: &ledger::Apdu,
) -> Result<Vec<u8>, DeviceError> {
    let reply = exchange_apdu(link, &apdu.to_bytes()).await?;
    Ok(ledger::check_reply(&reply)?.to_vec())
}

/// Where the Solana app stands on a Ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerApp {
    /// It is open, at a version Airlock signs with.
    Ready(AppConfiguration),
    /// The dashboard is showing: [`ledger_open_app`] asks the device to open it.
    Dashboard,
    /// Another app is open: the user quits it first.
    OtherApp,
}

/// Which app is open, and whether the Solana app is one Airlock signs with.
pub async fn ledger_app(link: &mut dyn PacketLink) -> Result<LedgerApp, DeviceError> {
    let app =
        ledger::parse_app_and_version(&ledger_call(link, &ledger::get_app_and_version()).await?)?;
    match app.name.as_str() {
        "BOLOS" => return Ok(LedgerApp::Dashboard),
        "Solana" => {}
        _ => return Ok(LedgerApp::OtherApp),
    }
    let config =
        ledger::parse_configuration(&ledger_call(link, &ledger::get_configuration()).await?)?;
    let (major, minor, patch) = config.version;
    let version = Version::new(major.into(), minor.into(), patch.into());
    if let Some(refusal) = HardwareVendor::Ledger.refusal(version) {
        return Err(DeviceError::TooOld(refusal));
    }
    Ok(LedgerApp::Ready(config))
}

/// Ask the Ledger to open the Solana app. The user approves it there; this
/// returns once they have, or refused.
pub async fn ledger_open_app(link: &mut dyn PacketLink) -> Result<(), DeviceError> {
    ledger_call(link, &ledger::open_app()).await.map(|_| ())
}

/// The key at `path`. With `confirm`, the device shows the address and the
/// user approves it there.
pub async fn ledger_pubkey(
    link: &mut dyn PacketLink,
    path: &str,
    confirm: bool,
) -> Result<Pubkey, DeviceError> {
    let reply = ledger_call(link, &ledger::get_pubkey(&indices(path)?, confirm)).await?;
    Ok(Pubkey::new_from_array(ledger::parse_pubkey(&reply)?))
}

/// The key at `path` is `expected`, or the device is not the one this
/// wallet was added from.
pub async fn ledger_check_key(
    link: &mut dyn PacketLink,
    path: &str,
    expected: &Pubkey,
) -> Result<(), DeviceError> {
    let found = ledger_pubkey(link, path, false).await?;
    if found != *expected {
        return Err(DeviceError::WrongKey {
            vendor: "Ledger",
            expected: *expected,
            path: path.to_string(),
        });
    }
    Ok(())
}

/// Sign `message`, the bytes a transaction's signatures cover, with the key
/// at `path`, which has to be `signer`. Returns once the user confirms on
/// the device, with a signature already verified.
pub async fn ledger_sign(
    link: &mut dyn PacketLink,
    path: &str,
    signer: &Pubkey,
    message: &[u8],
) -> Result<Signature, DeviceError> {
    sign_chunks(
        link,
        ledger::sign_message(&indices(path)?, message),
        signer,
        message,
    )
    .await
}

/// Sign an off-chain message, already in the OCMS v1 format, with the key at
/// `path`. The signature covers the OCMS bytes.
pub async fn ledger_sign_offchain(
    link: &mut dyn PacketLink,
    path: &str,
    signer: &Pubkey,
    ocms: &[u8],
) -> Result<Signature, DeviceError> {
    sign_chunks(
        link,
        ledger::sign_offchain_message(&indices(path)?, ocms),
        signer,
        ocms,
    )
    .await
}

async fn sign_chunks(
    link: &mut dyn PacketLink,
    chunks: Vec<ledger::Apdu>,
    signer: &Pubkey,
    signed: &[u8],
) -> Result<Signature, DeviceError> {
    let mut reply = Vec::new();
    for chunk in &chunks {
        reply = ledger_call(link, chunk).await?;
    }
    let signature = Signature::from(ledger::parse_signature(&reply)?);
    if !signature.verify(signer.as_ref(), signed) {
        return Err(DeviceError::BadSignature("Ledger"));
    }
    Ok(signature)
}

/// What a Trezor is, from the Features it answers a new session with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrezorReady {
    pub features: Features,
    pub version: Version,
}

/// The id of `PinMatrixRequest`: a PIN typed on the host, which only a
/// Model One asks for.
const PIN_MATRIX_REQUEST: u16 = 18;

/// Send `message` and answer what the Trezor asks on the way: each button
/// it waits on, and the passphrase, which is typed on the device. Returns
/// the reply once it is not one of those.
pub async fn trezor_call(
    link: &mut dyn PacketLink,
    message: &trezor::Message,
) -> Result<Reply, DeviceError> {
    let mut next = message.clone();
    // Enough for every screen of a long transaction, and an end to a loop.
    for _ in 0..256 {
        let answer = exchange_message(link, &next).await?;
        if answer.kind == PIN_MATRIX_REQUEST {
            return Err(TrezorError::PinExpected.into());
        }
        match trezor::parse_reply(&answer)? {
            Reply::ButtonRequest { .. } => next = trezor::button_ack(),
            Reply::PassphraseRequest => next = trezor::passphrase_on_device(),
            reply => return Ok(reply),
        }
    }
    Err(TrezorError::BadReply.into())
}

/// Start a session, and refuse a Trezor in its bootloader, one locked, one
/// that needs its PIN typed on the host, or one below the first firmware
/// that reads v1.
pub async fn trezor_ready(link: &mut dyn PacketLink) -> Result<TrezorReady, DeviceError> {
    let Reply::Features(features) = trezor_call(link, &trezor::initialize()).await? else {
        return Err(TrezorError::BadReply.into());
    };
    if features.bootloader {
        return Err(DeviceError::TooOld(
            "This Trezor is in its bootloader. Restart it, then try again.".into(),
        ));
    }
    let (major, minor, patch) = features.version;
    let version = Version::new(
        u16::try_from(major).unwrap_or(u16::MAX),
        u16::try_from(minor).unwrap_or(u16::MAX),
        u16::try_from(patch).unwrap_or(u16::MAX),
    );
    if let Some(refusal) = HardwareVendor::Trezor.refusal(version) {
        return Err(DeviceError::TooOld(refusal));
    }
    // A Safe 7 speaks THP, and its Codec v1 session signs nothing.
    if features.internal_model.as_deref() == Some("T3W1") {
        return Err(DeviceError::TooOld(
            "A Trezor Safe 7 talks over THP, which this build of Airlock does not speak yet."
                .into(),
        ));
    }
    Ok(TrezorReady { features, version })
}

/// The key at `path`. With `show`, the Trezor shows its address and the
/// user confirms it there.
pub async fn trezor_pubkey(
    link: &mut dyn PacketLink,
    path: &str,
    show: bool,
) -> Result<Pubkey, DeviceError> {
    match trezor_call(link, &trezor::solana_get_public_key(&indices(path)?, show)).await? {
        Reply::PublicKey(key) => Ok(Pubkey::new_from_array(key)),
        _ => Err(TrezorError::BadReply.into()),
    }
}

/// The key at `path` is `expected`, or this is another Trezor, or another
/// passphrase.
pub async fn trezor_check_key(
    link: &mut dyn PacketLink,
    path: &str,
    expected: &Pubkey,
) -> Result<(), DeviceError> {
    let found = trezor_pubkey(link, path, false).await?;
    if found != *expected {
        return Err(DeviceError::WrongKey {
            vendor: "Trezor",
            expected: *expected,
            path: path.to_string(),
        });
    }
    Ok(())
}

/// Sign `message`, the bytes a transaction's signatures cover, with the key
/// at `path`. Token accounts are shown by address: Airlock names them in
/// its own review (D7).
pub async fn trezor_sign(
    link: &mut dyn PacketLink,
    path: &str,
    signer: &Pubkey,
    message: &[u8],
) -> Result<Signature, DeviceError> {
    let sign = trezor::solana_sign_tx(&indices(path)?, message, &[]);
    let Reply::Signature(signature) = trezor_call(link, &sign).await? else {
        return Err(TrezorError::BadReply.into());
    };
    let signature = Signature::from(signature);
    if !signature.verify(signer.as_ref(), message) {
        return Err(DeviceError::BadSignature("Trezor"));
    }
    Ok(signature)
}

/// Sign `text` as an offchain message, version 1, naming `signer` alone.
/// The Trezor writes the message itself: it has to be `ocms`, the bytes
/// the site gets back, and the signature has to cover them.
pub async fn trezor_sign_offchain(
    link: &mut dyn PacketLink,
    path: &str,
    signer: &Pubkey,
    ocms: &[u8],
) -> Result<Signature, DeviceError> {
    let (signers, text) = wallet_core::site_message::read_offchain_v1(ocms)
        .ok_or(DeviceError::BadSignature("Trezor"))?;
    let text = std::str::from_utf8(text).map_err(|_| DeviceError::BadSignature("Trezor"))?;
    let keys: Vec<[u8; 32]> = signers.iter().map(Pubkey::to_bytes).collect();
    let sign = trezor::solana_sign_message(&indices(path)?, text, &keys);
    let Reply::MessageSignature { signature, signed } = trezor_call(link, &sign).await? else {
        return Err(TrezorError::BadReply.into());
    };
    if signed.as_deref().is_some_and(|signed| signed != ocms) {
        return Err(DeviceError::BadSignature("Trezor"));
    }
    let signature = Signature::from(signature);
    if !signature.verify(signer.as_ref(), ocms) {
        return Err(DeviceError::BadSignature("Trezor"));
    }
    Ok(signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hw_protocol::link::fake::{FakeLedger, Step};
    use wallet_core::{KeypairSigner, WalletSigner};

    const PATH: &str = "m/44'/501'/0'/0'";

    fn ok(data: &[u8]) -> Step {
        let mut reply = data.to_vec();
        reply.extend_from_slice(&[0x90, 0x00]);
        Step {
            expect: None,
            reply: Ok(reply),
        }
    }

    fn status(word: u16) -> Step {
        Step {
            expect: None,
            reply: Ok(word.to_be_bytes().to_vec()),
        }
    }

    fn app(name: &str, version: &str) -> Step {
        let mut data = vec![1, name.len() as u8];
        data.extend_from_slice(name.as_bytes());
        data.push(version.len() as u8);
        data.extend_from_slice(version.as_bytes());
        ok(&data)
    }

    #[test]
    fn the_solana_app_is_found_open_and_its_version_checked() {
        let mut ledger = FakeLedger::new(vec![app("Solana", "1.18.0"), ok(&[1, 0, 1, 18, 0])]);
        let found = pollster::block_on(ledger_app(&mut ledger)).unwrap();
        assert!(matches!(found, LedgerApp::Ready(config) if config.blind_signing));

        let mut ledger = FakeLedger::new(vec![app("Solana", "1.16.1"), ok(&[0, 0, 1, 16, 1])]);
        let error = pollster::block_on(ledger_app(&mut ledger)).unwrap_err();
        assert!(matches!(error, DeviceError::TooOld(text) if text.contains("1.18.0")));

        let mut ledger = FakeLedger::new(vec![app("BOLOS", "1.6.1")]);
        assert_eq!(
            pollster::block_on(ledger_app(&mut ledger)),
            Ok(LedgerApp::Dashboard)
        );
        let mut ledger = FakeLedger::new(vec![app("Ethereum", "1.10.0")]);
        assert_eq!(
            pollster::block_on(ledger_app(&mut ledger)),
            Ok(LedgerApp::OtherApp)
        );
    }

    #[test]
    fn the_key_at_the_path_has_to_be_the_wallets() {
        let key = KeypairSigner::new_random();
        let mut ledger = FakeLedger::new(vec![ok(key.pubkey().as_ref())]);
        pollster::block_on(ledger_check_key(&mut ledger, PATH, &key.pubkey())).unwrap();
        assert_eq!(
            ledger.received[0],
            ledger::get_pubkey(&hw_protocol::parse_path(PATH).unwrap(), false).to_bytes()
        );

        let other = KeypairSigner::new_random();
        let mut ledger = FakeLedger::new(vec![ok(other.pubkey().as_ref())]);
        let error =
            pollster::block_on(ledger_check_key(&mut ledger, PATH, &key.pubkey())).unwrap_err();
        assert!(matches!(error, DeviceError::WrongKey { .. }));
    }

    #[test]
    fn a_signature_is_checked_before_it_is_used() {
        let key = KeypairSigner::new_random();
        let message = vec![5u8; 700];
        let signature = key.try_sign_message(&message).unwrap();
        // Three chunks: the first two are acknowledged, the last carries the signature.
        let mut ledger = FakeLedger::new(vec![ok(&[]), ok(&[]), ok(signature.as_ref())]);
        let got =
            pollster::block_on(ledger_sign(&mut ledger, PATH, &key.pubkey(), &message)).unwrap();
        assert_eq!(got, signature);
        assert_eq!(ledger.received.len(), 3);

        // A signature over other bytes is refused.
        let wrong = key.try_sign_message(b"something else").unwrap();
        let mut ledger = FakeLedger::new(vec![ok(&[]), ok(&[]), ok(wrong.as_ref())]);
        let error = pollster::block_on(ledger_sign(&mut ledger, PATH, &key.pubkey(), &message))
            .unwrap_err();
        assert_eq!(error, DeviceError::BadSignature("Ledger"));
    }

    /// A Trezor that answers each message with the next of `replies`.
    struct FakeTrezor {
        replies: std::collections::VecDeque<trezor::Message>,
        unframer: trezor::Unframer,
        outgoing: std::collections::VecDeque<[u8; 64]>,
        received: Vec<trezor::Message>,
    }

    impl FakeTrezor {
        fn new(replies: Vec<trezor::Message>) -> Self {
            Self {
                replies: replies.into(),
                unframer: trezor::Unframer::default(),
                outgoing: Default::default(),
                received: Vec::new(),
            }
        }
    }

    impl PacketLink for FakeTrezor {
        fn write(
            &mut self,
            packet: [u8; 64],
        ) -> hw_protocol::link::LinkFuture<'_, Result<(), hw_protocol::link::LinkError>> {
            Box::pin(async move {
                if let Some(message) = self.unframer.push(&packet).unwrap() {
                    self.unframer = trezor::Unframer::default();
                    self.received.push(message);
                    let reply = self
                        .replies
                        .pop_front()
                        .ok_or(hw_protocol::link::LinkError::Disconnected)?;
                    self.outgoing.extend(trezor::frame(&reply));
                }
                Ok(())
            })
        }

        fn read(
            &mut self,
        ) -> hw_protocol::link::LinkFuture<'_, Result<[u8; 64], hw_protocol::link::LinkError>>
        {
            Box::pin(async move {
                self.outgoing
                    .pop_front()
                    .ok_or(hw_protocol::link::LinkError::Disconnected)
            })
        }
    }

    fn features(major: u64, minor: u64, patch: u64, internal: &str) -> trezor::Message {
        let mut body = Vec::new();
        for (field, value) in [(2u8, major), (3, minor), (4, patch)] {
            body.extend_from_slice(&[field << 3, value as u8]);
        }
        // Field 44, length-delimited: its key is two bytes.
        body.extend_from_slice(&[0xe2, 0x02]);
        body.push(internal.len() as u8);
        body.extend_from_slice(internal.as_bytes());
        trezor::Message::new(17, body)
    }

    fn reply(kind: u16, field: u8, bytes: &[u8]) -> trezor::Message {
        let mut body = vec![field << 3 | 2, bytes.len() as u8];
        body.extend_from_slice(bytes);
        trezor::Message::new(kind, body)
    }

    #[test]
    fn a_trezor_session_answers_buttons_and_the_passphrase_on_the_device() {
        let key = KeypairSigner::new_random();
        let message = vec![3u8; 200];
        let signature = key.try_sign_message(&message).unwrap();
        let mut trezor = FakeTrezor::new(vec![
            features(2, 13, 1, "T3T1"),
            reply(901, 1, key.pubkey().as_ref()),
            trezor::Message::new(26, Vec::new()),
            trezor::Message::new(41, Vec::new()),
            trezor::Message::new(26, Vec::new()),
            reply(905, 1, signature.as_ref()),
        ]);
        let ready = pollster::block_on(trezor_ready(&mut trezor)).unwrap();
        assert_eq!(ready.version, Version::new(2, 13, 1));
        pollster::block_on(trezor_check_key(&mut trezor, PATH, &key.pubkey())).unwrap();
        let got =
            pollster::block_on(trezor_sign(&mut trezor, PATH, &key.pubkey(), &message)).unwrap();
        assert_eq!(got, signature);
        let kinds: Vec<u16> = trezor.received.iter().map(|m| m.kind).collect();
        // Initialize, the key, the transaction, then an ack, the passphrase
        // on the device, and an ack.
        assert_eq!(kinds, [0, 900, 904, 27, 42, 27]);
    }

    #[test]
    fn a_trezor_below_the_first_firmware_that_reads_v1_or_a_safe_7_is_refused() {
        let mut old = FakeTrezor::new(vec![features(2, 8, 9, "T2T1")]);
        let error = pollster::block_on(trezor_ready(&mut old)).unwrap_err();
        assert!(matches!(error, DeviceError::TooOld(text) if text.contains("2.13.1")));
        let mut thp = FakeTrezor::new(vec![features(2, 14, 0, "T3W1")]);
        let error = pollster::block_on(trezor_ready(&mut thp)).unwrap_err();
        assert!(matches!(error, DeviceError::TooOld(text) if text.contains("THP")));
    }

    #[test]
    fn a_trezor_offchain_message_has_to_be_the_one_the_site_gets_back() {
        let key = KeypairSigner::new_random();
        let ocms = wallet_core::site_message::offchain_v1(&[key.pubkey()], b"hello");
        let signature = key.try_sign_message(&ocms).unwrap();
        let mut answer = vec![(1 << 3) | 2, 64];
        answer.extend_from_slice(signature.as_ref());
        answer.extend_from_slice(&[(2 << 3) | 2, ocms.len() as u8]);
        answer.extend_from_slice(&ocms);
        let mut trezor = FakeTrezor::new(vec![trezor::Message::new(907, answer)]);
        let got = pollster::block_on(trezor_sign_offchain(
            &mut trezor,
            PATH,
            &key.pubkey(),
            &ocms,
        ))
        .unwrap();
        assert_eq!(got, signature);

        // It signed other bytes than the site would get.
        let other = wallet_core::site_message::offchain_v1(&[key.pubkey()], b"other");
        let signature = key.try_sign_message(&other).unwrap();
        let mut answer = vec![(1 << 3) | 2, 64];
        answer.extend_from_slice(signature.as_ref());
        answer.extend_from_slice(&[(2 << 3) | 2, other.len() as u8]);
        answer.extend_from_slice(&other);
        let mut trezor = FakeTrezor::new(vec![trezor::Message::new(907, answer)]);
        let error = pollster::block_on(trezor_sign_offchain(
            &mut trezor,
            PATH,
            &key.pubkey(),
            &ocms,
        ))
        .unwrap_err();
        assert_eq!(error, DeviceError::BadSignature("Trezor"));
    }

    #[test]
    fn a_rejection_on_the_device_is_told_apart() {
        let key = KeypairSigner::new_random();
        let mut ledger = FakeLedger::new(vec![status(0x6985)]);
        let error =
            pollster::block_on(ledger_sign(&mut ledger, PATH, &key.pubkey(), &[1, 2])).unwrap_err();
        assert!(error.is_rejection());
        let mut ledger = FakeLedger::new(vec![status(0x6808)]);
        let error =
            pollster::block_on(ledger_sign(&mut ledger, PATH, &key.pubkey(), &[1, 2])).unwrap_err();
        assert!(!error.is_rejection());
        assert!(error.to_string().contains("Blind signing"));
    }
}

/// Sign `bytes` with the key at `path` of the seed `token` names in the
/// phone's Seed Vault, once the user confirms on the Seed Vault's screen.
/// The signature is checked against `signer` and the bytes, as a device's.
pub async fn seed_vault_sign(
    vault: &dyn hw_protocol::seed_vault::SeedVault,
    token: u64,
    path: &str,
    signer: &Pubkey,
    bytes: &[u8],
    kind: hw_protocol::seed_vault::Payload,
) -> Result<Signature, SeedVaultSignError> {
    let signature = Signature::from(vault.sign(token, path, bytes, kind).await?);
    if !signature.verify(signer.as_ref(), bytes) {
        return Err(SeedVaultSignError::Device(DeviceError::BadSignature(
            "Seed Vault",
        )));
    }
    Ok(signature)
}

/// Why a Seed Vault signature is not there: refused, or wrong.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SeedVaultSignError {
    #[error("{0}")]
    Vault(#[from] hw_protocol::seed_vault::SeedVaultError),
    #[error("{0}")]
    Device(#[from] DeviceError),
}
