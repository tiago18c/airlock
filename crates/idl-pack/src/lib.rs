//! The IDL pack: the IDLs of third-party programs, in one compressed file the
//! app ships beside its binary and reads once.
//!
//! A pack is zlib over a list of named files:
//!
//! ```text
//! magic     b"AIRLOCK-IDLS\x01"
//! count     u32 LE
//! entries   count times:
//!   name    u32 LE length, then UTF-8
//!   body    u32 LE length, then the bytes
//! ```
//!
//! The first entry is `manifest.json`, which says what each other file is.
//! The pack carries files as they are: whoever reads it checks the pack's
//! SHA-256 against the one its build pinned, and reads the manifest.
//!
//! With the `build` feature, [`pack_dir`] packs a source folder: its
//! `manifest.toml`, written as JSON, and every file the manifest lists.

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::{Read, Write};
use thiserror::Error;

const MAGIC: &[u8] = b"AIRLOCK-IDLS\x01";

/// The entry that describes the others.
pub const MANIFEST: &str = "manifest.json";

/// What a pack may inflate to. Far above what it holds, far below what
/// would hurt.
pub const MAX_UNPACKED: usize = 64 << 20;

const MAX_ENTRIES: usize = 4096;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PackError {
    #[error("the IDL pack is not zlib data")]
    NotCompressed,
    #[error("the IDL pack inflates past {} MiB", MAX_UNPACKED >> 20)]
    TooLarge,
    #[error("the IDL pack does not start with its magic bytes")]
    BadMagic,
    #[error("the IDL pack ends inside an entry")]
    Truncated,
    #[error("the IDL pack lists more than {MAX_ENTRIES} files")]
    TooManyEntries,
    #[error("an IDL pack entry name is not UTF-8")]
    BadName,
    #[error("the IDL pack has bytes after its last entry")]
    TrailingBytes,
}

/// Pack `entries` in order. The manifest should be first.
pub fn pack(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut raw = MAGIC.to_vec();
    raw.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, body) in entries {
        raw.extend_from_slice(&(name.len() as u32).to_le_bytes());
        raw.extend_from_slice(name.as_bytes());
        raw.extend_from_slice(&(body.len() as u32).to_le_bytes());
        raw.extend_from_slice(body);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&raw)
        .expect("writing to a Vec does not fail");
    encoder.finish().expect("writing to a Vec does not fail")
}

/// The named files of a pack, in the order they were packed.
pub fn unpack(packed: &[u8]) -> Result<Vec<(String, Vec<u8>)>, PackError> {
    let mut raw = Vec::new();
    ZlibDecoder::new(packed)
        .take(MAX_UNPACKED as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|_| PackError::NotCompressed)?;
    if raw.len() > MAX_UNPACKED {
        return Err(PackError::TooLarge);
    }
    let mut rest = raw.strip_prefix(MAGIC).ok_or(PackError::BadMagic)?;
    let count = take_u32(&mut rest)? as usize;
    if count > MAX_ENTRIES {
        return Err(PackError::TooManyEntries);
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name = take_bytes(&mut rest)?;
        let name = String::from_utf8(name.to_vec()).map_err(|_| PackError::BadName)?;
        let body = take_bytes(&mut rest)?.to_vec();
        entries.push((name, body));
    }
    if !rest.is_empty() {
        return Err(PackError::TrailingBytes);
    }
    Ok(entries)
}

fn take_u32(rest: &mut &[u8]) -> Result<u32, PackError> {
    let (head, tail) = rest.split_first_chunk::<4>().ok_or(PackError::Truncated)?;
    *rest = tail;
    Ok(u32::from_le_bytes(*head))
}

fn take_bytes<'a>(rest: &mut &'a [u8]) -> Result<&'a [u8], PackError> {
    let len = take_u32(rest)? as usize;
    if rest.len() < len {
        return Err(PackError::Truncated);
    }
    let (head, tail) = rest.split_at(len);
    *rest = tail;
    Ok(head)
}

