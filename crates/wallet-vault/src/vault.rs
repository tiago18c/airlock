//! The vault: one encrypted document, held open in memory while unlocked.

use crate::seal::{open as unseal, open_or_backup, open_with_key, SealKey};
use crate::store::VaultStore;
use crate::VaultError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
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

/// What opening a vault needs from its store, read without the password.
///
/// Deriving the key takes about a second, so it is a step of its own: whoever
/// shares the vault can go on using it meanwhile, and a window can go on drawing.
pub struct Unlocking<D> {
    file: Vec<u8>,
    backup: Option<Vec<u8>>,
    document: PhantomData<fn() -> D>,
}

impl<D: Document> Unlocking<D> {
    /// Derive the key and open what was read. A vault sealed at less than the
    /// current cost gets a second derivation, only once the password has opened it.
    pub fn open(self, password: &str) -> Result<Unlocked<D>, VaultError> {
        let mut unsealed = open_or_backup(password, &self.file, self.backup.as_deref())?;
        let opened = open::<D>(&unsealed.plaintext);
        unsealed.plaintext.zeroize();
        let data = opened?;
        // One that cannot be made now is tried at the next unlock.
        let stronger = unsealed
            .key
            .below_current()
            .then(|| SealKey::new(password).ok())
            .flatten();
        Ok(Unlocked {
            data,
            key: unsealed.key,
            stronger,
            damaged: unsealed.from_backup.then_some(self.file),
        })
    }
}

impl<D: Document> Unlocking<D> {
    /// Open what was read with the key of an earlier unlock, as
    /// [`Vault::seal_key_bytes`] gave it and a platform's secure hardware kept
    /// it. No key is derived, so it takes no time. A damaged file is not
    /// opened from its backup, and a vault below the current cost is not
    /// raised: both wait for the password.
    pub fn open_with_key(self, key: &SealKey) -> Result<Unlocked<D>, VaultError> {
        let mut unsealed = open_with_key(key, &self.file)?;
        let opened = open::<D>(&unsealed.plaintext);
        unsealed.plaintext.zeroize();
        Ok(Unlocked {
            data: opened?,
            key: unsealed.key,
            stronger: None,
            damaged: None,
        })
    }
}

/// A vault opened by [`Unlocking::open`], for [`Vault::finish_unlock`].
pub struct Unlocked<D> {
    data: D,
    key: SealKey,
    /// The key to seal it with from now on, when `key` is below the current cost.
    stronger: Option<SealKey>,
    /// What the file held, when it was damaged and its backup opened instead.
    damaged: Option<Vec<u8>>,
}

/// What changing a password needs from the store, read without either password.
pub struct PasswordChange {
    file: Vec<u8>,
}

impl PasswordChange {
    /// Check that `current` opens what is stored, and derive the key for `new`.
    /// Takes twice what an unlock does.
    pub fn derive(self, current: &str, new: &str) -> Result<SealKey, VaultError> {
        let mut unsealed = unseal(current, &self.file)?;
        unsealed.plaintext.zeroize();
        SealKey::new(new)
    }
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

    /// The key the open vault is sealed under, as bytes, for a platform to
    /// keep behind the user's fingerprint ([`SealKey::to_bytes`]). Locked, there
    /// is none.
    pub fn seal_key_bytes(&self) -> Result<zeroize::Zeroizing<Vec<u8>>, VaultError> {
        self.open
            .as_ref()
            .map(|open| open.key.to_bytes())
            .ok_or(VaultError::Locked)
    }

    /// Start an empty vault under `password` and save it. When the save
    /// fails the vault stays locked. Only over nothing: a store that holds a
    /// vault, or that cannot be read, is refused and left as it is.
    pub fn create(&mut self, password: &str) -> Result<(), VaultError> {
        if self.read()?.is_some() {
            return Err(VaultError::AlreadyExists);
        }
        self.create_under(SealKey::new(password)?)
    }

