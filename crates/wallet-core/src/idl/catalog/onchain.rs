//! IDLs as programs publish them on chain: the program-metadata account and
//! the legacy Anchor IDL account.

use super::*;

/// Program that owns canonical IDL metadata accounts.
pub const PROGRAM_METADATA_ID: &str = "ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S";

pub(super) const METADATA_HEADER_LEN: usize = 96;

pub(super) const METADATA_SEED_LEN: usize = 16;

/// PDA of the program-metadata `idl` account: seeds `[program_id, "idl" padded to 16]`.
pub fn program_metadata_idl_address(program_id: &Pubkey) -> Result<Pubkey, IdlError> {
    let metadata =
        Pubkey::from_str(PROGRAM_METADATA_ID).map_err(|err| IdlError::Invalid(err.to_string()))?;
    let mut seed = [0u8; METADATA_SEED_LEN];
    seed[..3].copy_from_slice(b"idl");
    let (pda, _) = Pubkey::find_program_address(&[program_id.as_ref(), &seed], &metadata);
    Ok(pda)
}

/// IDL JSON stored in a program-metadata account.
///
/// A 96-byte header is followed by the data section. `encoding` is at byte 83,
/// `compression` at 84, `data_source` at 86, and `data_length` is a little-endian
/// u32 at bytes 87..91. Only inline data (`data_source` 0) is accepted.
pub fn idl_bytes_from_program_metadata(data: &[u8]) -> Result<Vec<u8>, IdlError> {
    if data.len() < METADATA_HEADER_LEN {
        return Err(IdlError::Invalid(
            "program-metadata account is too short".into(),
        ));
    }
    if data[0] != 2 {
        return Err(IdlError::Invalid(
            "on-chain account is not program metadata".into(),
        ));
    }
    let encoding = data[83];
    let compression = data[84];
    let data_source = data[86];
    let data_length = u32::from_le_bytes(data[87..91].try_into().unwrap()) as usize;
    if data_source != 0 {
        return Err(IdlError::Invalid(
            "program-metadata IDL is not stored inline".into(),
        ));
    }
    let end = METADATA_HEADER_LEN
        .checked_add(data_length)
        .filter(|end| *end <= data.len())
        .ok_or_else(|| IdlError::Invalid("program-metadata length is out of range".into()))?;
    let inflated = inflate_metadata(&data[METADATA_HEADER_LEN..end], compression)?;
    decode_metadata_text(inflated, encoding)
}

pub(super) fn inflate_metadata(raw: &[u8], compression: u8) -> Result<Vec<u8>, IdlError> {
    match compression {
        0 => Ok(raw.to_vec()),
        1 => inflate_bounded(flate2::read::GzDecoder::new(raw))
            .map_err(|err| inflate_error("program-metadata gzip", err)),
        2 => inflate_bounded(flate2::read::ZlibDecoder::new(raw))
            .map_err(|err| inflate_error("program-metadata zlib", err)),
        other => Err(IdlError::Invalid(format!(
            "unknown program-metadata compression {other}"
        ))),
    }
}

pub(super) fn decode_metadata_text(bytes: Vec<u8>, encoding: u8) -> Result<Vec<u8>, IdlError> {
    match encoding {
        0 | 1 => Ok(bytes),
        2 => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| IdlError::Invalid("program-metadata base58 is not text".into()))?;
            bs58::decode(text.trim())
                .into_vec()
                .map_err(|err| IdlError::Invalid(format!("program-metadata base58 failed: {err}")))
        }
        3 => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| IdlError::Invalid("program-metadata base64 is not text".into()))?;
            base64::engine::general_purpose::STANDARD
                .decode(text.trim())
                .map_err(|err| IdlError::Invalid(format!("program-metadata base64 failed: {err}")))
        }
        other => Err(IdlError::Invalid(format!(
            "unknown program-metadata encoding {other}"
        ))),
    }
}

/// Address of the Anchor IDL account for `program_id`.
///
/// `find_program_address([], program_id)` is the base, then
/// `create_with_seed(base, "anchor:idl", program_id)`.
pub fn anchor_idl_address(program_id: &Pubkey) -> Result<Pubkey, IdlError> {
    let (base, _) = Pubkey::find_program_address(&[], program_id);
    Pubkey::create_with_seed(&base, "anchor:idl", program_id)
        .map_err(|err| IdlError::Invalid(err.to_string()))
}

