//! Times `Scanner::scan` on 1280×720 frames like the camera loop hands over.
//!
//! cargo run --release -p qr-codec --example bench

// Each example uses only part of the synthetic camera.
#[allow(dead_code)]
#[path = "../tests/integration/support/mod.rs"]
mod support;

use qr_codec::{encode, EcLevel, Scanner};
use std::time::Instant;
use support::{render, Rng, Scene};

fn time(name: &str, frame: &[u8], scene: &Scene) {
    let mut scanner = Scanner::new();
    let ok = scanner.scan(frame, scene.width, scene.height).is_ok();
    let mut runs: Vec<f64> = (0..40)
        .map(|_| {
            let t = Instant::now();
            let _ = scanner.scan(frame, scene.width, scene.height);
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    runs.sort_by(f64::total_cmp);
    println!(
        "{name:<28} read {ok:<5}  median {:6.2} ms  max {:6.2} ms",
        runs[20], runs[39]
    );
}

fn main() {
    let scene = Scene {
        width: 1280,
        height: 720,
        module_px: 4.0,
        rotation_deg: 4.0,
        tilt: (0.1, 0.05),
        noise: 12,
        gradient: 0.15,
        clutter: true,
        ..Scene::default()
    };
    for (name, len) in [
        ("v23 (default chunk)", 793),
        ("v10", 200),
        ("v40 at 2.6 px", 2331),
    ] {
        let data = Rng(1).bytes(len);
        let symbol = encode(&data, EcLevel::M).unwrap();
        let mut s = scene.clone();
        if len == 2331 {
            s.module_px = 2.6;
            s.tilt = (0.0, 0.0);
        }
        time(name, &render(symbol.matrix(), &s), &s);
    }
    let empty = Scene {
        module_px: 0.0,
        ..scene.clone()
    };
    time(
        "nothing in view, clutter",
        &render(&qr_codec::BitMatrix::new(21), &empty),
        &empty,
    );
    time("camera noise", &Rng(5).bytes(1280 * 720), &scene);
}
