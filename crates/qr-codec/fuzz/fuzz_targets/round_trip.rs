//! Whatever is encoded decodes back to the same bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use qr_codec::{decode_matrix, encode, EcLevel};

fuzz_target!(|data: &[u8]| {
    let Some((&level, payload)) = data.split_first() else {
        return;
    };
    let ecc = EcLevel::ALL[level as usize % 4];
    if let Ok(symbol) = encode(payload, ecc) {
        let decoded = decode_matrix(symbol.matrix()).expect("our own symbol decodes");
        assert_eq!(decoded.bytes, payload);
        assert_eq!(decoded.corrected, 0);
    }
});
