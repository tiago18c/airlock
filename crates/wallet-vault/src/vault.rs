//! The vault: one encrypted document, held open in memory while unlocked.

use crate::seal::{open as unseal, SealKey};
use crate::store::VaultStore;
use crate::VaultError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// What a vault holds. Its shape has a number, sealed in with it.
///
/// A vault whose number is not this build's is refused whole, before any of
/// it is read as `Self`. Nothing reads two shapes: when the shape changes,
/// the number goes up and older vaults stop opening, with an error that says
/// that is why.
pub trait Document: Serialize + DeserializeOwned + Default {
    const FORMAT: u32;
}

/// What is sealed: the document and the number of its shape.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Sealing<'a, D> {
    format: u32,
    document: &'a D,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sealed<D> {
    document: D,
}

/// Only the number, read before anything else is.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Format {
    #[serde(default = "unnumbered")]
    format: u32,
}

/// Vaults written before shapes were numbered held the document bare.
fn unnumbered() -> u32 {
    1
}

/// An encrypted store of `D`. What `D` holds is the caller's business: this
/// type only seals it, opens it and keeps it in memory while unlocked.
///
/// The password is asked for to create and to unlock, and nowhere else. The
/// key derived from it is kept while unlocked, so a save neither needs the
/// password nor derives the key again.
pub struct Vault<S: VaultStore, D> {
    store: S,
    open: Option<Open<D>>,
}

/// What an unlocked vault holds: the document and the key to seal it with.
struct Open<D> {
    data: D,
    key: SealKey,
}

impl<S: VaultStore, D: Document> Vault<S, D> {
    pub fn new(store: S) -> Self {
        Self { store, open: None }
    }

    pub fn is_unlocked(&self) -> bool {
        self.open.is_some()
    }

    pub fn is_initialized(&self) -> Result<bool, VaultError> {
        Ok(self.store.load()?.is_some())
    }

    pub fn data(&self) -> Result<&D, VaultError> {
        self.open
            .as_ref()
            .map(|open| &open.data)
            .ok_or(VaultError::Locked)
    }

    pub fn data_mut(&mut self) -> Result<&mut D, VaultError> {
        self.open
            .as_mut()
            .map(|open| &mut open.data)
            .ok_or(VaultError::Locked)
    }

    /// Start an empty vault under `password` and save it. When the save
    /// fails the vault stays locked.
    pub fn create(&mut self, password: &str) -> Result<(), VaultError> {
        self.open = Some(Open {
            data: D::default(),
            key: SealKey::new(password)?,
        });
        let saved = self.save();
        if saved.is_err() {
            self.open = None;
        }
        saved
    }

    pub fn unlock(&mut self, password: &str) -> Result<(), VaultError> {
        let bytes = self.store.load()?.ok_or(VaultError::NotInitialized)?;
        let (key, mut json) = unseal(password, &bytes)?;
        let opened = open::<D>(&json);
        json.zeroize();
        self.open = Some(Open { data: opened?, key });
        Ok(())
    }

    /// Forget the document and the key.
    pub fn lock(&mut self) {
        self.open = None;
    }

    pub fn save(&mut self) -> Result<(), VaultError> {
        let open = self.open.as_ref().ok_or(VaultError::Locked)?;
        let sealing = Sealing {
            format: D::FORMAT,
            document: &open.data,
        };
        let mut json =
            serde_json::to_vec(&sealing).map_err(|e| VaultError::Serde(e.to_string()))?;
        let blob = open.key.seal(&json);
        json.zeroize();
        self.store.save(&blob?)
    }
}

/// Read a decrypted vault as `D`, or say that it is of another shape.
fn open<D: Document>(json: &[u8]) -> Result<D, VaultError> {
    let serde = |e: serde_json::Error| VaultError::Serde(e.to_string());
    let Format { format: found } = serde_json::from_slice(json).map_err(serde)?;
    if found != D::FORMAT {
        return Err(VaultError::OtherFormat {
            found,
            expected: D::FORMAT,
        });
    }
    let sealed: Sealed<D> = serde_json::from_slice(json).map_err(serde)?;
    Ok(sealed.document)
}

