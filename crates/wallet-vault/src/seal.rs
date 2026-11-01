//! A password-encrypted file, written as JSON. Argon2id derives the key, and
//! XChaCha20-Poly1305 seals the bytes.
//!
//! ```json
//! {
//!   "format": "airlock-vault",
//!   "version": 1,
//!   "kdf": { "name": "argon2id", "memoryKib": 262144, "iterations": 3, "parallelism": 1, "salt": "<base64>" },
//!   "cipher": { "name": "xchacha20-poly1305", "nonce": "<base64>" },
//!   "ciphertext": "<base64>"
//! }
//! ```
//!
//! Everything but the ciphertext is the header. Its JSON, written again from
//! the values read, is the associated data of the seal: a header changed by
//! anyone without the password fails to open, the same as a wrong password.
//!
//! The key is derived once, when a vault is created or opened, and kept in a
//! [`SealKey`] while it is open. Each save seals under that key and salt with
//! a fresh random nonce, so a save does not run Argon2 and nothing has to keep
//! the password.
//!
//! The cost is what stands between a copied file and its password, so it is set
//! as high as a person will wait for: about a quarter of a second on a fast
//! desktop and about a second on an ordinary laptop. A vault sealed at less is
//! raised to it when it is next unlocked.

use crate::VaultError;
use argon2::{Algorithm, Argon2, Block, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const FORMAT: &str = "airlock-vault";
const VERSION: u32 = 1;
const KDF: &str = "argon2id";
const CIPHER: &str = "xchacha20-poly1305";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;

/// Argon2id cost stored in the header so a later change can still open this vault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KdfParams {
    /// Memory cost in kibibytes.
    m_kib: u32,
    t: u32,
    p: u32,
}

impl KdfParams {
    /// What a vault is sealed at in a real build. The library has no threads, so
    /// more lanes would only slow down this device and not an attacker's.
    #[cfg(not(target_arch = "wasm32"))]
    #[cfg_attr(feature = "test-hook", allow(dead_code))]
    const STRONG: Self = Self {
        m_kib: 256 * 1024,
        t: 3,
        p: 1,
    };
    /// A browser tab has one thread, which waits for this, and less to spare.
    #[cfg(target_arch = "wasm32")]
    #[cfg_attr(feature = "test-hook", allow(dead_code))]
    const STRONG: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };

    #[cfg(not(any(test, feature = "test-hook")))]
    const CURRENT: Self = Self::STRONG;
    /// Tests seal many vaults, side by side.
    #[cfg(any(test, feature = "test-hook"))]
    const CURRENT: Self = Self {
        m_kib: 16,
        t: 1,
        p: 1,
    };

    /// The most memory a file can ask for, in kibibytes. The header is only
    /// checked once the key is derived, so this is what stops a changed file from
    /// taking all the machine has.
    const MOST_KIB: u32 = 1024 * 1024;

    fn checked(self) -> Result<Self, VaultError> {
        if !(8..=Self::MOST_KIB).contains(&self.m_kib)
            || !(1..=32).contains(&self.t)
            || !(1..=16).contains(&self.p)
        {
            return Err(VaultError::Crypto(
                "key derivation parameters are not supported".into(),
            ));
        }
        Ok(self)
    }

    /// Whether a vault sealed at this cost is to be raised to the current one.
    fn below_current(self) -> bool {
        self.m_kib < Self::CURRENT.m_kib || self.t < Self::CURRENT.t
    }
}

/// The sealed file.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    #[serde(flatten)]
    header: Header,
    #[serde(with = "b64")]
    ciphertext: Vec<u8>,
}

