//! The vault: one encrypted document, held open in memory while unlocked.

use crate::seal::{open as unseal, open_or_backup, SealKey};
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
    presence: Presence,
    /// The last unlock found the file damaged and opened its backup.
    recovered: bool,
}

/// What the store held when it was last read. Kept, so that asking takes no
/// read: a screen asks on every render.
enum Presence {
    Absent,
    Present,
    /// Reading it failed, which says nothing of what it holds.
    Unreadable(String),
}

impl Presence {
    fn of(read: &Result<Option<Vec<u8>>, VaultError>) -> Self {
        match read {
            Ok(Some(_)) => Self::Present,
            Ok(None) => Self::Absent,
            Err(err) => Self::Unreadable(err.to_string()),
        }
    }
}

/// What an unlocked vault holds: the document and the key to seal it with.
struct Open<D> {
    data: D,
    key: SealKey,
}

impl<S: VaultStore, D: Document> Vault<S, D> {
    pub fn new(store: S) -> Self {
        let presence = Presence::of(&store.load());
        Self {
            store,
            open: None,
            presence,
            recovered: false,
        }
    }

    pub fn is_unlocked(&self) -> bool {
        self.open.is_some()
    }

    /// Whether a vault is stored, as of the last read of the store. An error
    /// when that read failed: not knowing is not the same as there being none.
    pub fn is_initialized(&self) -> Result<bool, VaultError> {
        match &self.presence {
            Presence::Absent => Ok(false),
            Presence::Present => Ok(true),
            Presence::Unreadable(why) => Err(VaultError::Io(why.clone())),
        }
    }

    /// Why the store could not be read, as of the last time it was.
    pub fn read_problem(&self) -> Option<&str> {
        match &self.presence {
            Presence::Unreadable(why) => Some(why),
            Presence::Absent | Presence::Present => None,
        }
    }

    /// Whether the last unlock found the file damaged and opened the backup
    /// instead. Answers once.
    pub fn take_recovered(&mut self) -> bool {
        std::mem::take(&mut self.recovered)
    }

