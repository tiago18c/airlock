//! Encoding: bytes to a symbol. One byte-mode segment, the smallest version
//! that fits, and the mask with the lowest penalty.

use crate::bch::{format_word, version_word};
use crate::bitstream::{byte_segment_bits, encode_bytes};
use crate::matrix::{format_positions, mask_bit, version_positions, BitMatrix, Layout};
use crate::rs;
use crate::tables::{data_codewords, Blocks, MAX_VERSION, MIN_VERSION};
use crate::{EcLevel, EncodeError};

/// An encoded symbol, without its quiet zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    matrix: BitMatrix,
    version: u8,
    ecc: EcLevel,
    mask: u8,
}

impl Symbol {
    pub fn matrix(&self) -> &BitMatrix {
        &self.matrix
    }
    pub fn size(&self) -> usize {
        self.matrix.size()
    }
    pub fn version(&self) -> u8 {
        self.version
    }
    pub fn ecc(&self) -> EcLevel {
        self.ecc
    }
    pub fn mask(&self) -> u8 {
        self.mask
    }
}

/// `data` in the smallest symbol that holds it at `ecc`.
pub fn encode(data: &[u8], ecc: EcLevel) -> Result<Symbol, EncodeError> {
    encode_with(data, ecc, None, None)
}

/// Like [`encode`], with the version or mask fixed. A fixed version that is
/// too small is an error.
pub fn encode_with(
    data: &[u8],
    ecc: EcLevel,
    version: Option<u8>,
    mask: Option<u8>,
) -> Result<Symbol, EncodeError> {
    if mask.is_some_and(|m| m > 7) {
        return Err(EncodeError::BadMask);
    }
    let fits = |v: u8| byte_segment_bits(data.len(), v) <= data_codewords(v, ecc) * 8;
    let version = match version {
        Some(v) if !(MIN_VERSION..=MAX_VERSION).contains(&v) => {
            return Err(EncodeError::BadVersion)
        }
        Some(v) if fits(v) => v,
        Some(_) => return Err(too_long(data, ecc)),
        None => (MIN_VERSION..=MAX_VERSION)
            .find(|&v| fits(v))
            .ok_or_else(|| too_long(data, ecc))?,
    };

    let blocks = Blocks::new(version, ecc);
    let codewords = interleave(&encode_bytes(data, version, blocks.total_data()), &blocks);

    let layout = Layout::new(version);
    let mut unmasked = layout.base.clone();
    for (i, &(x, y)) in layout.data.iter().enumerate() {
        let bit = codewords
            .get(i / 8)
            .is_some_and(|&c| (c >> (7 - i % 8)) & 1 != 0);
        unmasked.set(x as usize, y as usize, bit);
    }
    if version >= 7 {
        let word = version_word(version);
        for copy in version_positions(layout.size) {
            for (i, (x, y)) in copy.into_iter().enumerate() {
                unmasked.set(x, y, (word >> i) & 1 != 0);
            }
        }
    }

    let masked = |m: u8| {
        let mut out = unmasked.clone();
        for &(x, y) in &layout.data {
            if mask_bit(m, x as usize, y as usize) {
                out.flip(x as usize, y as usize);
            }
        }
        let word = format_word(ecc, m);
        for copy in format_positions(layout.size) {
            for (i, (x, y)) in copy.into_iter().enumerate() {
                out.set(x, y, (word >> i) & 1 != 0);
            }
        }
        out
    };

    let (mask, matrix) = match mask {
        Some(m) => (m, masked(m)),
        None => (0..8)
            .map(|m| (m, masked(m)))
            .min_by_key(|(_, matrix)| penalty(matrix))
            .expect("eight masks"),
    };
    Ok(Symbol {
        matrix,
        version,
        ecc,
        mask,
    })
}

fn too_long(data: &[u8], ecc: EcLevel) -> EncodeError {
    let max = (data_codewords(MAX_VERSION, ecc) * 8 - byte_segment_bits(0, MAX_VERSION)) / 8;
    EncodeError::TooLong {
        len: data.len(),
        max,
    }
}