impl<S: VaultStore, D: Document + Clone> Vault<S, D> {
    /// Change the document and save it. When the save does not happen the
    /// document in memory is what it was before, so memory never holds what
    /// the store does not.
    pub fn change(&mut self, f: impl FnOnce(&mut D)) -> Result<(), VaultError> {
        let data = self.data_mut()?;
        let previous = data.clone();
        f(data);
        let saved = self.save();
        if saved.is_err() {
            *self.data_mut()? = previous;
        }
        saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;
    use serde::Deserialize;

    #[derive(Clone, Default, Serialize, Deserialize)]
    struct Notes {
        lines: Vec<String>,
    }

    impl Document for Notes {
        const FORMAT: u32 = 3;
    }

    /// The same notes in an earlier shape.
    #[derive(Clone, Default, Serialize, Deserialize)]
    struct OldNotes {
        text: String,
    }

    impl Document for OldNotes {
        const FORMAT: u32 = 2;
    }

    #[test]
    fn a_vault_of_another_shape_is_refused_whole_and_left_as_it_is() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut old: Vault<_, OldNotes> = Vault::new(store.clone());
        old.create("pw").unwrap();
        old.data_mut().unwrap().text = "Bob".into();
        old.save().unwrap();
        let sealed = store.load().unwrap().unwrap();

        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        let refused = vault.unlock("pw").unwrap_err();
        assert!(matches!(
            refused,
            VaultError::OtherFormat {
                found: 2,
                expected: 3
            }
        ));
        assert!(refused.to_string().contains("has not been changed"));
        // It stays locked, so nothing can save over the older vault.
        assert!(!vault.is_unlocked());
        assert!(matches!(vault.save(), Err(VaultError::Locked)));
        assert_eq!(store.load().unwrap().unwrap(), sealed);
        // A wrong password is still told apart from a wrong shape.
        assert!(matches!(vault.unlock("no"), Err(VaultError::UnlockFailed)));
        // The build that made it still opens it.
        old.lock();
        old.unlock("pw").unwrap();
        assert_eq!(old.data().unwrap().text, "Bob");
    }

    #[test]
    fn a_vault_from_before_shapes_were_numbered_counts_as_the_first() {
        // What the first builds sealed: the document, with nothing around it.
        let store = MemoryStore::new();
        let bare = serde_json::to_vec(&Notes {
            lines: vec!["Bob".into()],
        })
        .unwrap();
        store
            .save(&SealKey::new("pw").unwrap().seal(&bare).unwrap())
            .unwrap();
        let mut vault: Vault<_, Notes> = Vault::new(store);
        assert!(matches!(
            vault.unlock("pw"),
            Err(VaultError::OtherFormat {
                found: 1,
                expected: 3
            })
        ));
    }

    #[test]
    fn create_unlock_wrong_password() {
        let mut vault: Vault<_, Notes> = Vault::new(MemoryStore::new());
        vault.create("secret").unwrap();
        vault.lock();
        assert!(matches!(
            vault.unlock("nope"),
            Err(VaultError::UnlockFailed)
        ));
        vault.unlock("secret").unwrap();
        assert!(vault.data().unwrap().lines.is_empty());
    }

    #[test]
    fn what_was_saved_is_there_after_a_lock() {
        let mut vault: Vault<_, Notes> = Vault::new(MemoryStore::new());
        vault.create("pw").unwrap();
        vault.data_mut().unwrap().lines.push("Bob".into());
        vault.save().unwrap();
        vault.lock();
        assert!(vault.data().is_err());
        vault.unlock("pw").unwrap();
        assert_eq!(vault.data().unwrap().lines, vec!["Bob".to_string()]);
    }

    #[test]
    fn a_change_that_cannot_be_saved_is_not_kept_in_memory_either() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        assert!(matches!(
            vault.change(|notes| notes.lines.push("Early".into())),
            Err(VaultError::Locked)
        ));
        vault.create("pw").unwrap();
        vault
            .change(|notes| notes.lines.push("Bob".into()))
            .unwrap();

        store.refuse_saves(true);
        let refused = vault.change(|notes| notes.lines.push("Lost".into()));
        assert!(matches!(refused, Err(VaultError::Io(_))));
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);

        store.refuse_saves(false);
        vault.lock();
        vault.unlock("pw").unwrap();
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);
    }

    #[test]
    fn a_vault_that_could_not_be_created_stays_locked() {
        let store = MemoryStore::new();
        store.refuse_saves(true);
        let mut vault: Vault<_, Notes> = Vault::new(store);
        assert!(vault.create("pw").is_err());
        assert!(!vault.is_unlocked());
    }

    #[test]
    fn saves_after_an_unlock_open_with_the_same_password_and_a_lock_forgets_the_key() {
        let mut vault: Vault<_, Notes> = Vault::new(MemoryStore::new());
        vault.create("pw").unwrap();
        vault.lock();
        vault.unlock("pw").unwrap();
        vault
            .change(|notes| notes.lines.push("Bob".into()))
            .unwrap();
        vault
            .change(|notes| notes.lines.push("Ann".into()))
            .unwrap();
        vault.lock();
        // Locked, there is no key to seal with.
        assert!(matches!(
            vault.change(|notes| notes.lines.push("Lost".into())),
            Err(VaultError::Locked)
        ));
        assert!(matches!(
            vault.unlock("other"),
            Err(VaultError::UnlockFailed)
        ));
        vault.unlock("pw").unwrap();
        assert_eq!(vault.data().unwrap().lines, ["Bob", "Ann"]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn file_store_roundtrip() {
        use crate::store::FileStore;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.bin");
        let mut vault: Vault<_, Notes> = Vault::new(FileStore::new(&path));
        vault.create("disk").unwrap();
        vault.data_mut().unwrap().lines.push("Desk".into());
        vault.save().unwrap();
        drop(vault);
        let mut vault: Vault<_, Notes> = Vault::new(FileStore::new(&path));
        vault.unlock("disk").unwrap();
        assert_eq!(vault.data().unwrap().lines, vec!["Desk".to_string()]);
    }
}
