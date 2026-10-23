//! The two BCH codes that protect a symbol's metadata: format information
//! (BCH(15,5), then XOR 0x5412) and version information (BCH(18,6)).
//!
//! Both have only 32 or 34 valid words, so decoding is a nearest-word search.
//! A word is accepted within Hamming distance 3, which both codes correct.

use crate::EcLevel;

const FORMAT_GEN: u32 = 0x537;
const FORMAT_MASK: u32 = 0x5412;
const VERSION_GEN: u32 = 0x1F25;
const MAX_DISTANCE: u32 = 3;

/// The 15-bit format word for a level and mask, bit 14 first on the symbol.
pub(crate) fn format_word(ecc: EcLevel, mask: u8) -> u16 {
    let data = ((ecc.format_bits() as u32) << 3) | (mask as u32 & 7);
    (((data << 10) | remainder(data << 10, 14, FORMAT_GEN, 10)) ^ FORMAT_MASK) as u16
}

/// The 18-bit version word, for versions 7 to 40.
pub(crate) fn version_word(version: u8) -> u32 {
    let data = version as u32;
    (data << 12) | remainder(data << 12, 17, VERSION_GEN, 12)
}

fn remainder(mut value: u32, top: u32, generator: u32, degree: u32) -> u32 {
    for i in (degree..=top).rev() {
        if value & (1 << i) != 0 {
            value ^= generator << (i - degree);
        }
    }
    value
}

/// The level and mask nearest `word`, with the distance.
pub(crate) fn decode_format(word: u16) -> Option<(EcLevel, u8, u32)> {
    let mut best: Option<(EcLevel, u8, u32)> = None;
    for ecc in EcLevel::ALL {
        for mask in 0..8 {
            let d = (format_word(ecc, mask) ^ word).count_ones();
            if d <= MAX_DISTANCE && best.is_none_or(|b| d < b.2) {
                best = Some((ecc, mask, d));
            }
        }
    }
    best
}

/// The version nearest `word`, with the distance.
pub(crate) fn decode_version(word: u32) -> Option<(u8, u32)> {
    let mut best: Option<(u8, u32)> = None;
    for version in 7..=40 {
        let d = (version_word(version) ^ word).count_ones();
        if d <= MAX_DISTANCE && best.is_none_or(|b| d < b.1) {
            best = Some((version, d));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_words_match_the_spec_table() {
        // ISO/IEC 18004 Table C.1, by level and mask 0..7.
        let table: [(EcLevel, [u16; 8]); 4] = [
            (
                EcLevel::L,
                [
                    0x77C4, 0x72F3, 0x7DAA, 0x789D, 0x662F, 0x6318, 0x6C41, 0x6976,
                ],
            ),
            (
                EcLevel::M,
                [
                    0x5412, 0x5125, 0x5E7C, 0x5B4B, 0x45F9, 0x40CE, 0x4F97, 0x4AA0,
                ],
            ),
            (
                EcLevel::Q,
                [
                    0x355F, 0x3068, 0x3F31, 0x3A06, 0x24B4, 0x2183, 0x2EDA, 0x2BED,
                ],
            ),
            (
                EcLevel::H,
                [
                    0x1689, 0x13BE, 0x1CE7, 0x19D0, 0x0762, 0x0255, 0x0D0C, 0x083B,
                ],
            ),
        ];
        for (ecc, words) in table {
            for (mask, &w) in words.iter().enumerate() {
                assert_eq!(format_word(ecc, mask as u8), w, "{ecc:?} mask {mask}");
            }
        }
    }

    #[test]
    fn version_words_match_the_spec_table() {
        // ISO/IEC 18004 Table D.1.
        let table: [u32; 34] = [
            0x07C94, 0x085BC, 0x09A99, 0x0A4D3, 0x0BBF6, 0x0C762, 0x0D847, 0x0E60D, 0x0F928,
            0x10B78, 0x1145D, 0x12A17, 0x13532, 0x149A6, 0x15683, 0x168C9, 0x177EC, 0x18EC4,
            0x191E1, 0x1AFAB, 0x1B08E, 0x1CC1A, 0x1D33F, 0x1ED75, 0x1F250, 0x209D5, 0x216F0,
            0x228BA, 0x2379F, 0x24B0B, 0x2542E, 0x26A64, 0x27541, 0x28C69,
        ];
        for (i, &w) in table.iter().enumerate() {
            assert_eq!(version_word(i as u8 + 7), w, "version {}", i + 7);
        }
    }

    #[test]
    fn format_corrects_three_bit_errors() {
        for ecc in EcLevel::ALL {
            for mask in 0..8 {
                let w = format_word(ecc, mask);
                for a in 0..15 {
                    for b in (a + 1)..15 {
                        for c in (b + 1)..15 {
                            let bad = w ^ (1 << a) ^ (1 << b) ^ (1 << c);
                            let (e, m, d) = decode_format(bad).unwrap();
                            assert_eq!((e, m, d), (ecc, mask, 3));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn version_corrects_three_bit_errors() {
        for version in 7..=40 {
            let w = version_word(version);
            for a in 0..18 {
                for b in (a + 1)..18 {
                    for c in (b + 1)..18 {
                        let bad = w ^ (1 << a) ^ (1 << b) ^ (1 << c);
                        assert_eq!(decode_version(bad), Some((version, 3)));
                    }
                }
            }
        }
    }

    #[test]
    fn noise_is_rejected() {
        // Most 15-bit words are more than 3 from every format word.
        let rejected = (0..=0x7FFFu16)
            .filter(|&w| decode_format(w).is_none())
            .count();
        assert!(rejected > 0);
        assert_eq!(decode_version(0), None);
    }
}
