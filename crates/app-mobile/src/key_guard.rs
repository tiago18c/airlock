//! Fingerprint unlock on Android: the [`KeyGuard`] over `KeyGuardBridge` in
//! the app's MainActivity.kt.
//!
//! Enrolling seals the vault's key with an AES key Android's Keystore makes
//! (in StrongBox where the phone has one) and lets the app use only right
//! after a strong biometric check. What it sealed, an IV and a ciphertext,
//! is kept here in the mode's data folder: on its own it opens nothing. The
//! Keystore key is destroyed when the phone's fingerprints change, and the
//! password is then the way back.

use std::path::PathBuf;
use std::sync::Arc;

use app_core::{BoxFuture, KeyGuard, KeyGuardError};
use zeroize::Zeroizing;

/// The file in the mode's data folder that holds what was sealed.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const FILE: &str = "fingerprint-unlock.txt";

/// How long the user may take on the fingerprint prompt.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(300);
const POLL: std::time::Duration = std::time::Duration::from_millis(120);

/// Fingerprint unlock for this mode's vault.
pub struct AndroidKeyGuard {
    /// The Keystore key's alias: one per mode, as each has its own vault.
    alias: String,
    file: Option<PathBuf>,
}

/// The guard, when the phone has a strong biometric enrolled. None
/// elsewhere, and then Settings offers no fingerprint unlock.
pub fn key_guard() -> Option<Arc<dyn KeyGuard>> {
    #[cfg(target_os = "android")]
    {
        let level = crate::android::guard_availability().unwrap_or_else(|err| {
            tracing::warn!(target: "airlock::vault", %err, "could not ask for biometrics");
            0
        });
        tracing::info!(
            target: "airlock::vault",
            fingerprint = match level {
                2 => "StrongBox",
                1 => "TEE",
                _ => "none",
            },
            "key guard"
        );
        if level == 0 {
            return None;
        }
        let folder = app_core::storage::app_dirs().folder;
        Some(Arc::new(AndroidKeyGuard {
            alias: format!("airlock-vault-{folder}"),
            file: app_core::storage::data_dir().map(|dir| dir.join(FILE)),
        }))
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// `ok:<text>`, or `err:<code>:<text>`.
fn parse(result: &str) -> Result<String, KeyGuardError> {
    if let Some(text) = result.strip_prefix("ok:") {
        return Ok(text.to_string());
    }
    let rest = result.strip_prefix("err:").unwrap_or(result);
    let (code, text) = rest.split_once(':').unwrap_or((rest, ""));
    Err(match code {
        "declined" => KeyGuardError::Declined,
        "invalidated" => KeyGuardError::Invalidated,
        _ => KeyGuardError::Failed(text.to_string()),
    })
}

/// What was sealed, as the file keeps it: `<iv>:<sealed>:<StrongBox|TEE>`.
#[derive(Debug, PartialEq, Eq)]
struct Sealed {
    iv: String,
    sealed: String,
    hardware: String,
}

impl Sealed {
    fn read(text: &str) -> Option<Self> {
        let mut parts = text.trim().splitn(3, ':');
        let iv = parts.next()?.to_string();
        let sealed = parts.next()?.to_string();
        let hardware = parts.next().unwrap_or("TEE").to_string();
        (!iv.is_empty() && !sealed.is_empty()).then_some(Self {
            iv,
            sealed,
            hardware,
        })
    }

    fn write(&self) -> String {
        format!("{}:{}:{}", self.iv, self.sealed, self.hardware)
    }
}

/// Start request `id` with `start`, then wait for its result.
async fn ask(
    start: impl FnOnce(i64) -> Result<(), String>,
) -> Result<(i64, String), KeyGuardError> {
    static NEXT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    start(id).map_err(KeyGuardError::Failed)?;
    let end = std::time::Instant::now() + PATIENCE;
    loop {
        if let Some(result) = take(id).map_err(KeyGuardError::Failed)? {
            return parse(&result).map(|text| (id, text));
        }
        if std::time::Instant::now() > end {
            return Err(KeyGuardError::Failed(
                "no answer from the fingerprint prompt".into(),
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

fn take(id: i64) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        crate::android::guard_take(id)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = id;
        Err("not on Android".into())
    }
}

impl AndroidKeyGuard {
    fn sealed(&self) -> Option<Sealed> {
        let text = std::fs::read_to_string(self.file.as_ref()?).ok()?;
        Sealed::read(&text)
    }

    fn drop_file(&self) {
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }
}

impl KeyGuard for AndroidKeyGuard {
    fn enrolled(&self) -> bool {
        self.sealed().is_some()
    }

    fn enroll(&self, key: Zeroizing<Vec<u8>>) -> BoxFuture<Result<(), KeyGuardError>> {
        let alias = self.alias.clone();
        let file = self.file.clone();
        Box::pin(async move {
            let file = file.ok_or_else(|| KeyGuardError::Failed("no data folder".into()))?;
            let (_, text) = ask(|id| start_enroll(id, &alias, &key)).await?;
            drop(key);
            let sealed = Sealed::read(&text).ok_or_else(|| {
                KeyGuardError::Failed("the prompt answered nothing to keep".into())
            })?;
            std::fs::write(&file, sealed.write())
                .map_err(|err| KeyGuardError::Failed(err.to_string()))?;
            tracing::info!(target: "airlock::vault", hardware = %sealed.hardware, "fingerprint unlock on");
            Ok(())
        })
    }

    fn open(&self) -> BoxFuture<Result<Zeroizing<Vec<u8>>, KeyGuardError>> {
        let alias = self.alias.clone();
        let sealed = self.sealed();
        let file = self.file.clone();
        Box::pin(async move {
            let sealed = sealed.ok_or(KeyGuardError::Invalidated)?;
            match ask(|id| start_open(id, &alias, &sealed.iv, &sealed.sealed)).await {
                Ok((id, _)) => take_secret(id)?
                    .ok_or_else(|| KeyGuardError::Failed("the prompt gave no key back".into())),
                Err(KeyGuardError::Invalidated) => {
                    if let Some(file) = file {
                        let _ = std::fs::remove_file(file);
                    }
                    tracing::info!(target: "airlock::vault", "fingerprint unlock off: fingerprints changed");
                    Err(KeyGuardError::Invalidated)
                }
                Err(err) => Err(err),
            }
        })
    }

    fn forget(&self) {
        self.drop_file();
        #[cfg(target_os = "android")]
        if let Err(err) = crate::android::guard_forget(&self.alias) {
            tracing::warn!(target: "airlock::vault", %err, "could not delete the fingerprint key");
        }
        tracing::info!(target: "airlock::vault", "fingerprint unlock off");
    }
}

#[cfg(target_os = "android")]
fn start_enroll(id: i64, alias: &str, key: &[u8]) -> Result<(), String> {
    crate::android::guard_enroll(id, alias, key)
}
#[cfg(target_os = "android")]
fn start_open(id: i64, alias: &str, iv: &str, sealed: &str) -> Result<(), String> {
    crate::android::guard_open(id, alias, iv, sealed)
}
#[cfg(target_os = "android")]
fn take_secret(id: i64) -> Result<Option<Zeroizing<Vec<u8>>>, KeyGuardError> {
    crate::android::guard_take_secret(id).map_err(KeyGuardError::Failed)
}

#[cfg(not(target_os = "android"))]
fn start_enroll(_id: i64, _alias: &str, _key: &[u8]) -> Result<(), String> {
    Err("not on Android".into())
}
#[cfg(not(target_os = "android"))]
fn start_open(_id: i64, _alias: &str, _iv: &str, _sealed: &str) -> Result<(), String> {
    Err("not on Android".into())
}
#[cfg(not(target_os = "android"))]
fn take_secret(_id: i64) -> Result<Option<Zeroizing<Vec<u8>>>, KeyGuardError> {
    Err(KeyGuardError::Failed("not on Android".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_sealed_reads_back_as_written() {
        let sealed = Sealed {
            iv: "aXY=".into(),
            sealed: "c2VhbGVk".into(),
            hardware: "StrongBox".into(),
        };
        assert_eq!(Sealed::read(&sealed.write()), Some(sealed));
        assert_eq!(Sealed::read(""), None);
        assert_eq!(Sealed::read("only-one"), None);
    }

    #[test]
    fn results_read_as_the_bridge_writes_them() {
        assert_eq!(
            parse("ok:aXY=:c2VhbGVk:TEE"),
            Ok("aXY=:c2VhbGVk:TEE".into())
        );
        assert_eq!(
            parse("err:declined:Cancelled"),
            Err(KeyGuardError::Declined)
        );
        assert_eq!(parse("err:invalidated:"), Err(KeyGuardError::Invalidated));
        assert_eq!(
            parse("err:failed:no"),
            Err(KeyGuardError::Failed("no".into()))
        );
    }
}
