//! Any frame: the first byte picks the width, the rest is luma.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&w, frame)) = data.split_first() else {
        return;
    };
    let width = qr_codec::MIN_SIDE + w as usize % 200;
    let height = frame.len() / width;
    if height < qr_codec::MIN_SIDE {
        return;
    }
    let _ = qr_codec::Scanner::new().scan(&frame[..width * height], width, height);
});
