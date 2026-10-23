//! The data bit stream: segments of mode, character count and data, then a
//! terminator and padding.
//!
//! The encoder writes one byte-mode segment. The decoder reads numeric,
//! alphanumeric and byte segments, and names every other mode as
//! unsupported rather than skipping it.

use crate::DecodeError;

const MODE_NUMERIC: u32 = 0b0001;
const MODE_ALPHANUMERIC: u32 = 0b0010;
const MODE_STRUCTURED_APPEND: u32 = 0b0011;
const MODE_BYTE: u32 = 0b0100;
const MODE_FNC1_FIRST: u32 = 0b0101;
const MODE_ECI: u32 = 0b0111;
const MODE_KANJI: u32 = 0b1000;
const MODE_FNC1_SECOND: u32 = 0b1001;
const MODE_HANZI: u32 = 0b1101;

const ALPHANUMERIC: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/// Character-count bits for a mode, ISO/IEC 18004 Table 3.
fn count_bits(mode: u32, version: u8) -> usize {
    let band = match version {
        0..=9 => 0,
        10..=26 => 1,
        _ => 2,
    };
    match mode {
        MODE_NUMERIC => [10, 12, 14][band],
        MODE_ALPHANUMERIC => [9, 11, 13][band],
        _ => [8, 16, 16][band],
    }
}

/// Bits a byte-mode segment of `len` bytes needs.
pub(crate) fn byte_segment_bits(len: usize, version: u8) -> usize {
    4 + count_bits(MODE_BYTE, version) + 8 * len
}

/// `data` as one byte-mode segment, terminated and padded to `capacity`
/// codewords. The caller has checked that it fits.
pub(crate) fn encode_bytes(data: &[u8], version: u8, capacity: usize) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.push(MODE_BYTE, 4);
    w.push(data.len() as u32, count_bits(MODE_BYTE, version));
    for &b in data {
        w.push(b as u32, 8);
    }
    let bits = capacity * 8;
    w.push(0, (bits - w.len).min(4));
    let pad_to_byte = (8 - w.len % 8) % 8;
    w.push(0, pad_to_byte);
    let mut out = w.bytes;
    for pad in [0xEC, 0x11].into_iter().cycle() {
        if out.len() >= capacity {
            break;
        }
        out.push(pad);
    }
    out
}

/// The bytes the segments in `data` carry. Numeric and alphanumeric
/// segments come out as their ASCII text.
pub(crate) fn decode(data: &[u8], version: u8) -> Result<Vec<u8>, DecodeError> {
    let mut r = BitReader { data, pos: 0 };
    let mut out = Vec::new();
    while r.remaining() >= 4 {
        let mode = r.read(4)?;
        match mode {
            0 => break,
            MODE_NUMERIC => {
                let mut count = r.read(count_bits(mode, version))? as usize;
                while count > 0 {
                    let digits = count.min(3);
                    let value = r.read([4, 7, 10][digits - 1])?;
                    if value >= [10, 100, 1000][digits - 1] {
                        return Err(DecodeError::Bitstream);
                    }
                    let text = [value / 100, value / 10 % 10, value % 10];
                    out.extend(text[3 - digits..].iter().map(|&d| b'0' + d as u8));
                    count -= digits;
                }
            }
            MODE_ALPHANUMERIC => {
                let mut count = r.read(count_bits(mode, version))? as usize;
                while count >= 2 {
                    let value = r.read(11)? as usize;
                    if value >= 45 * 45 {
                        return Err(DecodeError::Bitstream);
                    }
                    out.push(ALPHANUMERIC[value / 45]);
                    out.push(ALPHANUMERIC[value % 45]);
                    count -= 2;
                }
                if count == 1 {
                    let value = r.read(6)? as usize;
                    if value >= 45 {
                        return Err(DecodeError::Bitstream);
                    }
                    out.push(ALPHANUMERIC[value]);
                }
            }
            MODE_BYTE => {
                let count = r.read(count_bits(mode, version))? as usize;
                if r.remaining() < count * 8 {
                    return Err(DecodeError::Bitstream);
                }
                for _ in 0..count {
                    out.push(r.read(8)? as u8);
                }
            }
            MODE_ECI
            | MODE_KANJI
            | MODE_HANZI
            | MODE_STRUCTURED_APPEND
            | MODE_FNC1_FIRST
            | MODE_FNC1_SECOND => return Err(DecodeError::UnsupportedMode(mode as u8)),
            _ => return Err(DecodeError::Bitstream),
        }
    }
    Ok(out)
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    len: usize,
}

impl BitWriter {
    fn push(&mut self, value: u32, bits: usize) {
        for i in (0..bits).rev() {
            if self.len.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> i) & 1 != 0 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> (self.len % 8);
            }
            self.len += 1;
        }
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl BitReader<'_> {
    fn remaining(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    fn read(&mut self, bits: usize) -> Result<u32, DecodeError> {
        if bits > self.remaining() {
            return Err(DecodeError::Bitstream);
        }
        let mut v = 0u32;
        for _ in 0..bits {
            let bit = (self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_segment_round_trips() {
        for (version, capacity) in [(1u8, 19usize), (10, 274), (40, 2956)] {
            let data: Vec<u8> = (0..capacity.min(300) / 2).map(|i| (i * 37) as u8).collect();
            let encoded = encode_bytes(&data, version, capacity);
            assert_eq!(encoded.len(), capacity);
            assert_eq!(decode(&encoded, version).unwrap(), data);
        }
    }

    #[test]
    fn padding_alternates() {
        let encoded = encode_bytes(b"A", 1, 19);
        // 0100 00000001 01000001 0000, then 0xEC 0x11 …
        assert_eq!(&encoded[..4], &[0x40, 0x14, 0x10, 0xEC]);
        assert_eq!(encoded[4], 0x11);
    }

    #[test]
    fn numeric_and_alphanumeric() {
        // "01234567" at 1-M: Thonky's numeric example.
        let numeric = [0x10, 0x20, 0x0C, 0x56, 0x61, 0x80, 0xEC, 0x11];
        assert_eq!(decode(&numeric, 1).unwrap(), b"01234567");
        // "AC-42" in alphanumeric: 0010 000000101 then pairs.
        let mut w = BitWriter::default();
        w.push(MODE_ALPHANUMERIC, 4);
        w.push(5, 9);
        w.push(10 * 45 + 12, 11);
        w.push(41 * 45 + 4, 11);
        w.push(2, 6);
        w.push(0, 4);
        assert_eq!(decode(&w.bytes, 1).unwrap(), b"AC-42");
    }

    #[test]
    fn unsupported_and_bad_modes_are_named() {
        assert_eq!(
            decode(&[0x70, 0x00], 1),
            Err(DecodeError::UnsupportedMode(7))
        );
        assert_eq!(
            decode(&[0x80, 0x00], 1),
            Err(DecodeError::UnsupportedMode(8))
        );
        assert_eq!(decode(&[0xF0, 0x00], 1), Err(DecodeError::Bitstream));
    }

    #[test]
    fn a_count_past_the_end_is_rejected() {
        // Byte mode claiming 255 bytes in a 4-byte stream.
        assert_eq!(
            decode(&[0x4F, 0xF0, 0x00, 0x00], 1),
            Err(DecodeError::Bitstream)
        );
    }
}
