//! Desktop file dialogs and folder I/O for importing and exporting IDLs.
//! Each function returns `None` when the dialog was cancelled.

use std::path::{Path, PathBuf};

/// File name and contents, as a bundle folder holds them.
pub type IdlFiles = Vec<(String, Vec<u8>)>;

#[derive(Debug, thiserror::Error)]
pub enum IdlFilesError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("That folder has no index.json.")]
    NoIndex,
}

/// Ask for one IDL file and read it.
pub fn pick_idl_file() -> Option<Result<Vec<u8>, IdlFilesError>> {
    let path = rfd::FileDialog::new()
        .add_filter("IDL", &["json"])
        .pick_file()?;
    Some(std::fs::read(path).map_err(IdlFilesError::from))
}

/// Ask for a bundle folder and read its JSON files.
pub fn pick_idl_bundle() -> Option<Result<IdlFiles, IdlFilesError>> {
    let dir = rfd::FileDialog::new().pick_folder()?;
    Some(read_json_dir(&dir))
}

/// Ask for a folder and write `files` into it. Returns the folder.
pub fn save_idl_bundle(files: &[(String, Vec<u8>)]) -> Option<Result<PathBuf, IdlFilesError>> {
    let dir = rfd::FileDialog::new().pick_folder()?;
    Some(write_files(&dir, files).map(|()| dir))
}

/// Every `.json` file directly inside `dir`. A bundle must include `index.json`.
pub fn read_json_dir(dir: &Path) -> Result<IdlFiles, IdlFilesError> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".json") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        files.push((name.to_string(), bytes));
    }
    if files
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("index.json"))
    {
        Ok(files)
    } else {
        Err(IdlFilesError::NoIndex)
    }
}

pub fn write_files(dir: &Path, files: &[(String, Vec<u8>)]) -> Result<(), IdlFilesError> {
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("airlock-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_bundle_round_trips_and_skips_what_is_not_json() {
        let dir = scratch("bundle");
        let files = vec![
            ("index.json".to_string(), b"{}".to_vec()),
            ("program.json".to_string(), b"[1]".to_vec()),
        ];
        write_files(&dir, &files).unwrap();
        std::fs::write(dir.join("notes.txt"), b"ignored").unwrap();
        std::fs::create_dir(dir.join("nested.json")).unwrap();

        let mut read = read_json_dir(&dir).unwrap();
        read.sort();
        assert_eq!(read, files);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_without_an_index_is_not_a_bundle() {
        let dir = scratch("no-index");
        write_files(&dir, &[("program.json".to_string(), b"[1]".to_vec())]).unwrap();
        assert!(matches!(
            read_json_dir(&dir).unwrap_err(),
            IdlFilesError::NoIndex
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
