//! Token logos, one file per mint in the data folder's `logos` folder:
//! `logos/<mint>.<png|jpg|webp>`. The prices job writes each once; the
//! Portfolio reads each at most once per run of the app. The browser build
//! keeps no logos.

use base64::Engine;
use solana_pubkey::Pubkey;
use wallet_core::portfolio::LogoKind;
use wallet_core::VaultError;

use super::Storage;

/// The folder in the data folder that holds the logos.
pub const LOGOS_DIR: &str = "logos";

/// Where `mint`'s logo is kept. Only the mint, which is base58, goes into the
/// name, so nothing the service sent can choose where a file is written.
pub fn logo_file(mint: &Pubkey, kind: LogoKind) -> String {
    format!("{LOGOS_DIR}/{mint}.{}", kind.extension())
}

/// Keep `mint`'s logo. The file is written under a temporary name and then
/// renamed, so a crash leaves no half file.
pub fn save_logo(
    storage: &dyn Storage,
    mint: &Pubkey,
    kind: LogoKind,
    bytes: &[u8],
) -> Result<(), VaultError> {
    storage.open(&logo_file(mint, kind)).save(bytes)
}

/// `mint`'s logo, when its file is there and can be read.
pub fn load_logo(storage: &dyn Storage, mint: &Pubkey, kind: LogoKind) -> Option<Vec<u8>> {
    storage.open(&logo_file(mint, kind)).load().ok().flatten()
}

/// A `data:` URL of a logo, for the page, which never loads a remote image.
pub fn logo_data_url(kind: LogoKind, bytes: &[u8]) -> String {
    format!(
        "data:{};base64,{}",
        kind.media_type(),
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::FileStorage;

    #[test]
    fn a_logo_is_kept_in_the_logos_folder_by_mint() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path());
        let mint = Pubkey::new_from_array([7; 32]);
        assert_eq!(load_logo(&storage, &mint, LogoKind::Webp), None);
        save_logo(&storage, &mint, LogoKind::Webp, b"RIFF....WEBP").unwrap();
        let file = dir.path().join("logos").join(format!("{mint}.webp"));
        assert_eq!(std::fs::read(&file).unwrap(), b"RIFF....WEBP");
        assert_eq!(
            load_logo(&storage, &mint, LogoKind::Webp).as_deref(),
            Some(&b"RIFF....WEBP"[..])
        );
        // Kept as WebP, it is no JPEG.
        assert_eq!(load_logo(&storage, &mint, LogoKind::Jpeg), None);
        // Nothing but the file is left in the folder.
        let names: Vec<_> = std::fs::read_dir(dir.path().join("logos"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from(format!("{mint}.webp"))]);
        // A file the user deleted reads as none.
        std::fs::remove_file(&file).unwrap();
        assert_eq!(load_logo(&storage, &mint, LogoKind::Webp), None);
    }

    #[test]
    fn a_logo_goes_to_the_page_as_a_data_url() {
        assert_eq!(
            logo_data_url(LogoKind::Png, &[0x89, b'P', b'N', b'G']),
            "data:image/png;base64,iVBORw=="
        );
        assert!(logo_data_url(LogoKind::Jpeg, b"x").starts_with("data:image/jpeg;base64,"));
        assert_eq!(
            logo_file(&Pubkey::new_from_array([0; 32]), LogoKind::Jpeg),
            "logos/11111111111111111111111111111111.jpg"
        );
    }
}
