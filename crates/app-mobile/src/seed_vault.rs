//! The phone's Seed Vault, through `SeedVaultBridge` in the app's
//! `MainActivity.kt`, and the device hub that hands it to the core.
//!
//! Each request starts a Seed Vault screen over the app and returns at
//! once with an id; the result is collected by asking the bridge for that
//! id until it has one. Nothing secret crosses: auth tokens, paths, public
//! keys, the bytes to sign and the signature.

use std::sync::Arc;

use hw_protocol::link::{Connected, DeviceHub, LinkError, LinkFuture, PacketLink};
use hw_protocol::seed_vault::{Authorize, Payload, SeedVault, SeedVaultError};

/// The hub a phone gives the core: no USB devices, and its Seed Vault when
/// it has one.
pub struct PhoneHub {
    vault: Option<AndroidSeedVault>,
}

impl DeviceHub for PhoneHub {
    fn list(&self) -> LinkFuture<'_, Result<Vec<Connected>, LinkError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn open<'a>(&'a self, _id: &'a str) -> LinkFuture<'a, Result<Box<dyn PacketLink>, LinkError>> {
        Box::pin(async {
            Err(LinkError::Denied(
                "this phone does not open USB hardware wallets".into(),
            ))
        })
    }

    fn opens_cables(&self) -> bool {
        false
    }

    fn seed_vault(&self) -> Option<&dyn SeedVault> {
        self.vault.as_ref().map(|vault| vault as &dyn SeedVault)
    }
}

/// The hub for `open_core`: always one, so Add wallet knows this build
/// opens no USB device, with the Seed Vault when the phone has one.
pub fn hub() -> Option<Arc<dyn DeviceHub>> {
    let vault = AndroidSeedVault::detect();
    tracing::info!(
        target: "airlock::device",
        seed_vault = match &vault {
            None => "none",
            Some(vault) if vault.secure => "secure",
            Some(_) => "simulator",
        },
        "phone hub"
    );
    Some(Arc::new(PhoneHub { vault }))
}

/// The Seed Vault behind `SeedVaultBridge`.
pub struct AndroidSeedVault {
    secure: bool,
}

impl AndroidSeedVault {
    fn detect() -> Option<Self> {
        #[cfg(target_os = "android")]
        {
            match crate::android::bridge_availability() {
                Ok(2) => Some(Self { secure: true }),
                Ok(1) => Some(Self { secure: false }),
                Ok(_) => None,
                Err(err) => {
                    tracing::warn!(target: "airlock::device", %err, "could not ask for a Seed Vault");
                    None
                }
            }
        }
        #[cfg(not(target_os = "android"))]
        {
            None
        }
    }
}

/// How long the user may take on a Seed Vault screen.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(600);
/// How often the bridge is asked whether the screen is done.
const POLL: std::time::Duration = std::time::Duration::from_millis(150);

