//! The `qrcode` crate as an independent reference. Our decoder must read
//! its symbols, and our encoder must draw the same modules for the same
//! version, level and mask.

use qr_codec::{decode_matrix, encode_with, BitMatrix, EcLevel};
use qrcode::bits::Bits;
use qrcode::canvas::{Canvas, MaskPattern};
use qrcode::types::Color;
use qrcode::{QrCode, Version};

const MASKS: [MaskPattern; 8] = [
    MaskPattern::Checkerboard,
    MaskPattern::HorizontalLines,
    MaskPattern::VerticalLines,
    MaskPattern::DiagonalLines,
    MaskPattern::LargeCheckerboard,
    MaskPattern::Fields,
    MaskPattern::Diamonds,
    MaskPattern::Meadow,
];

fn their_level(ecc: EcLevel) -> qrcode::EcLevel {
    match ecc {
        EcLevel::L => qrcode::EcLevel::L,
        EcLevel::M => qrcode::EcLevel::M,
        EcLevel::Q => qrcode::EcLevel::Q,
        EcLevel::H => qrcode::EcLevel::H,
    }
}

fn to_matrix(width: usize, colors: Vec<Color>) -> BitMatrix {
    BitMatrix::from_modules(
        width,
        colors.into_iter().map(|c| c == Color::Dark).collect(),
    )
    .unwrap()
}

/// A byte-mode symbol from the reference, with the mask fixed.
fn reference(data: &[u8], version: u8, ecc: EcLevel, mask: u8) -> Option<BitMatrix> {
    let v = Version::Normal(version as i16);
    let level = their_level(ecc);
    let mut bits = Bits::new(v);
    bits.push_byte_data(data).ok()?;
    bits.push_terminator(level).ok()?;
    let (data, ec) = qrcode::ec::construct_codewords(&bits.into_bytes(), v, level).ok()?;
    let mut canvas = Canvas::new(v, level);
    canvas.draw_all_functional_patterns();
    canvas.draw_data(&data, &ec);
    canvas.apply_mask(MASKS[mask as usize]);
    Some(to_matrix(17 + 4 * version as usize, canvas.into_colors()))
}

/// The most bytes that fit, found by asking the reference.
fn capacity(version: u8, ecc: EcLevel) -> usize {
    let v = Version::Normal(version as i16);
    let mut len = 3000;
    loop {
        let mut bits = Bits::new(v);
        if bits.push_byte_data(&vec![0; len]).is_ok()
            && bits.push_terminator(their_level(ecc)).is_ok()
        {
            return len;
        }
        len -= 1;
    }
}

struct Rng(u64);
impl Rng {
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n)
            .map(|_| {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0 as u8
            })
            .collect()
    }
}

#[test]
fn every_version_level_and_mask_matches_the_reference() {
    let mut rng = Rng(0xD1CE);
    for version in 1..=40u8 {
        for ecc in EcLevel::ALL {
            let full = capacity(version, ecc);
            for mask in 0..8u8 {
                // Full symbols on even masks, partly padded ones on odd.
                let len = if mask % 2 == 0 { full } else { full / 3 + 1 };
                let data = rng.bytes(len);
                let theirs = reference(&data, version, ecc, mask).unwrap();

                let decoded = decode_matrix(&theirs)
                    .unwrap_or_else(|e| panic!("{version}-{ecc:?} mask {mask}: {e:?}"));
                assert_eq!(decoded.bytes, data, "{version}-{ecc:?} mask {mask}");
                assert_eq!(
                    (decoded.version, decoded.ecc, decoded.mask),
                    (version, ecc, mask)
                );

                let ours = encode_with(&data, ecc, Some(version), Some(mask)).unwrap();
                assert!(
                    ours.matrix() == &theirs,
                    "{version}-{ecc:?} mask {mask}: encoders differ\nours:\n{:?}\ntheirs:\n{:?}",
                    ours.matrix(),
                    theirs
                );
            }
        }
    }
}

#[test]
fn reads_numeric_and_alphanumeric_segments() {
    // The reference splits text into its most compact segments.
    for text in [
        "0123456789012345678901234567890123456789",
        "HELLO WORLD",
        "SOLANA:9WZDXWBBMKG8ZTBNMQPXVZ3UQMPPHBV1VNWXVBYCLCTZ?AMOUNT=1.5",
        "mixed 12345678901234567890 CASE and lower",
        "%$*+-./: ",
    ] {
        for ecc in EcLevel::ALL {
            let code = QrCode::with_error_correction_level(text, their_level(ecc)).unwrap();
            let m = to_matrix(code.width(), code.to_colors());
            assert_eq!(
                decode_matrix(&m).unwrap().bytes,
                text.as_bytes(),
                "{text:?} at {ecc:?}"
            );
        }
    }
}

#[test]
fn our_mask_choice_reads_back() {
    let mut rng = Rng(7);
    for len in [1, 17, 100, 793, 2331] {
        let data = rng.bytes(len);
        let s = qr_codec::encode(&data, EcLevel::M).unwrap();
        assert_eq!(decode_matrix(s.matrix()).unwrap().bytes, data);
    }
}
