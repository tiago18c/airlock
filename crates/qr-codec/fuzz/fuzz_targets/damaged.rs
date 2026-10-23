//! A valid symbol with modules flipped: reaches Reed–Solomon and the bit
//! stream parser far more often than a random grid does.
#![no_main]

use libfuzzer_sys::fuzz_target;
use qr_codec::{decode_matrix, encode_with, EcLevel};

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 {
        return;
    }
    let version = 1 + data[0] % 40;
    let ecc = EcLevel::ALL[data[1] as usize % 4];
    let Ok(symbol) = encode_with(b"fuzz", ecc, Some(version), None) else {
        return;
    };
    let mut m = symbol.matrix().clone();
    let n = m.size();
    for pair in data[2..].chunks_exact(2) {
        let i = u16::from_le_bytes([pair[0], pair[1]]) as usize % (n * n);
        let (x, y) = (i % n, i / n);
        m.set(x, y, !m.get(x, y));
    }
    let _ = decode_matrix(&m);
});