/// Start request `id` with `start`, then wait for its result.
async fn ask(start: impl FnOnce(i64) -> Result<(), String>) -> Result<String, SeedVaultError> {
    static NEXT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    start(id).map_err(SeedVaultError::Failed)?;
    let end = std::time::Instant::now() + PATIENCE;
    loop {
        if let Some(result) = take(id).map_err(SeedVaultError::Failed)? {
            return parse(&result);
        }
        if std::time::Instant::now() > end {
            return Err(SeedVaultError::Failed(
                "no answer from the Seed Vault".into(),
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

fn take(id: i64) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        crate::android::bridge_take(id)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = id;
        Err("not on Android".into())
    }
}

/// `ok:<text>`, or `err:<code>:<text>`.
fn parse(result: &str) -> Result<String, SeedVaultError> {
    if let Some(text) = result.strip_prefix("ok:") {
        return Ok(text.to_string());
    }
    let rest = result.strip_prefix("err:").unwrap_or(result);
    let (code, text) = rest.split_once(':').unwrap_or((rest, ""));
    Err(match code {
        "declined" => SeedVaultError::Declined,
        "permission" => SeedVaultError::NoPermission,
        "auth" => SeedVaultError::NotAuthorized,
        "noseeds" => SeedVaultError::Failed(
            "the Seed Vault has no seed yet. Use Import phrase or Create seed.".into(),
        ),
        _ => SeedVaultError::Failed(text.to_string()),
    })
}

fn decode<const N: usize>(text: &str) -> Result<[u8; N], SeedVaultError> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| {
            SeedVaultError::Failed("the Seed Vault answered something unreadable".into())
        })?;
    <[u8; N]>::try_from(bytes.as_slice()).map_err(|_| {
        SeedVaultError::Failed(format!(
            "the Seed Vault answered {} bytes, not {N}",
            bytes.len()
        ))
    })
}

impl SeedVault for AndroidSeedVault {
    fn secure(&self) -> bool {
        self.secure
    }

    fn authorize(&self, how: Authorize) -> LinkFuture<'_, Result<u64, SeedVaultError>> {
        Box::pin(async move {
            let how = match how {
                Authorize::Existing => 0,
                Authorize::Import => 1,
                Authorize::Create => 2,
            };
            let token = ask(|id| start_authorize(id, how)).await?;
            token
                .trim()
                .parse::<u64>()
                .map_err(|_| SeedVaultError::Failed("the Seed Vault answered no auth token".into()))
        })
    }

    fn public_keys<'a>(
        &'a self,
        token: u64,
        paths: &'a [String],
    ) -> LinkFuture<'a, Result<Vec<[u8; 32]>, SeedVaultError>> {
        Box::pin(async move {
            let joined = paths.join("\n");
            let keys = ask(|id| start_public_keys(id, token, &joined)).await?;
            let keys: Vec<[u8; 32]> = keys
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(decode::<32>)
                .collect::<Result<_, _>>()?;
            if keys.len() != paths.len() {
                return Err(SeedVaultError::Failed(format!(
                    "asked for {} keys, the Seed Vault answered {}",
                    paths.len(),
                    keys.len()
                )));
            }
            Ok(keys)
        })
    }

    fn sign<'a>(
        &'a self,
        token: u64,
        path: &'a str,
        payload: &'a [u8],
        kind: Payload,
    ) -> LinkFuture<'a, Result<[u8; 64], SeedVaultError>> {
        Box::pin(async move {
            use base64::Engine;
            let payload = base64::engine::general_purpose::STANDARD.encode(payload);
            let message = kind == Payload::Message;
            let signature = ask(|id| start_sign(id, token, path, &payload, message)).await?;
            decode::<64>(&signature)
        })
    }
}

#[cfg(target_os = "android")]
fn start_authorize(id: i64, how: i32) -> Result<(), String> {
    crate::android::bridge_authorize(id, how)
}
#[cfg(target_os = "android")]
fn start_public_keys(id: i64, token: u64, paths: &str) -> Result<(), String> {
    crate::android::bridge_public_keys(id, token, paths)
}
#[cfg(target_os = "android")]
fn start_sign(id: i64, token: u64, path: &str, payload: &str, message: bool) -> Result<(), String> {
    crate::android::bridge_sign(id, token, path, payload, message)
}

#[cfg(not(target_os = "android"))]
fn start_authorize(_id: i64, _how: i32) -> Result<(), String> {
    Err("not on Android".into())
}
#[cfg(not(target_os = "android"))]
fn start_public_keys(_id: i64, _token: u64, _paths: &str) -> Result<(), String> {
    Err("not on Android".into())
}
#[cfg(not(target_os = "android"))]
fn start_sign(
    _id: i64,
    _token: u64,
    _path: &str,
    _payload: &str,
    _message: bool,
) -> Result<(), String> {
    Err("not on Android".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_read_as_the_bridge_writes_them() {
        assert_eq!(parse("ok:42"), Ok("42".into()));
        assert_eq!(
            parse("err:declined:cancelled"),
            Err(SeedVaultError::Declined)
        );
        assert_eq!(parse("err:auth:gone"), Err(SeedVaultError::NotAuthorized));
        assert_eq!(
            parse("err:permission:no"),
            Err(SeedVaultError::NoPermission)
        );
        assert_eq!(
            parse("err:failed:it broke"),
            Err(SeedVaultError::Failed("it broke".into()))
        );
    }

    #[test]
    fn keys_and_signatures_must_be_whole() {
        use base64::Engine;
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        assert_eq!(decode::<32>(&key), Ok([7u8; 32]));
        assert!(decode::<64>(&key).is_err());
        assert!(decode::<32>("not base64!").is_err());
    }
}