/// What the file says about how it was sealed. Authenticated, not secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Header {
    format: String,
    version: u32,
    kdf: Kdf,
    cipher: Cipher,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Kdf {
    name: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    #[serde(with = "b64")]
    salt: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cipher {
    name: String,
    #[serde(with = "b64")]
    nonce: Vec<u8>,
}

impl Header {
    /// The associated data of the seal.
    fn aad(&self) -> Result<Vec<u8>, VaultError> {
        serde_json::to_vec(self).map_err(|e| VaultError::Serde(e.to_string()))
    }
}

fn derive_key(
    password: &str,
    salt: &[u8],
    params: KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
    let argon_params = Params::new(params.m_kib, params.t, params.p, Some(KEY_LEN))
        .map_err(|e| VaultError::Crypto(e.to_string()))?;
    // Asked for here: the library ends the process when there is not enough.
    let blocks = argon_params.block_count();
    let mut memory: Vec<Block> = Vec::new();
    memory
        .try_reserve_exact(blocks)
        .map_err(|_| VaultError::NoMemory)?;
    memory.resize(blocks, Block::default());
    #[cfg(feature = "test-hook")]
    crate::testing::deriving();
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    let derived =
        argon.hash_password_into_with_memory(password.as_bytes(), salt, key.as_mut(), &mut memory);
    // All of it follows from the password.
    memory.zeroize();
    derived.map_err(|e| VaultError::Crypto(e.to_string()))?;
    Ok(key)
}

/// The key of an open vault, with the salt and cost it was derived with.
/// Wiped when dropped, and never printed.
pub struct SealKey {
    params: KdfParams,
    salt: [u8; SALT_LEN],
    key: Zeroizing<[u8; KEY_LEN]>,
}

#[cfg(feature = "test-hook")]
impl Drop for SealKey {
    fn drop(&mut self) {
        // `key` is a `Zeroizing` and wipes itself right after this runs.
        crate::testing::key_wiped();
    }
}

impl SealKey {
    /// A key for a new vault: a fresh salt, at the current cost. This is what
    /// takes about a second, so it should not run where a window waits on it.
    pub fn new(password: &str) -> Result<Self, VaultError> {
        Self::with_params(password, KdfParams::CURRENT)
    }

    /// Whether what is sealed under this key is to be raised to the current cost.
    pub(crate) fn below_current(&self) -> bool {
        self.params.below_current()
    }

    /// A key at less than the current cost, as a vault from an earlier build has.
    #[cfg(test)]
    pub(crate) fn from_earlier_build(password: &str) -> Self {
        Self::with_params(
            password,
            KdfParams {
                m_kib: 8,
                t: 1,
                p: 1,
            },
        )
        .unwrap()
    }

    /// A key at more than the current cost, as a vault from a later build has.
    #[cfg(test)]
    pub(crate) fn from_later_build(password: &str) -> Self {
        Self::with_params(
            password,
            KdfParams {
                m_kib: 32,
                t: 2,
                p: 1,
            },
        )
        .unwrap()
    }

    fn with_params(password: &str, params: KdfParams) -> Result<Self, VaultError> {
        let params = params.checked()?;
        let mut salt = [0u8; SALT_LEN];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let key = derive_key(password, &salt, params)?;
        Ok(Self { params, salt, key })
    }

    /// The sealed file of `plaintext`, under a fresh nonce.
    pub(crate) fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, VaultError> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let header = Header {
            format: FORMAT.into(),
            version: VERSION,
            kdf: Kdf {
                name: KDF.into(),
                memory_kib: self.params.m_kib,
                iterations: self.params.t,
                parallelism: self.params.p,
                salt: self.salt.to_vec(),
            },
            cipher: Cipher {
                name: CIPHER.into(),
                nonce: nonce.to_vec(),
            },
        };
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_ref())
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        let aad = header.aad()?;
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        serde_json::to_vec_pretty(&Envelope { header, ciphertext })
            .map_err(|e| VaultError::Serde(e.to_string()))
    }
}

/// A sealed file, read as far as it can be without the password.
struct Sealed {
    header: Header,
    ciphertext: Vec<u8>,
    params: KdfParams,
    salt: [u8; SALT_LEN],
}

/// Why a file could not even be tried with the password.
enum Unsealable {
    /// It is not a sealed file at all.
    Damaged,
    /// It says it is another build's, and nothing here can say more about it.
    Other(VaultError),
}

impl From<Unsealable> for VaultError {
    fn from(why: Unsealable) -> Self {
        match why {
            Unsealable::Damaged => VaultError::UnsupportedVersion,
            Unsealable::Other(err) => err,
        }
    }
}

