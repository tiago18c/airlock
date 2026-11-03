//! What Airlock asks a hardware wallet, over a link a transport opened: get
//! the Solana app open, read a key, sign. Each use case takes the link and
//! plain data, so the same code runs on a real device, on an emulator, and
//! on the scripted fake in the tests.
//!
//! The device decides for its key. A signature that comes back is checked
//! against the key and the bytes before anything uses it.

use hw_protocol::ledger::{self, AppConfiguration, LedgerError};
use hw_protocol::link::{exchange_apdu, ExchangeError, PacketLink};
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