/// Inflate the IDL JSON stored in an Anchor IDL account.
///
/// Layout: 8-byte discriminator, 32-byte authority, then a Borsh `Vec<u8>` of
/// zlib-compressed JSON. The account bytes are not the catalog file.
///
/// Anchor's on-chain IDL account is `#[account("internal")]`, so its tag is
/// `sha256("internal:IdlAccount")[..8]`. Older accounts use
/// `sha256("account:IdlAccount")[..8]`.
pub fn idl_bytes_from_anchor_account(data: &[u8]) -> Result<Vec<u8>, IdlError> {
    const HEADER: usize = 8 + 32;
    if data.len() < HEADER + 4 {
        return Err(IdlError::Invalid(
            "on-chain IDL account is too short".into(),
        ));
    }
    let tag = &data[..8];
    if tag != anchor_idl_discriminator() && tag != internal_idl_discriminator() {
        return Err(IdlError::Invalid(
            "on-chain account is not an Anchor IDL account".into(),
        ));
    }
    let len = u32::from_le_bytes(data[HEADER..HEADER + 4].try_into().unwrap()) as usize;
    let start = HEADER + 4;
    let end = start
        .checked_add(len)
        .filter(|end| *end <= data.len())
        .ok_or_else(|| IdlError::Invalid("on-chain IDL length is out of range".into()))?;
    let inflated = inflate_bounded(flate2::read::ZlibDecoder::new(&data[start..end]))
        .map_err(|err| inflate_error("on-chain IDL", err))?;
    if inflated.is_empty() {
        return Err(IdlError::Empty);
    }
    Ok(inflated)
}

/// The bytes of an Anchor IDL account that holds `json`, in the layout
/// [`idl_bytes_from_anchor_account`] reads. For putting one on a test chain.
pub fn anchor_idl_account_data(json: &[u8]) -> Vec<u8> {
    let mut compressed = Vec::new();
    let mut encoder =
        flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, json).expect("writes to memory");
    encoder.finish().expect("writes to memory");
    let mut data = internal_idl_discriminator().to_vec();
    data.extend_from_slice(&[0u8; 32]);
    data.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    data.extend_from_slice(&compressed);
    data
}

/// The bytes of a program-metadata account that holds `json` inline and
/// uncompressed, in the layout [`idl_bytes_from_program_metadata`] reads.
/// For putting one on a test chain.
pub fn program_metadata_account_data(json: &[u8]) -> Vec<u8> {
    let mut data = vec![0u8; METADATA_HEADER_LEN];
    data[0] = 2;
    data[87..91].copy_from_slice(&(json.len() as u32).to_le_bytes());
    data.extend_from_slice(json);
    data
}

/// Largest IDL the app stores, compressed or not.
pub const MAX_IDL_BYTES: usize = 2_000_000;

pub(super) enum InflateError {
    TooLarge,
    Io(std::io::Error),
}

/// Inflate at most [`MAX_IDL_BYTES`]. A small account can expand to gigabytes,
/// so the limit applies while reading, not afterwards.
pub(super) fn inflate_bounded(decoder: impl std::io::Read) -> Result<Vec<u8>, InflateError> {
    let mut out = Vec::new();
    let mut limited = std::io::Read::take(decoder, MAX_IDL_BYTES as u64 + 1);
    std::io::Read::read_to_end(&mut limited, &mut out).map_err(InflateError::Io)?;
    if out.len() > MAX_IDL_BYTES {
        return Err(InflateError::TooLarge);
    }
    Ok(out)
}

pub(super) fn inflate_error(what: &str, err: InflateError) -> IdlError {
    match err {
        InflateError::TooLarge => IdlError::Invalid(format!("{what} is larger than 2 MB")),
        InflateError::Io(err) => IdlError::Invalid(format!("{what} did not inflate: {err}")),
    }
}

pub(super) fn anchor_idl_discriminator() -> [u8; 8] {
    idl_discriminator(b"account:IdlAccount")
}

/// Tag written by Anchor's `#[account("internal")]` IDL account.
pub(super) fn internal_idl_discriminator() -> [u8; 8] {
    idl_discriminator(b"internal:IdlAccount")
}

pub(super) fn idl_discriminator(preimage: &[u8]) -> [u8; 8] {
    let digest = Sha256::digest(preimage);
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest[..8]);
    out
}