    /// [`create`](Self::create), under a key already derived.
    pub fn create_under(&mut self, key: SealKey) -> Result<(), VaultError> {
        if self.read()?.is_some() {
            return Err(VaultError::AlreadyExists);
        }
        self.open = Some(Open {
            data: D::default(),
            key,
        });
        let saved = self.save();
        if saved.is_err() {
            self.open = None;
        }
        saved
    }

    /// Open the vault: [`begin_unlock`](Self::begin_unlock),
    /// [`Unlocking::open`] and [`finish_unlock`](Self::finish_unlock) in turn.
    pub fn unlock(&mut self, password: &str) -> Result<(), VaultError> {
        let opened = self.begin_unlock()?.open(password)?;
        self.finish_unlock(opened);
        Ok(())
    }

    /// Read what [`Unlocking::open`] needs.
    pub fn begin_unlock(&mut self) -> Result<Unlocking<D>, VaultError> {
        let file = self.read()?.ok_or(VaultError::NotInitialized)?;
        let backup = self.store.load_backup().ok().flatten();
        Ok(Unlocking {
            file,
            backup,
            document: PhantomData,
        })
    }

    /// Take in what [`Unlocking::open`] opened, and answer whether it was. It is
    /// not when the vault was unlocked since, and what is open may have changed.
    ///
    /// A file that turned out damaged was opened from the backup the store keeps
    /// of the save before: the damaged bytes are set aside, the file is written
    /// whole again, and [`take_recovered`](Self::take_recovered) says so. A vault
    /// below the current cost is raised to it.
    pub fn finish_unlock(&mut self, unlocked: Unlocked<D>) -> bool {
        if self.open.is_some() {
            return false;
        }
        let Unlocked {
            data,
            key,
            stronger,
            damaged,
        } = unlocked;
        self.open = Some(Open { data, key });
        if let Some(bytes) = damaged {
            self.recovered = true;
            // What is open is right, and the next save tries again.
            let _ = self.store.set_aside(&bytes);
            let _ = self.save();
        }
        if let Some(stronger) = stronger {
            self.raise_cost(stronger);
        }
        true
    }

