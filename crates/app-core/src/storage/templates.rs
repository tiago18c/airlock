//! The user's own templates, kept as the text of their files, one each:
//! `templates/<slug>.toml` in the data folder on desktop and Android, the
//! key `airlock.template.<slug>` in a browser. The slug comes from the
//! template's name (`wallet_core::template::template_slug`), so nothing a
//! file says can choose where it is written. What a file holds is read here
//! as text; whether it is a template is the caller's to say.

use wallet_core::template::{is_template_slug, MAX_TEMPLATE_BYTES};
use wallet_core::VaultError;

use super::Storage;

/// The most templates read. Any more are left where they are.
pub const MAX_STORED_TEMPLATES: usize = 64;

/// What every template's name in the store starts with.
#[cfg(not(target_arch = "wasm32"))]
pub const TEMPLATES_PREFIX: &str = "templates/";
#[cfg(target_arch = "wasm32")]
pub const TEMPLATES_PREFIX: &str = "template.";

#[cfg(not(target_arch = "wasm32"))]
const SUFFIX: &str = ".toml";
#[cfg(target_arch = "wasm32")]
const SUFFIX: &str = "";

/// The name the template `slug` is kept under.
pub fn template_file(slug: &str) -> String {
    format!("{TEMPLATES_PREFIX}{slug}{SUFFIX}")
}

/// The slug of a name in the store, when it is one a template is kept
/// under: not the older copy a save keeps beside it, nor anything else.
fn slug_of(name: &str) -> Option<&str> {
    let slug = name.strip_prefix(TEMPLATES_PREFIX)?.strip_suffix(SUFFIX)?;
    is_template_slug(slug).then_some(slug)
}

/// One template as kept: its slug, and its text, or why it could not be
/// read as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub slug: String,
    pub text: Result<String, String>,
}

/// What [`stored_templates`] read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredTemplates {
    /// By slug.
    pub kept: Vec<Stored>,
    /// How many more there are than [`MAX_STORED_TEMPLATES`].
    pub left_out: usize,
}

/// Every template kept in `storage`, by slug, at most
/// [`MAX_STORED_TEMPLATES`]. A file larger than a template can be, or that
/// is not text, is listed with the reason; it does not stop the others.
pub fn stored_templates(storage: &dyn Storage) -> Result<StoredTemplates, VaultError> {
    let mut slugs: Vec<String> = storage
        .list(TEMPLATES_PREFIX)?
        .iter()
        .filter_map(|name| slug_of(name).map(str::to_string))
        .collect();
    slugs.sort();
    slugs.dedup();
    let left_out = slugs.len().saturating_sub(MAX_STORED_TEMPLATES);
    slugs.truncate(MAX_STORED_TEMPLATES);
    let mut kept = Vec::new();
    for slug in slugs {
        let text = match storage.open(&template_file(&slug)).load() {
            Ok(None) => continue,
            Ok(Some(bytes)) if bytes.len() > MAX_TEMPLATE_BYTES => Err(format!(
                "This file is {} bytes. A template can be at most {MAX_TEMPLATE_BYTES}.",
                bytes.len()
            )),
            Ok(Some(bytes)) => String::from_utf8(bytes)
                .map_err(|_| "This file is not text, so it is not a template.".to_string()),
            Err(err) => Err(format!("This file could not be read: {err}")),
        };
        kept.push(Stored { slug, text });
    }
    Ok(StoredTemplates { kept, left_out })
}

/// Keeps `text` as the template `slug`, in place of one kept under it.
pub fn save_template(storage: &dyn Storage, slug: &str, text: &str) -> Result<(), VaultError> {
    if !is_template_slug(slug) {
        return Err(VaultError::Io(format!("`{slug}` is not a template's name")));
    }
    storage.open(&template_file(slug)).save(text.as_bytes())
}

