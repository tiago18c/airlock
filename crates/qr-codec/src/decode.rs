//! Decoding a sampled grid: format, version, unmask, read codewords,
//! de-interleave, correct, parse.

use crate::bch::{decode_format, decode_version};
use crate::bitstream;
use crate::matrix::{format_positions, mask_bit, version_positions, BitMatrix, Layout};
use crate::rs;
use crate::tables::{Blocks, MAX_VERSION};
use crate::{DecodeError, EcLevel};

/// What a symbol carried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    /// The segments' bytes, concatenated. Byte mode is passed through raw.
    pub bytes: Vec<u8>,
    pub version: u8,
    pub ecc: EcLevel,
    pub mask: u8,
    /// Codewords Reed–Solomon had to fix, over all blocks.
    pub corrected: usize,
}

/// Decodes a grid of modules, without its quiet zone.
pub fn decode_matrix(m: &BitMatrix) -> Result<Decoded, DecodeError> {
    let size = m.size();
    if !(21..=177).contains(&size) || size % 4 != 1 {
        return Err(DecodeError::Size);
    }
    let version = ((size - 17) / 4) as u8;
    if version >= 7 {
        // The size says which version this is; the version information
        // only has to agree. When both copies are unreadable the
        // codewords' own ECC still decides.
        let read = version_positions(size).map(|copy| read_word(m, &copy) as u32);
        let best = read
            .iter()
            .filter_map(|&w| decode_version(w))
            .min_by_key(|&(_, d)| d);
        if let Some((v, _)) = best {
            if v != version {
                return Err(DecodeError::Version);
            }
        }
    }

    let mut formats: Vec<(EcLevel, u8, u32)> = format_positions(size)
        .iter()
        .filter_map(|copy| decode_format(read_word(m, copy) as u16))
        .collect();
    formats.sort_by_key(|&(_, _, d)| d);
    formats.dedup_by_key(|&mut (e, k, _)| (e, k));
    if formats.is_empty() {
        return Err(DecodeError::Format);
    }

    let layout = Layout::new(version);
    let mut last = DecodeError::Format;
    for (ecc, mask, _) in formats {
        match decode_with(m, &layout, version, ecc, mask) {
            Ok(d) => return Ok(d),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// The version a grid's own version information names, when readable.
pub(crate) fn version_info(m: &BitMatrix) -> Option<u8> {
    if m.size() < 45 {
        return None;
    }
    version_positions(m.size())
        .iter()
        .filter_map(|copy| decode_version(read_word(m, copy) as u32))
        .min_by_key(|&(_, d)| d)
        .map(|(v, _)| v)
}

/// Whether a grid that failed to decode still looks like a symbol: both
/// copies of the format information name the same level and mask, and the
/// timing patterns alternate. Noise passes one format copy about half the
/// time, so one copy alone says nothing.
pub(crate) fn looks_like_symbol(m: &BitMatrix) -> bool {
    let size = m.size();
    if size < 21 {
        return false;
    }
    let [a, b] = format_positions(size).map(|copy| decode_format(read_word(m, &copy) as u16));
    let agree = matches!((a, b), (Some((e1, m1, _)), Some((e2, m2, _))) if e1 == e2 && m1 == m2);
    let timing = 8..size - 8;
    let right = timing
        .clone()
        .filter(|&i| m.get(i, 6) == (i % 2 == 0))
        .count()
        + timing
            .clone()
            .filter(|&i| m.get(6, i) == (i % 2 == 0))
            .count();
    agree && right * 5 >= timing.len() * 2 * 4
}

fn read_word(m: &BitMatrix, positions: &[(usize, usize)]) -> u64 {
    positions
        .iter()
        .enumerate()
        .fold(0, |w, (i, &(x, y))| w | (u64::from(m.get(x, y)) << i))
}

fn decode_with(
    m: &BitMatrix,
    layout: &Layout,
    version: u8,
    ecc: EcLevel,
    mask: u8,
) -> Result<Decoded, DecodeError> {
    debug_assert!(version <= MAX_VERSION);
    let blocks = Blocks::new(version, ecc);
    let total = blocks.total_data() + blocks.count * blocks.ecc_len;

    let mut stream = vec![0u8; total];
    for (i, &(x, y)) in layout.data.iter().take(total * 8).enumerate() {
        let (x, y) = (x as usize, y as usize);
        if m.get(x, y) != mask_bit(mask, x, y) {
            stream[i / 8] |= 0x80 >> (i % 8);
        }
    }

    // Undo the interleaving: data column by column, then ECC.
    let mut split: Vec<Vec<u8>> = (0..blocks.count)
        .map(|b| Vec::with_capacity(blocks.data_len(b) + blocks.ecc_len))
        .collect();
    let mut next = stream.iter().copied();
    for i in 0..=blocks.short_data {
        for (b, block) in split.iter_mut().enumerate() {
            if i < blocks.data_len(b) {
                block.extend(next.next());
            }
        }
    }
    for _ in 0..blocks.ecc_len {
        for block in split.iter_mut() {
            block.extend(next.next());
        }
    }

    let mut data = Vec::with_capacity(blocks.total_data());
    let mut corrected = 0;
    for (b, block) in split.iter_mut().enumerate() {
        corrected +=
            rs::correct(block, blocks.ecc_len, blocks.max_errors).map_err(|_| DecodeError::Ecc)?;
        data.extend_from_slice(&block[..blocks.data_len(b)]);
    }

    Ok(Decoded {
        bytes: bitstream::decode(&data, version)?,
        version,
        ecc,
        mask,
        corrected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::byte_segment_bits;
    use crate::encode::encode_with;
    use crate::tables::data_codewords;

    fn capacity(version: u8, ecc: EcLevel) -> usize {
        (data_codewords(version, ecc) * 8 - byte_segment_bits(0, version)) / 8
    }

    #[test]
    fn round_trips_every_version_level_and_mask() {
        for version in 1..=40u8 {
            for ecc in EcLevel::ALL {
                let mask = version % 8;
                let len = (1 + (version as usize * 13) % 50).min(capacity(version, ecc));
                let data: Vec<u8> = (0..len)
                    .map(|i| (i * 31 + version as usize) as u8)
                    .collect();
                let s = encode_with(&data, ecc, Some(version), Some(mask)).unwrap();
                let d = decode_matrix(s.matrix()).unwrap();
                assert_eq!(d.bytes, data);
                assert_eq!(
                    (d.version, d.ecc, d.mask, d.corrected),
                    (version, ecc, mask, 0)
                );
            }
        }
    }

    /// Flips every bit of `count` codewords in each block.
    fn damage(m: &mut BitMatrix, version: u8, ecc: EcLevel, per_block: usize) {
        let layout = Layout::new(version);
        let blocks = Blocks::new(version, ecc);
        // Stream order of (block, codeword in block).
        let mut order = Vec::new();
        for i in 0..=blocks.short_data {
            for b in 0..blocks.count {
                if i < blocks.data_len(b) {
                    order.push((b, i));
                }
            }
        }
        for i in 0..blocks.ecc_len {
            for b in 0..blocks.count {
                order.push((b, blocks.data_len(b) + i));
            }
        }
        let position = |b: usize, i: usize| order.iter().position(|&o| o == (b, i)).unwrap();
        for b in 0..blocks.count {
            let len = blocks.data_len(b) + blocks.ecc_len;
            for k in 0..per_block {
                let i = (b * 3 + k) % len;
                let p = position(b, i);
                for bit in 0..8 {
                    let (x, y) = layout.data[p * 8 + bit];
                    m.flip(x as usize, y as usize);
                }
            }
        }
    }

    #[test]
    fn corrects_the_most_errors_each_block_allows() {
        for version in [1u8, 2, 3, 5, 7, 10, 20, 27, 40] {
            for ecc in EcLevel::ALL {
                let data = vec![0x5A; capacity(version, ecc).min(10)];
                let s = encode_with(&data, ecc, Some(version), Some(3)).unwrap();
                let blocks = Blocks::new(version, ecc);
                let mut m = s.matrix().clone();
                damage(&mut m, version, ecc, blocks.max_errors);
                let d = decode_matrix(&m).unwrap_or_else(|e| panic!("{version}-{ecc:?}: {e:?}"));
                assert_eq!(d.bytes, data);
                assert_eq!(d.corrected, blocks.max_errors * blocks.count);

                let mut worse = s.matrix().clone();
                damage(&mut worse, version, ecc, blocks.max_errors + 1);
                assert!(
                    decode_matrix(&worse).map(|d| d.bytes) != Ok(data.clone()),
                    "{version}-{ecc:?} read past its bound"
                );
            }
        }
    }

    #[test]
    fn reads_with_one_format_copy_destroyed() {
        let s = encode_with(b"format", EcLevel::Q, Some(4), Some(5)).unwrap();
        let mut m = s.matrix().clone();
        for (x, y) in format_positions(m.size())[0] {
            m.flip(x, y);
        }
        assert_eq!(decode_matrix(&m).unwrap().bytes, b"format");
    }

    #[test]
    fn rejects_a_wrong_version_word() {
        let s = encode_with(b"v", EcLevel::L, Some(7), None).unwrap();
        let mut m = s.matrix().clone();
        let word = crate::bch::version_word(8);
        for copy in version_positions(m.size()) {
            for (i, (x, y)) in copy.into_iter().enumerate() {
                m.set(x, y, (word >> i) & 1 != 0);
            }
        }
        assert_eq!(decode_matrix(&m), Err(DecodeError::Version));
    }

    #[test]
    fn bad_sizes() {
        assert_eq!(decode_matrix(&BitMatrix::new(20)), Err(DecodeError::Size));
        assert_eq!(decode_matrix(&BitMatrix::new(22)), Err(DecodeError::Size));
        assert_eq!(decode_matrix(&BitMatrix::new(181)), Err(DecodeError::Size));
        assert_eq!(decode_matrix(&BitMatrix::new(21)), Err(DecodeError::Format));
    }
}