    /// Seal under `stronger` from now on. The save leaves what was stored as the
    /// store's backup, at the old cost and just as good to anyone guessing the
    /// password, so a second save replaces it. When a save fails the old key stays,
    /// and the next unlock tries again.
    fn raise_cost(&mut self, stronger: SealKey) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let older = std::mem::replace(&mut open.key, stronger);
        if self.save().is_err() {
            if let Some(open) = self.open.as_mut() {
                open.key = older;
            }
            return;
        }
        let _ = self.save();
    }

    /// Seal the vault under `new` from now on. `current` has to open what is
    /// stored, so an unlocked window left alone is not enough to change it.
    /// [`begin_password_change`](Self::begin_password_change),
    /// [`PasswordChange::derive`] and
    /// [`finish_password_change`](Self::finish_password_change) in turn.
    pub fn change_password(&mut self, current: &str, new: &str) -> Result<(), VaultError> {
        let key = self.begin_password_change()?.derive(current, new)?;
        self.finish_password_change(key)
    }

    /// Read what [`PasswordChange::derive`] needs.
    pub fn begin_password_change(&self) -> Result<PasswordChange, VaultError> {
        if self.open.is_none() {
            return Err(VaultError::Locked);
        }
        let file = self.store.load()?.ok_or(VaultError::NotInitialized)?;
        Ok(PasswordChange { file })
    }

    /// Seal under what [`PasswordChange::derive`] made, which has its own salt.
    /// Once the vault is saved under it, the backup the store kept of the save
    /// before, still sealed under the old password, is deleted. When the save fails
    /// nothing changes.
    pub fn finish_password_change(&mut self, key: SealKey) -> Result<(), VaultError> {
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

    fn cost_of(file: &[u8]) -> (u64, u64) {
        let value: serde_json::Value = serde_json::from_slice(file).unwrap();
        (
            value["kdf"]["memoryKib"].as_u64().unwrap(),
            value["kdf"]["iterations"].as_u64().unwrap(),
        )
    }

    /// A vault sealed at less than today's cost, with one save behind it so
    /// that its backup is at that cost too. Locked, as a new run finds it.
    fn from_an_earlier_build(
        store: &std::sync::Arc<MemoryStore>,
    ) -> Vault<std::sync::Arc<MemoryStore>, Notes> {
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        vault
            .create_under(SealKey::from_earlier_build("pw"))
            .unwrap();
        vault.data_mut().unwrap().lines.push("Bob".into());
        vault.save().unwrap();
        vault.lock();
        vault
    }

    #[test]
    fn a_vault_sealed_at_less_is_raised_on_unlock_and_its_backup_with_it() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault = from_an_earlier_build(&store);
        let before = cost_of(&store.load().unwrap().unwrap());
        assert!(before < cost_of(&SealKey::new("pw").unwrap().seal(b"x").unwrap()));
        assert_eq!(cost_of(&store.load_backup().unwrap().unwrap()), before);

        vault.unlock("pw").unwrap();

        let raised = cost_of(&SealKey::new("pw").unwrap().seal(b"x").unwrap());
        assert_eq!(cost_of(&store.load().unwrap().unwrap()), raised);
        assert_eq!(cost_of(&store.load_backup().unwrap().unwrap()), raised);
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);
        assert!(!vault.take_recovered());

        // The password is what it was, and nothing is raised a second time.
        vault.lock();
        let file = store.load().unwrap();
        vault.unlock("pw").unwrap();
        assert_eq!(store.load().unwrap(), file);
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);
    }

    #[test]
    fn a_wrong_password_raises_nothing() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault = from_an_earlier_build(&store);
        let file = store.load().unwrap();
        assert!(matches!(
            vault.unlock("nope"),
            Err(VaultError::UnlockFailed)
        ));
        assert_eq!(store.load().unwrap(), file);
        assert!(!vault.is_unlocked());
    }

    #[test]
    fn a_vault_that_cannot_be_raised_opens_anyway_and_is_raised_at_the_next_unlock() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault = from_an_earlier_build(&store);
        let file = store.load().unwrap();

        store.refuse_saves(true);
        vault.unlock("pw").unwrap();
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);
        assert_eq!(store.load().unwrap(), file);

        // Saves go on under the key the file was opened with.
        store.refuse_saves(false);
        vault.save().unwrap();
        let older = cost_of(file.as_deref().unwrap());
        assert_eq!(cost_of(&store.load().unwrap().unwrap()), older);

        vault.lock();
        vault.unlock("pw").unwrap();
        assert!(cost_of(&store.load().unwrap().unwrap()) > older);
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);
    }

    #[test]
    fn a_vault_sealed_at_more_is_left_there() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        vault.create_under(SealKey::from_later_build("pw")).unwrap();
        vault.lock();
        let file = store.load().unwrap();
        vault.unlock("pw").unwrap();
        assert_eq!(store.load().unwrap(), file);
    }

    #[test]
    fn a_damaged_file_at_less_is_recovered_and_raised() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault = from_an_earlier_build(&store);
        store.damage(b"not a sealed file");
        vault.unlock("pw").unwrap();
        assert!(vault.take_recovered());
        assert_eq!(store.set_aside_bytes().unwrap(), b"not a sealed file");
        let raised = cost_of(&SealKey::new("pw").unwrap().seal(b"x").unwrap());
        assert_eq!(cost_of(&store.load().unwrap().unwrap()), raised);
        assert_eq!(cost_of(&store.load_backup().unwrap().unwrap()), raised);
    }

    #[test]
    fn an_unlock_that_arrives_late_does_not_replace_what_is_open() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        vault.create("pw").unwrap();
        vault.lock();

        let first = vault.begin_unlock().unwrap().open("pw").unwrap();
        let second = vault.begin_unlock().unwrap().open("pw").unwrap();
        assert!(vault.finish_unlock(first));
        vault.data_mut().unwrap().lines.push("Bob".into());
        assert!(!vault.finish_unlock(second));
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);
    }

    #[test]
    fn the_steps_of_an_unlock_hold_nothing_of_the_vault() {
        // They own what they read, so the compiler lets the vault go on being
        // used between them. That is what keeps a window from waiting.
        fn sends<T: Send + 'static>(_: &T) {}
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store);
        vault.create("pw").unwrap();
        let change = vault.begin_password_change().unwrap();
        sends(&change);
        vault.data_mut().unwrap().lines.push("Bob".into());
        let key = change.derive("pw", "new").unwrap();
        sends(&key);
        vault.finish_password_change(key).unwrap();

        vault.lock();
        let reading = vault.begin_unlock().unwrap();
        sends(&reading);
        assert!(!vault.is_unlocked());
        let opened = reading.open("new").unwrap();
        sends(&opened);
        assert!(vault.finish_unlock(opened));
        assert_eq!(vault.data().unwrap().lines, ["Bob"]);
    }

    #[test]
    fn a_vault_stored_or_locked_since_the_key_was_derived_is_left_alone() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        let late = SealKey::new("pw").unwrap();
        vault.create("pw").unwrap();
        assert!(matches!(
            vault.create_under(late),
            Err(VaultError::AlreadyExists)
        ));
        let key = vault
            .begin_password_change()
            .unwrap()
            .derive("pw", "new")
            .unwrap();
        vault.lock();
        assert!(matches!(
            vault.finish_password_change(key),
            Err(VaultError::Locked)
        ));
        vault.unlock("pw").unwrap();
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

    /// What a platform keeps behind a fingerprint opens the vault, without
    /// the password and with no derivation, while the vault is sealed under it.
    #[test]
    fn the_key_kept_from_an_unlock_opens_the_vault_until_its_password_changes() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        assert!(matches!(vault.seal_key_bytes(), Err(VaultError::Locked)));
        vault.create("pw").unwrap();
        vault.data_mut().unwrap().lines.push("Kept".into());
        vault.save().unwrap();
        let kept = vault.seal_key_bytes().unwrap();
        vault.lock();
        assert!(matches!(vault.seal_key_bytes(), Err(VaultError::Locked)));

        let key = SealKey::from_bytes(&kept).unwrap();
        let opened = vault.begin_unlock().unwrap().open_with_key(&key).unwrap();
        assert!(vault.finish_unlock(opened));
        assert_eq!(vault.data().unwrap().lines, ["Kept"]);

        // A save under the same key leaves it opening the vault.
        vault.data_mut().unwrap().lines.push("Again".into());
        vault.save().unwrap();
        vault.lock();
        let key = SealKey::from_bytes(&kept).unwrap();
        assert!(vault.begin_unlock().unwrap().open_with_key(&key).is_ok());

        // A new password seals under a new salt: the kept key opens nothing.
        vault.unlock("pw").unwrap();
        vault.change_password("pw", "new pw").unwrap();
        vault.lock();
        let key = SealKey::from_bytes(&kept).unwrap();
        assert!(matches!(
            vault.begin_unlock().unwrap().open_with_key(&key),
            Err(VaultError::UnlockFailed)
        ));
        vault.unlock("new pw").unwrap();
    }

    #[test]
    fn kept_key_bytes_that_were_changed_open_nothing() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut vault: Vault<_, Notes> = Vault::new(store.clone());
        vault.create("pw").unwrap();
        let kept = vault.seal_key_bytes().unwrap();
        vault.lock();

        let mut flipped = kept.to_vec();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        let key = SealKey::from_bytes(&flipped).unwrap();
        assert!(matches!(
            vault.begin_unlock().unwrap().open_with_key(&key),
            Err(VaultError::UnlockFailed)
        ));
        assert!(SealKey::from_bytes(&kept[..kept.len() - 1]).is_err());
        let mut retagged = kept.to_vec();
        retagged[0] = b'X';
        assert!(SealKey::from_bytes(&retagged).is_err());
        assert!(!vault.is_unlocked());
    }
}