/// Removes the template `slug`.
pub fn remove_template(storage: &dyn Storage, slug: &str) -> Result<(), VaultError> {
    if !is_template_slug(slug) {
        return Err(VaultError::Io(format!("`{slug}` is not a template's name")));
    }
    storage.remove(&template_file(slug))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::FileStorage;
    use crate::testing::MemoryStorage;

    fn kept(storage: &dyn Storage) -> Vec<(String, Result<String, String>)> {
        stored_templates(storage)
            .unwrap()
            .kept
            .into_iter()
            .map(|stored| (stored.slug, stored.text))
            .collect()
    }

    fn saves_lists_replaces_and_removes(storage: &dyn Storage) {
        assert!(kept(storage).is_empty(), "nothing is kept yet");
        save_template(storage, "pay-rent", "one").unwrap();
        save_template(storage, "airdrop", "two").unwrap();
        save_template(storage, "pay-rent", "three").unwrap();
        assert_eq!(
            kept(storage),
            [
                ("airdrop".to_string(), Ok("two".to_string())),
                ("pay-rent".to_string(), Ok("three".to_string())),
            ],
            "by slug, the older copy of a replaced one not listed"
        );
        remove_template(storage, "pay-rent").unwrap();
        remove_template(storage, "pay-rent").unwrap();
        assert_eq!(
            kept(storage),
            [("airdrop".to_string(), Ok("two".to_string()))],
            "removed with the copy kept beside it"
        );
        assert!(save_template(storage, "../vault", "x").is_err());
        assert!(remove_template(storage, "create-token").is_err());
    }

    #[test]
    fn templates_are_files_in_the_templates_folder() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path());
        saves_lists_replaces_and_removes(&storage);
        let file = dir.path().join("templates").join("airdrop.toml");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &std::path::Path| {
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777
            };
            assert_eq!(mode(&file), 0o600);
            assert_eq!(mode(&dir.path().join("templates")), 0o700);
        }
    }

    #[test]
    fn templates_in_memory_follow_the_same_rules() {
        saves_lists_replaces_and_removes(&MemoryStorage::default());
    }

    #[test]
    fn a_file_that_is_no_template_is_listed_with_why_and_the_rest_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path());
        let folder = dir.path().join("templates");
        std::fs::create_dir_all(folder.join("nested.toml")).unwrap();
        std::fs::write(folder.join("big.toml"), vec![b'#'; MAX_TEMPLATE_BYTES + 1]).unwrap();
        std::fs::write(folder.join("binary.toml"), [0xff, 0xfe, 0x00]).unwrap();
        std::fs::write(folder.join("good.toml"), "format = 1").unwrap();
        std::fs::write(folder.join("Not A Slug.toml"), "x").unwrap();
        std::fs::write(folder.join("notes.txt"), "x").unwrap();
        std::fs::write(folder.join("good.toml.tmp"), "x").unwrap();
        let read = kept(&storage);
        let slugs: Vec<&str> = read.iter().map(|(slug, _)| slug.as_str()).collect();
        assert_eq!(slugs, ["big", "binary", "good"]);
        assert!(read[0].1.as_ref().unwrap_err().contains("at most"));
        assert!(read[1].1.as_ref().unwrap_err().contains("not text"));
        assert_eq!(read[2].1, Ok("format = 1".to_string()));
    }

    #[test]
    fn at_most_so_many_are_read() {
        let storage = MemoryStorage::default();
        for n in 0..MAX_STORED_TEMPLATES + 3 {
            storage.put(&template_file(&format!("t{n:03}")), b"x");
        }
        let read = stored_templates(&storage).unwrap();
        assert_eq!(read.kept.len(), MAX_STORED_TEMPLATES);
        assert_eq!(read.left_out, 3);
        assert_eq!(read.kept[0].slug, "t000");
    }

    #[test]
    fn a_store_that_cannot_be_read_says_so() {
        let storage = MemoryStorage::default();
        storage.refuse_reads(true);
        assert!(stored_templates(&storage).is_err());
    }
}