impl Sealed {
    fn read(file: &[u8]) -> Result<Self, Unsealable> {
        let Envelope { header, ciphertext } =
            serde_json::from_slice(file).map_err(|_| Unsealable::Damaged)?;
        if header.format != FORMAT
            || header.version != VERSION
            || header.kdf.name != KDF
            || header.cipher.name != CIPHER
        {
            return Err(Unsealable::Other(VaultError::UnsupportedVersion));
        }
        let salt: [u8; SALT_LEN] = header
            .kdf
            .salt
            .as_slice()
            .try_into()
            .map_err(|_| Unsealable::Damaged)?;
        if header.cipher.nonce.len() != NONCE_LEN {
            return Err(Unsealable::Damaged);
        }
        let params = KdfParams {
            m_kib: header.kdf.memory_kib,
            t: header.kdf.iterations,
            p: header.kdf.parallelism,
        }
        .checked()
        .map_err(Unsealable::Other)?;
        Ok(Self {
            header,
            ciphertext,
            params,
            salt,
        })
    }

    fn derive(&self, password: &str) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
        derive_key(password, &self.salt, self.params)
    }

    fn decrypt(&self, key: &[u8; KEY_LEN]) -> Result<Vec<u8>, VaultError> {
        let cipher = XChaCha20Poly1305::new_from_slice(key)
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        let aad = self.header.aad()?;
        cipher
            .decrypt(
                XNonce::from_slice(&self.header.cipher.nonce),
                Payload {
                    msg: &self.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| VaultError::UnlockFailed)
    }

    /// The vault this opens, under `key`.
    fn opened(
        self,
        key: Zeroizing<[u8; KEY_LEN]>,
        plaintext: Vec<u8>,
        from_backup: bool,
    ) -> Opened {
        Opened {
            key: SealKey {
                params: self.params,
                salt: self.salt,
                key,
            },
            plaintext,
            from_backup,
        }
    }
}

/// What opening a stored vault found.
pub(crate) struct Opened {
    /// For the saves after.
    pub key: SealKey,
    pub plaintext: Vec<u8>,
    /// The file was damaged, and this is what its backup holds.
    pub from_backup: bool,
}

/// Open a sealed file with its password.
pub(crate) fn open(password: &str, file: &[u8]) -> Result<Opened, VaultError> {
    open_or_backup(password, file, None)
}

/// Open `file` with its password, or `backup` when `file` is damaged.
///
/// Damage to the ciphertext and a wrong password look the same, so a file
/// that does not open is tried against its backup. Both are sealed under one
/// key, so that takes no second derivation and a wrong password costs what it
/// always did. A backup under another salt is from before a password change,
/// and the old password must not open anything: it is tried only when `file`
/// is not a sealed file at all, with no salt to compare.
pub(crate) fn open_or_backup(
    password: &str,
    file: &[u8],
    backup: Option<&[u8]>,
) -> Result<Opened, VaultError> {
    let older = backup.and_then(|bytes| Sealed::read(bytes).ok());
    let sealed = match (Sealed::read(file), older) {
        (Ok(sealed), older) => (sealed, older),
        (Err(Unsealable::Damaged), Some(older)) => {
            let key = older.derive(password)?;
            let plaintext = older.decrypt(&key)?;
            return Ok(older.opened(key, plaintext, true));
        }
        (Err(why), _) => return Err(why.into()),
    };
    let (sealed, older) = sealed;
    let key = sealed.derive(password)?;
    match sealed.decrypt(&key) {
        Ok(plaintext) => Ok(sealed.opened(key, plaintext, false)),
        Err(err) => match older {
            Some(older) if older.salt == sealed.salt && older.params == sealed.params => {
                let plaintext = older.decrypt(&key).map_err(|_| err)?;
                Ok(older.opened(key, plaintext, true))
            }
            _ => Err(err),
        },
    }
}

