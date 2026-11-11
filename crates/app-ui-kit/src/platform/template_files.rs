//! Desktop file dialogs for importing and exporting a template. Each
//! function returns `None` when the dialog was cancelled.
//!
//! A debug build started with `AIRLOCK_DEBUG_FILE_DIR` set skips the dialog:
//! importing reads `import.toml` from that folder and exporting writes the
//! file there, so a script driving the app can do both. A release build
//! has no such way around the dialog.

use std::path::PathBuf;
use wallet_core::template::MAX_TEMPLATE_BYTES;

/// Why a template file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum TemplateFileError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("This file is {0} bytes. A template can be at most {MAX_TEMPLATE_BYTES}.")]
    TooLarge(u64),
    #[error("Template files can't be picked on this device.")]
    NoDialog,
}

/// The folder a debug build reads and writes template files in instead of
/// asking, when it is given one.
#[cfg(not(target_os = "android"))]
fn debug_folder() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        std::env::var_os("AIRLOCK_DEBUG_FILE_DIR").map(PathBuf::from)
    } else {
        None
    }
}

/// Ask for one template file and read it, refusing one larger than a
/// template can be before reading it.
#[cfg(not(target_os = "android"))]
pub fn pick_template_file() -> Option<Result<Vec<u8>, TemplateFileError>> {
    let path = match debug_folder() {
        Some(folder) => folder.join("import.toml"),
        None => rfd::FileDialog::new()
            .add_filter("Template", &["toml"])
            .pick_file()?,
    };
    Some(read_limited(&path))
}

/// Ask where to save `text` as `file_name`, and write it there. Returns the
/// file.
#[cfg(not(target_os = "android"))]
pub fn save_template_file(
    file_name: &str,
    text: &str,
) -> Option<Result<PathBuf, TemplateFileError>> {
    let path = match debug_folder() {
        Some(folder) => folder.join(file_name),
        None => rfd::FileDialog::new()
            .add_filter("Template", &["toml"])
            .set_file_name(file_name)
            .save_file()?,
    };
    Some(
        std::fs::write(&path, text)
            .map(|()| path)
            .map_err(TemplateFileError::from),
    )
}

// Android has no file picker for the app: each answers that, should a
// screen ask anyway (Build leaves its buttons out there).

#[cfg(target_os = "android")]
pub fn pick_template_file() -> Option<Result<Vec<u8>, TemplateFileError>> {
    Some(Err(TemplateFileError::NoDialog))
}

#[cfg(target_os = "android")]
pub fn save_template_file(
    _file_name: &str,
    _text: &str,
) -> Option<Result<PathBuf, TemplateFileError>> {
    Some(Err(TemplateFileError::NoDialog))
}

/// The file at `path`, when it is no larger than a template can be.
pub fn read_limited(path: &std::path::Path) -> Result<Vec<u8>, TemplateFileError> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_TEMPLATE_BYTES as u64 {
        return Err(TemplateFileError::TooLarge(size));
    }
    Ok(std::fs::read(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_larger_than_a_template_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.toml");
        std::fs::write(&big, vec![b'#'; MAX_TEMPLATE_BYTES + 1]).unwrap();
        assert!(matches!(
            read_limited(&big),
            Err(TemplateFileError::TooLarge(_))
        ));
        let small = dir.path().join("small.toml");
        std::fs::write(&small, "format = 1").unwrap();
        assert_eq!(read_limited(&small).unwrap(), b"format = 1");
    }
}