/// Splits data codewords into blocks, adds each block's ECC, and
/// interleaves: data column by column, then ECC column by column.
fn interleave(data: &[u8], blocks: &Blocks) -> Vec<u8> {
    let mut split = Vec::with_capacity(blocks.count);
    let mut at = 0;
    for b in 0..blocks.count {
        let len = blocks.data_len(b);
        let block = &data[at..at + len];
        at += len;
        split.push((block, rs::ecc(block, blocks.ecc_len)));
    }
    let mut out = Vec::with_capacity(data.len() + blocks.count * blocks.ecc_len);
    for i in 0..=blocks.short_data {
        for (block, _) in &split {
            if let Some(&c) = block.get(i) {
                out.push(c);
            }
        }
    }
    for i in 0..blocks.ecc_len {
        for (_, ecc) in &split {
            out.push(ecc[i]);
        }
    }
    out
}

/// The mask penalty of ISO/IEC 18004 §7.8.3: runs, 2×2 blocks,
/// finder-like patterns and dark balance.
fn penalty(m: &BitMatrix) -> u32 {
    let n = m.size();
    let mut score = 0u32;

    let line = |i: usize, horizontal: bool| -> Vec<bool> {
        (0..n)
            .map(|j| if horizontal { m.get(j, i) } else { m.get(i, j) })
            .collect()
    };
    for i in 0..n {
        for horizontal in [true, false] {
            let l = line(i, horizontal);
            // N1: a run of five or more, 3 plus 1 per extra module.
            let mut run = 1;
            for j in 1..=n {
                if j < n && l[j] == l[j - 1] {
                    run += 1;
                } else {
                    if run >= 5 {
                        score += 3 + (run - 5);
                    }
                    run = 1;
                }
            }
            // N3: 1:1:3:1:1 with four light modules on either side.
            const FINDER: [bool; 7] = [true, false, true, true, true, false, true];
            for j in 0..n.saturating_sub(6) {
                if l[j..j + 7] == FINDER {
                    let before = (1..=4).all(|d| j < d || !l[j - d]);
                    let after = (0..4).all(|d| j + 7 + d >= n || !l[j + 7 + d]);
                    if before || after {
                        score += 40;
                    }
                }
            }
        }
    }

    // N2: each 2×2 block of one colour.
    for y in 0..n - 1 {
        for x in 0..n - 1 {
            let c = m.get(x, y);
            if c == m.get(x + 1, y) && c == m.get(x, y + 1) && c == m.get(x + 1, y + 1) {
                score += 3;
            }
        }
    }

    // N4: 10 per 5 % the dark share is away from half.
    let total = (n * n) as u32;
    let dark = (0..n)
        .flat_map(|y| (0..n).map(move |x| (x, y)))
        .filter(|&(x, y)| m.get(x, y))
        .count() as u32;
    let k = (dark * 20)
        .abs_diff(total * 10)
        .div_ceil(total)
        .saturating_sub(1);
    score + k * 10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_smallest_version() {
        assert_eq!(encode(&[0; 17], EcLevel::L).unwrap().version(), 1);
        assert_eq!(encode(&[0; 18], EcLevel::L).unwrap().version(), 2);
        assert_eq!(encode(&[0; 2331], EcLevel::M).unwrap().version(), 40);
        assert_eq!(
            encode(&[0; 2332], EcLevel::M),
            Err(EncodeError::TooLong {
                len: 2332,
                max: 2331
            })
        );
    }

    #[test]
    fn rejects_bad_options() {
        assert_eq!(
            encode_with(b"x", EcLevel::M, Some(0), None),
            Err(EncodeError::BadVersion)
        );
        assert_eq!(
            encode_with(b"x", EcLevel::M, Some(41), None),
            Err(EncodeError::BadVersion)
        );
        assert_eq!(
            encode_with(b"x", EcLevel::M, None, Some(8)),
            Err(EncodeError::BadMask)
        );
        assert!(matches!(
            encode_with(&[0; 100], EcLevel::M, Some(1), None),
            Err(EncodeError::TooLong { .. })
        ));
    }
}