mod b64 {
    use super::*;
    use serde::{de::Error, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: KdfParams = KdfParams {
        m_kib: 8,
        t: 1,
        p: 1,
    };

    fn json(file: &[u8]) -> serde_json::Value {
        serde_json::from_slice(file).unwrap()
    }

    fn small(plaintext: &[u8]) -> Vec<u8> {
        SealKey::with_params("pw", SMALL)
            .unwrap()
            .seal(plaintext)
            .unwrap()
    }

    #[test]
    fn a_real_build_asks_for_what_it_says_and_derives_what_it_always_will() {
        const {
            assert!(KdfParams::STRONG.m_kib >= 256 * 1024);
            assert!(KdfParams::STRONG.t >= 3);
            assert!(KdfParams::STRONG.p == 1);
        }
        assert_eq!(KdfParams::STRONG.checked().ok(), Some(KdfParams::STRONG));
        // Whatever updates the library, what was sealed has to open. Worked out
        // with the library's own allocation.
        let key = derive_key("correct horse battery", &[7; SALT_LEN], KdfParams::STRONG).unwrap();
        assert_eq!(
            hex(key.as_ref()),
            "e99b7c4f3ffa4ded3e5ff6c9239ead9fa083e7173e5888686a0a0cf09f52aaaa"
        );
    }

    #[test]
    fn a_vault_from_before_the_cost_went_up_still_opens() {
        let earlier = KdfParams {
            m_kib: 19 * 1024,
            t: 2,
            p: 1,
        };
        let key = derive_key("correct horse battery", &[7; SALT_LEN], earlier).unwrap();
        assert_eq!(
            hex(key.as_ref()),
            "58b44651ad77fc1dcc5f39c5b1edf71487783bef836982faba561430486946a1"
        );
        let file = SealKey::with_params("pw", earlier)
            .unwrap()
            .seal(b"hello")
            .unwrap();
        assert_eq!(open("pw", &file).unwrap().plaintext, b"hello");
    }

    #[test]
    fn what_is_below_the_current_cost_is_told_by_memory_or_time() {
        let at = |m_kib, t| KdfParams { m_kib, t, p: 1 };
        let current = KdfParams::CURRENT;
        assert!(!current.below_current());
        assert!(at(current.m_kib - 1, current.t).below_current());
        assert!(at(current.m_kib, current.t - 1).below_current());
        assert!(!at(current.m_kib * 2, current.t + 1).below_current());
        assert!(!KdfParams::STRONG.below_current());
    }

    #[test]
    fn a_file_that_asks_for_too_much_memory_is_refused_before_any_is_taken() {
        let file = small(b"hello");
        let mut value = json(&file);
        value["kdf"]["memoryKib"] = (KdfParams::MOST_KIB + 1).into();
        let changed = serde_json::to_vec(&value).unwrap();
        assert!(matches!(open("pw", &changed), Err(VaultError::Crypto(_))));
        // The most it can ask for is more than a vault is sealed at today, so a
        // later build can raise it.
        const { assert!(KdfParams::STRONG.m_kib < KdfParams::MOST_KIB) }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn the_file_is_json_that_names_how_it_was_sealed() {
        let file = SealKey::new("pw").unwrap().seal(b"hello").unwrap();
        let value = json(&file);
        assert_eq!(value["format"], "airlock-vault");
        assert_eq!(value["version"], 1);
        assert_eq!(value["kdf"]["name"], "argon2id");
        assert_eq!(value["kdf"]["memoryKib"], KdfParams::CURRENT.m_kib);
        assert_eq!(value["kdf"]["iterations"], KdfParams::CURRENT.t);
        assert_eq!(value["kdf"]["parallelism"], 1);
        assert_eq!(value["cipher"]["name"], "xchacha20-poly1305");
        assert!(value["ciphertext"].is_string());
        assert_eq!(open("pw", &file).unwrap().plaintext, b"hello");
    }

    #[test]
    fn open_reads_kdf_parameters_from_the_header() {
        let file = small(b"hello");
        assert_eq!(json(&file)["kdf"]["memoryKib"], 8);
        assert_eq!(open("pw", &file).unwrap().plaintext, b"hello");
        assert!(matches!(open("nope", &file), Err(VaultError::UnlockFailed)));
    }

    #[test]
    fn a_changed_header_fails_like_a_wrong_password() {
        let file = small(b"hello");
        for (path, value) in [
            ("/kdf/iterations", serde_json::json!(2)),
            ("/kdf/memoryKib", serde_json::json!(16)),
            (
                "/kdf/salt",
                serde_json::json!(STANDARD.encode([9u8; SALT_LEN])),
            ),
        ] {
            let mut changed = json(&file);
            *changed.pointer_mut(path).unwrap() = value;
            let changed = serde_json::to_vec(&changed).unwrap();
            assert!(
                matches!(open("pw", &changed), Err(VaultError::UnlockFailed)),
                "{path}"
            );
        }
        // Spacing is not part of what is sealed.
        let compact = serde_json::to_vec(&json(&file)).unwrap();
        assert_ne!(compact, file);
        assert_eq!(open("pw", &compact).unwrap().plaintext, b"hello");
    }

    #[test]
    fn what_is_not_this_file_is_refused() {
        for file in [
            b"WLT1\x01".to_vec(),
            b"{}".to_vec(),
            serde_json::to_vec(&{
                let mut value = json(&small(b"x"));
                value["format"] = "other".into();
                value
            })
            .unwrap(),
            serde_json::to_vec(&{
                let mut value = json(&small(b"x"));
                value["extra"] = 1.into();
                value
            })
            .unwrap(),
        ] {
            assert!(matches!(
                open("pw", &file),
                Err(VaultError::UnsupportedVersion)
            ));
        }
    }

    /// `again`, and `hello` as what its backup holds, under one key.
    fn saved_twice() -> (Vec<u8>, Vec<u8>) {
        let key = SealKey::with_params("pw", SMALL).unwrap();
        (key.seal(b"again").unwrap(), key.seal(b"hello").unwrap())
    }

    /// `file` with the last byte of its ciphertext changed.
    fn flipped(file: &[u8]) -> Vec<u8> {
        let mut value = json(file);
        let mut ciphertext = STANDARD
            .decode(value["ciphertext"].as_str().unwrap())
            .unwrap();
        *ciphertext.last_mut().unwrap() ^= 1;
        value["ciphertext"] = STANDARD.encode(ciphertext).into();
        serde_json::to_vec(&value).unwrap()
    }

    #[test]
    fn a_whole_file_opens_as_itself_whatever_its_backup_holds() {
        let (file, backup) = saved_twice();
        let opened = open_or_backup("pw", &file, Some(&backup)).unwrap();
        assert_eq!(opened.plaintext, b"again");
        assert!(!opened.from_backup);
    }

    #[test]
    fn a_damaged_file_opens_from_its_backup_with_the_right_password_only() {
        let (file, backup) = saved_twice();
        for damaged in [flipped(&file), b"not a sealed file".to_vec(), Vec::new()] {
            let opened = open_or_backup("pw", &damaged, Some(&backup)).unwrap();
            assert_eq!(opened.plaintext, b"hello");
            assert!(opened.from_backup);
            assert!(matches!(
                open_or_backup("nope", &damaged, Some(&backup)),
                Err(VaultError::UnlockFailed)
            ));
            assert!(open_or_backup("pw", &damaged, None).is_err());
        }
    }

    #[test]
    fn a_backup_from_before_a_password_change_is_not_tried() {
        // Under one password here, and each with a salt of its own, which is
        // how a change leaves them.
        let before = small(b"old");
        let after = small(b"new");
        assert!(matches!(
            open_or_backup("pw", &flipped(&after), Some(&before)),
            Err(VaultError::UnlockFailed)
        ));
    }

    #[test]
    fn a_file_of_another_version_is_left_to_its_build() {
        let (file, backup) = saved_twice();
        let mut value = json(&file);
        value["version"] = 2.into();
        let newer = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            open_or_backup("pw", &newer, Some(&backup)),
            Err(VaultError::UnsupportedVersion)
        ));
    }

    #[test]
    fn the_key_kept_from_an_open_seals_what_the_password_opens() {
        let first = small(b"hello");
        let key = open("pw", &first).unwrap().key;
        let second = key.seal(b"again").unwrap();
        let third = key.seal(b"again").unwrap();
        // The same salt and cost, a new nonce each time.
        assert_eq!(json(&second)["kdf"], json(&first)["kdf"]);
        assert_ne!(json(&second)["cipher"], json(&third)["cipher"]);
        assert_eq!(open("pw", &second).unwrap().plaintext, b"again");
        assert_eq!(open("pw", &third).unwrap().plaintext, b"again");
    }
}