    /// Read the store, and note what it held.
    fn read(&mut self) -> Result<Option<Vec<u8>>, VaultError> {
        let read = self.store.load();
        self.presence = Presence::of(&read);
        read
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
    /// fails the vault stays locked. Only over nothing: a store that holds a
    /// vault, or that cannot be read, is refused and left as it is.
    pub fn create(&mut self, password: &str) -> Result<(), VaultError> {
        if self.read()?.is_some() {
            return Err(VaultError::AlreadyExists);
        }
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

    /// Open the vault. A file that turns out damaged is opened from the
    /// backup the store keeps of the save before: the damaged bytes are set
    /// aside, the file is written whole again, and
    /// [`take_recovered`](Self::take_recovered) says so.
    pub fn unlock(&mut self, password: &str) -> Result<(), VaultError> {
        let bytes = self.read()?.ok_or(VaultError::NotInitialized)?;
        let backup = self.store.load_backup().ok().flatten();
        let mut unsealed = open_or_backup(password, &bytes, backup.as_deref())?;
        let opened = open::<D>(&unsealed.plaintext);
        unsealed.plaintext.zeroize();
        self.open = Some(Open {
            data: opened?,
            key: unsealed.key,
        });
        if unsealed.from_backup {
            self.recovered = true;
            // What is open is right, and the next save tries again.
            let _ = self.store.set_aside(&bytes);
            let _ = self.save();
        }
        Ok(())
    }

    /// Seal the vault under `new` from now on. `current` has to open what is
    /// stored, so an unlocked window left alone is not enough to change it.
    /// The new key has its own salt. Once the vault is saved under it, the
    /// backup the store kept of the save before, still sealed under
    /// `current`, is deleted. When the save fails nothing changes.
    pub fn change_password(&mut self, current: &str, new: &str) -> Result<(), VaultError> {
        if self.open.is_none() {
            return Err(VaultError::Locked);
        }
        let bytes = self.store.load()?.ok_or(VaultError::NotInitialized)?;
        let mut unsealed = unseal(current, &bytes)?;
        unsealed.plaintext.zeroize();
        let key = SealKey::new(new)?;
        let open = self.open.as_mut().ok_or(VaultError::Locked)?;
        let old = std::mem::replace(&mut open.key, key);
        if let Err(err) = self.save() {
            if let Some(open) = self.open.as_mut() {
                open.key = old;
            }
            return Err(err);
        }
        self.store.discard_backup().map_err(|err| {
            VaultError::Io(format!(
                "The password was changed, but the copy kept from before could not be deleted, and the old password still opens it ({err})."
            ))
        })
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
        self.store.save(&blob?)?;
        self.presence = Presence::Present;
        Ok(())
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

    fn stored_vault() -> std::sync::Arc<MemoryStore> {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        vault.create("pw").unwrap();
        vault
            .change(|notes| notes.lines.push("Bob".into()))
            .unwrap();
        store
    }

    #[test]
    fn a_store_that_cannot_be_read_is_not_taken_for_an_empty_one() {
        let store = stored_vault();
        let sealed = store.load().unwrap().unwrap();
        store.refuse_reads(true);
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());

        assert!(vault.is_initialized().is_err());
        assert!(matches!(vault.create("new"), Err(VaultError::Io(_))));
        assert!(!vault.is_unlocked());
        assert!(matches!(vault.unlock("pw"), Err(VaultError::Io(_))));

        store.refuse_reads(false);
        assert_eq!(store.load().unwrap().unwrap(), sealed);
        vault.unlock("pw").unwrap();
        assert!(vault.is_initialized().unwrap());
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);
    }

    #[test]
    fn a_new_vault_is_never_made_over_one_that_is_stored() {
        let store = stored_vault();
        let sealed = store.load().unwrap().unwrap();
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        assert!(matches!(
            vault.create("new"),
            Err(VaultError::AlreadyExists)
        ));
        assert!(!vault.is_unlocked());
        assert_eq!(store.load().unwrap().unwrap(), sealed);
        vault.unlock("pw").unwrap();
    }

    #[test]
    fn asking_whether_a_vault_is_stored_does_not_read_the_store() {
        let store = stored_vault();
        let vault: Vault<_, Notes> = Vault::new(store.clone());
        store.refuse_reads(true);
        assert!(vault.is_initialized().unwrap());
    }

    #[test]
    fn a_damaged_file_opens_from_its_backup_and_is_written_whole_again() {
        let store = stored_vault();
        store.damage(b"not a sealed file");
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());

        // The wrong password finds nothing, and nothing changes.
        assert!(matches!(
            vault.unlock("nope"),
            Err(VaultError::UnlockFailed)
        ));
        assert_eq!(store.set_aside_bytes(), None);

        vault.unlock("pw").unwrap();
        assert!(vault.take_recovered());
        assert!(!vault.take_recovered());
        // The backup holds the save before the last. The empty vault it was
        // made from came before "Bob".
        assert!(vault.data().unwrap().lines.is_empty());
        assert_eq!(
            store.set_aside_bytes().as_deref(),
            Some(&b"not a sealed file"[..])
        );

        // Reading it again finds a whole file.
        vault.lock();
        vault.unlock("pw").unwrap();
        assert!(!vault.take_recovered());
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

    #[test]
    fn a_changed_password_opens_the_vault_and_the_old_one_does_not() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        vault.create("first").unwrap();
        vault
            .change(|notes| notes.lines.push("Bob".into()))
            .unwrap();
        // The current password has to be the right one.
        assert!(matches!(
            vault.change_password("wrong", "second"),
            Err(VaultError::UnlockFailed)
        ));
        vault.change_password("first", "second").unwrap();
        // Saves after it are sealed under the new key too.
        vault
            .change(|notes| notes.lines.push("Ann".into()))
            .unwrap();
        vault.lock();
        assert!(matches!(
            vault.unlock("first"),
            Err(VaultError::UnlockFailed)
        ));
        vault.unlock("second").unwrap();
        assert_eq!(vault.data().unwrap().lines, ["Bob", "Ann"]);
        vault.lock();
        assert!(matches!(
            vault.change_password("second", "third"),
            Err(VaultError::Locked)
        ));
    }

    #[test]
    fn a_password_change_that_cannot_be_saved_keeps_the_old_one() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        vault.create("first").unwrap();
        store.refuse_saves(true);
        assert!(vault.change_password("first", "second").is_err());
        store.refuse_saves(false);
        vault
            .change(|notes| notes.lines.push("Bob".into()))
            .unwrap();
        vault.lock();
        vault.unlock("first").unwrap();
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_password_change_deletes_the_backup_under_the_old_one() {
        use crate::store::FileStore;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.json");
        let mut vault: Vault<_, Notes> = Vault::new(FileStore::new(&path));
        vault.create("first").unwrap();
        vault
            .change(|notes| notes.lines.push("Bob".into()))
            .unwrap();
        let backup = dir.path().join("vault.json.bak");
        assert!(backup.exists());
        vault.change_password("first", "second").unwrap();
        assert!(!backup.exists());
        // Only the new password opens what is left on disk.
        let mut again: Vault<_, Notes> = Vault::new(FileStore::new(&path));
        assert!(matches!(
            again.unlock("first"),
            Err(VaultError::UnlockFailed)
        ));
        again.unlock("second").unwrap();
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