/// A packed folder and its SHA-256, which the build pins.
#[cfg(feature = "build")]
pub struct Packed {
    pub bytes: Vec<u8>,
    pub sha256: [u8; 32],
}

/// Pack a source folder: `manifest.toml` as [`MANIFEST`], then each file its
/// `idls` entries name in `file`, in that order. A listed file that is
/// missing, or a JSON file in the folder the manifest does not list, is an
/// error, so a file is never shipped or left out by accident.
#[cfg(feature = "build")]
pub fn pack_dir(dir: &std::path::Path) -> Result<Packed, String> {
    use sha2::{Digest, Sha256};

    let manifest_path = dir.join("manifest.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|err| format!("{}: {err}", manifest_path.display()))?;
    let manifest: toml::Value =
        toml::from_str(&text).map_err(|err| format!("{}: {err}", manifest_path.display()))?;
    let manifest_json = serde_json::to_vec(&manifest).map_err(|err| err.to_string())?;
    let files: Vec<String> = manifest
        .get("idls")
        .and_then(toml::Value::as_array)
        .ok_or("manifest.toml has no [[idls]]")?
        .iter()
        .map(|entry| {
            entry
                .get("file")
                .and_then(toml::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "an [[idls]] entry has no file".to_string())
        })
        .collect::<Result<_, _>>()?;

    let mut listed = std::collections::BTreeSet::new();
    let mut bodies = Vec::with_capacity(files.len());
    for file in &files {
        if file.contains(['/', '\\']) || !listed.insert(file.as_str()) {
            return Err(format!("{file}: listed twice, or not a plain file name"));
        }
        let path = dir.join(file);
        let body = std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        bodies.push(body);
    }
    for entry in std::fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))? {
        let name = entry.map_err(|err| err.to_string())?.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".json") && !listed.contains(name.as_ref()) {
            return Err(format!("{name} is not listed in manifest.toml"));
        }
    }

    let mut entries: Vec<(&str, &[u8])> = vec![(MANIFEST, &manifest_json)];
    entries.extend(
        files
            .iter()
            .map(String::as_str)
            .zip(bodies.iter().map(Vec::as_slice)),
    );
    let bytes = pack(&entries);
    let sha256 = Sha256::digest(&bytes).into();
    Ok(Packed { bytes, sha256 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pack_unpacks_to_the_same_files_in_order() {
        let packed = pack(&[(MANIFEST, b"{}"), ("a.json", b"[1]"), ("b.json", b"")]);
        assert_eq!(
            unpack(&packed).unwrap(),
            vec![
                (MANIFEST.to_string(), b"{}".to_vec()),
                ("a.json".to_string(), b"[1]".to_vec()),
                ("b.json".to_string(), Vec::new()),
            ]
        );
    }

    #[test]
    fn anything_else_is_refused() {
        assert_eq!(unpack(b"not zlib"), Err(PackError::NotCompressed));
        let compress = |raw: &[u8]| {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(raw).unwrap();
            encoder.finish().unwrap()
        };
        assert_eq!(unpack(&compress(b"OTHER")), Err(PackError::BadMagic));
        let mut cut = MAGIC.to_vec();
        cut.extend_from_slice(&1u32.to_le_bytes());
        cut.extend_from_slice(&5u32.to_le_bytes());
        cut.extend_from_slice(b"ab");
        assert_eq!(unpack(&compress(&cut)), Err(PackError::Truncated));
        let mut extra = MAGIC.to_vec();
        extra.extend_from_slice(&0u32.to_le_bytes());
        extra.push(0);
        assert_eq!(unpack(&compress(&extra)), Err(PackError::TrailingBytes));
    }

    #[test]
    fn a_pack_that_inflates_past_the_limit_is_refused() {
        let zeros = vec![0u8; MAX_UNPACKED + 1];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&zeros).unwrap();
        let bomb = encoder.finish().unwrap();
        assert_eq!(unpack(&bomb), Err(PackError::TooLarge));
    }
}
