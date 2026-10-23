//! Any grid: the first byte picks the version, the rest are the modules.
#![no_main]

use libfuzzer_sys::fuzz_target;
use qr_codec::{decode_matrix, BitMatrix};

fuzz_target!(|data: &[u8]| {
    let Some((&v, bits)) = data.split_first() else {
        return;
    };
    let size = 21 + 4 * (v as usize % 40);
    let modules = (0..size * size)
        .map(|i| bits.get(i / 8).is_some_and(|b| (b >> (i % 8)) & 1 == 1))
        .collect();
    let m = BitMatrix::from_modules(size, modules).unwrap();
    let _ = decode_matrix(&m);
});
