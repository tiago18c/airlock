//! Hostile input: whatever is held up to the camera must not panic, hang or
//! take unbounded time. The fuzz targets in `fuzz/` explore further; these
//! run on every `cargo test`.

use crate::support::Rng;
use qr_codec::{decode_matrix, encode, encode_with, BitMatrix, EcLevel, ScanError, Scanner};
use std::time::{Duration, Instant};

#[test]
fn random_grids_of_every_size() {
    let mut rng = Rng(0xBAD);
    for size in (21..=177).step_by(4) {
        for _ in 0..25 {
            let bits: Vec<bool> = (0..size * size).map(|_| rng.next() & 1 == 1).collect();
            let m = BitMatrix::from_modules(size, bits).unwrap();
            let _ = decode_matrix(&m);
        }
    }
}

#[test]
fn damaged_symbols_never_panic() {
    let mut rng = Rng(0xD00D);
    for version in [1u8, 2, 6, 7, 13, 22, 40] {
        for ecc in EcLevel::ALL {
            // 1-H holds 7 bytes.
            let data = rng.bytes(7);
            let s = encode_with(&data, ecc, Some(version), None).unwrap();
            for percent in [1u64, 5, 10, 20, 35, 50] {
                let mut m = s.matrix().clone();
                for y in 0..m.size() {
                    for x in 0..m.size() {
                        if rng.next() % 100 < percent {
                            m.set(x, y, !m.get(x, y));
                        }
                    }
                }
                let _ = decode_matrix(&m);
                let _ = decode_matrix(&m.transposed());
            }
        }
    }
}

/// Scans `frame` and checks it neither panics nor runs long.
fn bounded(name: &str, frame: &[u8], w: usize, h: usize) -> Result<(), ScanError> {
    let t = Instant::now();
    let r = Scanner::new().scan(frame, w, h).map(|_| ());
    // Generous for a debug or loaded CI machine; a frame normally takes a
    // few milliseconds.
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "{name} took {:?}",
        t.elapsed()
    );
    r
}

#[test]
fn pathological_frames() {
    let (w, h) = (1280, 720);
    let mut frames: Vec<(&str, Vec<u8>)> = vec![
        ("black", vec![0; w * h]),
        ("white", vec![255; w * h]),
        (
            "1px checkerboard",
            (0..w * h)
                .map(|i| if (i % w + i / w) % 2 == 0 { 0 } else { 255 })
                .collect(),
        ),
        ("noise", Rng(3).bytes(w * h)),
    ];
    // Vertical stripes in 1:1:3:1:1, so every row is full of finder-like
    // runs and every column is flat.
    let stripes: Vec<u8> = (0..w * h)
        .map(|i| {
            let x = (i % w) / 3 % 7;
            if matches!(x, 1 | 5) {
                255
            } else {
                0
            }
        })
        .collect();
    frames.push(("finder stripes", stripes));
    // A wall of finder patterns: hundreds of candidates, all real.
    let tile: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) / 3 % 9, (i / w) / 3 % 9);
            let d = (x as isize - 4).abs().max((y as isize - 4).abs());
            if d == 2 || d == 4 {
                255
            } else {
                0
            }
        })
        .collect();
    frames.push(("finder tiles", tile));
    for (name, frame) in &frames {
        let r = bounded(name, frame, w, h);
        assert!(r.is_err(), "{name} decoded");
    }
}

#[test]
fn many_symbols_in_one_frame() {
    // A grid of small symbols: real finders everywhere, and a decode is
    // fine as long as it is one of them.
    let (w, h) = (1280, 720);
    let mut frame = vec![230u8; w * h];
    let mut payloads = Vec::new();
    for (k, (ox, oy)) in (0..6)
        .flat_map(|i| (0..3).map(move |j| (40 + i * 200, 30 + j * 230)))
        .enumerate()
    {
        let data = format!("symbol {k}").into_bytes();
        let s = encode(&data, EcLevel::M).unwrap();
        let px = 6;
        for y in 0..s.size() * px {
            for x in 0..s.size() * px {
                if s.matrix().get(x / px, y / px) {
                    frame[(oy + y) * w + ox + x] = 20;
                }
            }
        }
        payloads.push(data);
    }
    let t = Instant::now();
    let d = Scanner::new()
        .scan(&frame, w, h)
        .expect("one of them reads");
    assert!(payloads.contains(&d.bytes));
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[test]
fn random_small_frames() {
    let mut rng = Rng(0x5A11);
    let mut scanner = Scanner::new();
    for _ in 0..300 {
        let w = 21 + (rng.next() % 100) as usize;
        let h = 21 + (rng.next() % 100) as usize;
        let frame = rng.bytes(w * h);
        let _ = scanner.scan(&frame, w, h);
        let rgba = rng.bytes(w * h * 4);
        let _ = scanner.scan_rgba(&rgba, w, h);
    }
}
