//! Per-version constants.
//!
//! Only two tables are data: ECC codewords per block and the number of
//! blocks, by level and version. Everything else is derived: the codeword
//! count from the module layout, and the block split from the count. The
//! alignment table is written out and checked against its formula.

use crate::EcLevel;

pub(crate) const MIN_VERSION: u8 = 1;
pub(crate) const MAX_VERSION: u8 = 40;

/// Modules per side.
pub(crate) fn size(version: u8) -> usize {
    17 + 4 * version as usize
}

// Index 0 is unused so a version indexes directly. Rows: L, M, Q, H.
const ECC_PER_BLOCK: [[u8; 41]; 4] = [
    [
        0, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28,
        30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    [
        0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28,
        28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    ],
    [
        0, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30,
        30, 30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    [
        0, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24,
        30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
];

const NUM_BLOCKS: [[u8; 41]; 4] = [
    [
        0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13,
        14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25,
    ],
    [
        0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21,
        23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
    ],
    [
        0, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29,
        34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68,
    ],
    [
        0, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32,
        35, 37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81,
    ],
];

/// Centre coordinates of the alignment patterns, on both axes.
const ALIGNMENT: [&[u8]; 41] = [
    &[],
    &[],
    &[6, 18],
    &[6, 22],
    &[6, 26],
    &[6, 30],
    &[6, 34],
    &[6, 22, 38],
    &[6, 24, 42],
    &[6, 26, 46],
    &[6, 28, 50],
    &[6, 30, 54],
    &[6, 32, 58],
    &[6, 34, 62],
    &[6, 26, 46, 66],
    &[6, 26, 48, 70],
    &[6, 26, 50, 74],
    &[6, 30, 54, 78],
    &[6, 30, 56, 82],
    &[6, 30, 58, 86],
    &[6, 34, 62, 90],
    &[6, 28, 50, 72, 94],
    &[6, 26, 50, 74, 98],
    &[6, 30, 54, 78, 102],
    &[6, 28, 54, 80, 106],
    &[6, 32, 58, 84, 110],
    &[6, 30, 58, 86, 114],
    &[6, 34, 62, 90, 118],
    &[6, 26, 50, 74, 98, 122],
    &[6, 30, 54, 78, 102, 126],
    &[6, 26, 52, 78, 104, 130],
    &[6, 30, 56, 82, 108, 134],
    &[6, 34, 60, 86, 112, 138],
    &[6, 30, 58, 86, 114, 142],
    &[6, 34, 62, 90, 118, 146],
    &[6, 30, 54, 78, 102, 126, 150],
    &[6, 24, 50, 76, 102, 128, 154],
    &[6, 28, 54, 80, 106, 132, 158],
    &[6, 32, 58, 84, 110, 136, 162],
    &[6, 26, 54, 82, 110, 138, 166],
    &[6, 30, 58, 86, 114, 142, 170],
];

pub(crate) fn alignment_positions(version: u8) -> &'static [u8] {
    ALIGNMENT[version as usize]
}

/// Modules left for codewords once the function patterns, format and
/// version information are placed.
pub(crate) fn raw_data_modules(version: u8) -> usize {
    let v = version as usize;
    let mut result = (16 * v + 128) * v + 64;
    if v >= 2 {
        let align = v / 7 + 2;
        result -= (25 * align - 10) * align - 55;
        if v >= 7 {
            result -= 36;
        }
    }
    result
}

/// Codewords the symbol holds, data and ECC together.
pub(crate) fn total_codewords(version: u8) -> usize {
    raw_data_modules(version) / 8
}

/// How a symbol's codewords split into blocks. Short blocks come first; a
/// long block has one more data codeword.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Blocks {
    pub count: usize,
    pub ecc_len: usize,
    pub short_count: usize,
    pub short_data: usize,
    /// The most errors one block may correct. Below the ECC bound for the
    /// smallest symbols, which reserve codewords against misdecoding.
    pub max_errors: usize,
}

impl Blocks {
    pub(crate) fn new(version: u8, ecc: EcLevel) -> Self {
        let total = total_codewords(version);
        let count = NUM_BLOCKS[ecc.index()][version as usize] as usize;
        let ecc_len = ECC_PER_BLOCK[ecc.index()][version as usize] as usize;
        let short_len = total / count;
        Blocks {
            count,
            ecc_len,
            short_count: count - total % count,
            short_data: short_len - ecc_len,
            max_errors: (ecc_len - misdecode_protection(version, ecc)) / 2,
        }
    }

    pub(crate) fn data_len(&self, block: usize) -> usize {
        self.short_data + usize::from(block >= self.short_count)
    }

    pub(crate) fn total_data(&self) -> usize {
        self.short_data * self.count + (self.count - self.short_count)
    }
}

/// ISO/IEC 18004 Table 9, footnote: codewords kept for misdecode protection.
fn misdecode_protection(version: u8, ecc: EcLevel) -> usize {
    match (version, ecc) {
        (1, EcLevel::L) => 3,
        (1, EcLevel::M) | (2, EcLevel::L) => 2,
        (1, _) | (3, EcLevel::L) => 1,
        _ => 0,
    }
}

/// Data codewords the symbol holds.
pub(crate) fn data_codewords(version: u8, ecc: EcLevel) -> usize {
    Blocks::new(version, ecc).total_data()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_table_matches_its_formula() {
        for v in 2..=MAX_VERSION {
            let n = v as usize / 7 + 2;
            let step = (v as usize * 8 + n * 3 + 5) / (n * 4 - 4) * 2;
            let mut expected = vec![6usize];
            expected.extend((0..n - 1).rev().map(|k| size(v) - 7 - k * step));
            let got: Vec<usize> = alignment_positions(v).iter().map(|&p| p as usize).collect();
            assert_eq!(got, expected, "version {v}");
        }
    }

    #[test]
    fn known_capacities() {
        // Byte-mode capacity, ISO/IEC 18004 Table 7: data codewords minus
        // the 4-bit mode and the character count.
        let byte_capacity = |v: u8, ecc: EcLevel| {
            let count_bits = if v <= 9 { 8 } else { 16 };
            (data_codewords(v, ecc) * 8 - 4 - count_bits) / 8
        };
        let cases = [
            (1, EcLevel::L, 17),
            (1, EcLevel::M, 14),
            (1, EcLevel::Q, 11),
            (1, EcLevel::H, 7),
            (2, EcLevel::M, 26),
            (5, EcLevel::Q, 60),
            (7, EcLevel::H, 64),
            (10, EcLevel::M, 213),
            (15, EcLevel::L, 520),
            (20, EcLevel::M, 666),
            (22, EcLevel::M, 779),
            (23, EcLevel::M, 857),
            (27, EcLevel::Q, 805),
            (40, EcLevel::L, 2953),
            (40, EcLevel::M, 2331),
            (40, EcLevel::Q, 1663),
            (40, EcLevel::H, 1273),
        ];
        for (v, ecc, cap) in cases {
            assert_eq!(byte_capacity(v, ecc), cap, "{v}-{ecc:?}");
        }
    }

    #[test]
    fn known_block_splits() {
        // 5-Q: 2 blocks of 15 data codewords and 2 of 16, 18 ECC each.
        let b = Blocks::new(5, EcLevel::Q);
        assert_eq!(
            (b.count, b.short_count, b.short_data, b.ecc_len),
            (4, 2, 15, 18)
        );
        // 40-H: 20 blocks of 15 and 61 of 16, 30 ECC each.
        let b = Blocks::new(40, EcLevel::H);
        assert_eq!(
            (b.count, b.short_count, b.short_data, b.ecc_len),
            (81, 20, 15, 30)
        );
        assert_eq!(total_codewords(1), 26);
        assert_eq!(total_codewords(40), 3706);
    }

    #[test]
    fn misdecode_protection_matches_the_spec() {
        // Table 9 lists the corrections each small symbol allows.
        assert_eq!(Blocks::new(1, EcLevel::L).max_errors, 2);
        assert_eq!(Blocks::new(1, EcLevel::M).max_errors, 4);
        assert_eq!(Blocks::new(1, EcLevel::Q).max_errors, 6);
        assert_eq!(Blocks::new(1, EcLevel::H).max_errors, 8);
        assert_eq!(Blocks::new(2, EcLevel::L).max_errors, 4);
        assert_eq!(Blocks::new(3, EcLevel::L).max_errors, 7);
        assert_eq!(Blocks::new(40, EcLevel::H).max_errors, 15);
    }
}
