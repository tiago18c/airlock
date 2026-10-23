//! One frame per QR symbol: how a payload is cut up and read back.

use super::*;
use wincode::{SchemaRead, SchemaWrite};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub version: u8,
    pub transfer_id: [u8; 8],
    pub index: u16,
    pub count: u16,
    pub total_len: u32,
    pub crc32: u32,
    pub chunk: Vec<u8>,
}

/// The fixed part of a frame, as it is on the wire.
#[derive(SchemaWrite, SchemaRead)]
struct Header {
    magic: [u8; 4],
    version: u8,
    transfer_id: [u8; 8],
    index: u16,
    count: u16,
    total_len: u32,
    crc32: u32,
}

const _: () = assert!(
    match <Header as SchemaWrite<wincode::config::DefaultConfig>>::TYPE_META {
        wincode::TypeMeta::Static { size, .. } => size == HEADER_LEN,
        _ => false,
    }
);

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let header = Header {
            magic: *MAGIC,
            version: self.version,
            transfer_id: self.transfer_id,
            index: self.index,
            count: self.count,
            total_len: self.total_len,
            crc32: self.crc32,
        };
        let mut out = wincode::serialize(&header).expect("a fixed-size header");
        out.extend_from_slice(&self.chunk);
        out
    }
}

pub fn transfer_id(payload: &[u8]) -> [u8; 8] {
    let a = crc32(payload);
    let mut mix = Vec::with_capacity(payload.len() + 4);
    mix.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    mix.extend_from_slice(payload);
    mix.reverse();
    let b = crc32(&mix);
    let mut id = [0u8; 8];
    id[..4].copy_from_slice(&a.to_le_bytes());
    id[4..].copy_from_slice(&b.to_le_bytes());
    id
}

pub fn clamp_chunk_size(requested: usize) -> usize {
    requested.clamp(MIN_CHUNK, MAX_FRAME_BYTES - HEADER_LEN)
}

/// Cut `payload` into frames of `chunk_size` bytes each. The last chunk is
/// padded with zeros, so every frame, and every QR code drawn from one, is
/// the same size.
pub fn encode_frames(payload: &[u8], chunk_size: usize) -> Result<Vec<Frame>, ProtocolError> {
    if payload.is_empty() {
        return Err(ProtocolError::Empty);
    }
    if payload.len() > MAX_PAYLOAD {
        return Err(ProtocolError::TooLarge { len: payload.len() });
    }

    let chunk_size = clamp_chunk_size(chunk_size);
    let count = payload.len().div_ceil(chunk_size) as u16;
    let id = transfer_id(payload);
    let crc = crc32(payload);
    let total_len = payload.len() as u32;

    let frames = payload
        .chunks(chunk_size)
        .enumerate()
        .map(|(i, chunk)| {
            let mut chunk = chunk.to_vec();
            chunk.resize(chunk_size, 0);
            Frame {
                version: VERSION,
                transfer_id: id,
                index: i as u16,
                count,
                total_len,
                crc32: crc,
                chunk,
            }
        })
        .collect();
    Ok(frames)
}

pub fn parse_frame(bytes: &[u8]) -> Result<Frame, ProtocolError> {
    let (head, chunk) = bytes
        .split_at_checked(HEADER_LEN)
        .ok_or(ProtocolError::Truncated)?;
    let header: Header = wincode::deserialize_exact(head).map_err(|_| ProtocolError::Truncated)?;
    if &header.magic != MAGIC {
        return Err(ProtocolError::BadMagic);
    }
    if header.version != VERSION {
        return Err(ProtocolError::UnsupportedVersion(header.version));
    }
    if header.count == 0 || header.index >= header.count {
        return Err(ProtocolError::BadHeader);
    }
    if header.total_len == 0 || header.total_len as usize > MAX_PAYLOAD {
        return Err(ProtocolError::BadHeader);
    }

    Ok(Frame {
        version: header.version,
        transfer_id: header.transfer_id,
        index: header.index,
        count: header.count,
        total_len: header.total_len,
        crc32: header.crc32,
        chunk: chunk.to_vec(),
    })
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

pub fn hex_id(id: &[u8; 8]) -> String {
    hex::encode(id)
}
