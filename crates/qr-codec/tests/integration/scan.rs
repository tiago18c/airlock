//! The scanner on synthetic camera frames.

use crate::support::{render, sweep, Rng, Scene};
use qr_codec::{encode, EcLevel, ScanError, Scanner};

/// Encodes `len` random bytes at M, renders them with `scene`, scans.
fn scan_ok(len: usize, scene: &Scene) -> Result<(), String> {
    let data = Rng(len as u64 + scene.seed).bytes(len);
    let symbol = encode(&data, EcLevel::M).unwrap();
    let frame = render(symbol.matrix(), scene);
    match Scanner::new().scan(&frame, scene.width, scene.height) {
        Ok(d) if d.bytes == data => Ok(()),
        Ok(_) => Err(format!("v{} wrong bytes", symbol.version())),
        Err(e) => Err(format!("v{} {e:?}", symbol.version())),
    }
}

/// Runs every case and fails listing those that did not read, if more than
/// `allowed` did not.
fn expect(name: &str, cases: Vec<(String, usize, Scene)>, allowed: usize) {
    let total = cases.len();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = total.div_ceil(threads).max(1);
    let failed: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .filter_map(|(label, len, scene)| {
                            scan_ok(*len, scene).err().map(|e| format!("{label}: {e}"))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    println!("{name}: {}/{total} read", total - failed.len());
    assert!(
        failed.len() <= allowed,
        "{name}: {} of {total} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

/// The most bytes version `v` holds at M, so `encode` picks exactly `v`.
fn len_for(v: u8) -> usize {
    (1..=2331)
        .rev()
        .find(|&n| qr_codec::encode_with(&vec![0; n], EcLevel::M, Some(v), Some(0)).is_ok())
        .unwrap()
}

#[test]
fn clean_frames_of_every_version() {
    let cases = (1..=40u8)
        .map(|v| {
            let n = 17.0 + 4.0 * v as f64;
            let scene = Scene {
                width: 960,
                height: 960,
                module_px: (880.0 / (n + 8.0)).min(6.0),
                ..Scene::default()
            };
            (format!("v{v}"), len_for(v), scene)
        })
        .collect();
    expect("clean", cases, 0);
}

#[test]
fn rotation() {
    let mut cases = Vec::new();
    for (v, px) in [(2u8, 5.0), (7, 4.0), (23, 3.5)] {
        for deg in (0..360).step_by(15) {
            let scene = Scene {
                width: 800,
                height: 800,
                module_px: px,
                rotation_deg: deg as f64 + 0.7,
                seed: deg as u64,
                ..Scene::default()
            };
            cases.push((format!("v{v} {deg}°"), len_for(v), scene));
        }
    }
    expect("rotation", cases, 0);
}

#[test]
fn perspective() {
    let mut cases = Vec::new();
    for (v, px) in [(4u8, 5.0), (10, 4.5), (23, 4.0)] {
        for (tx, ty) in [(0.1, 0.0), (0.0, 0.2), (0.2, 0.1), (0.3, 0.0), (0.25, 0.25)] {
            for deg in [0.0, 30.0, 200.0] {
                let scene = Scene {
                    width: 900,
                    height: 900,
                    module_px: px,
                    tilt: (tx, ty),
                    rotation_deg: deg,
                    ..Scene::default()
                };
                cases.push((format!("v{v} tilt {tx},{ty} {deg}°"), len_for(v), scene));
            }
        }
    }
    expect("perspective", cases, 0);
}

#[test]
fn camera_noise_blur_and_light() {
    let mut cases = Vec::new();
    for (v, px) in [(5u8, 5.0), (15, 4.0), (23, 4.0)] {
        for (blur, noise, gradient, dark, light) in [
            (1, 0, 0.0, 30, 220),
            (0, 30, 0.0, 30, 220),
            (1, 20, 0.4, 30, 220),
            (0, 10, 0.0, 90, 160),
            (2, 10, 0.2, 40, 230),
        ] {
            let scene = Scene {
                width: 900,
                height: 900,
                module_px: px,
                blur,
                noise,
                gradient,
                dark,
                light,
                rotation_deg: 8.0,
                clutter: true,
                seed: (blur * 100 + noise as usize) as u64,
                ..Scene::default()
            };
            cases.push((
                format!("v{v} blur {blur} noise {noise} grad {gradient} {dark}/{light}"),
                len_for(v),
                scene,
            ));
        }
    }
    expect("camera", cases, 0);
}

#[test]
fn inverted_and_mirrored() {
    let mut cases = Vec::new();
    for v in [3u8, 12] {
        for (invert, mirror) in [(true, false), (false, true), (true, true)] {
            let scene = Scene {
                module_px: 4.0,
                invert,
                mirror,
                rotation_deg: 20.0,
                ..Scene::default()
            };
            cases.push((
                format!("v{v} invert {invert} mirror {mirror}"),
                len_for(v),
                scene,
            ));
        }
    }
    expect("inverted/mirrored", cases, 0);
}

#[test]
fn lens_distortion_at_high_versions() {
    let mut cases = Vec::new();
    for v in [23u8, 30, 40] {
        for barrel in [0.05, 0.1, 0.15] {
            let n = 17.0 + 4.0 * v as f64;
            // Barrel distortion magnifies the corners; leave room for it.
            let scene = Scene {
                width: 1280,
                height: 1280,
                module_px: (1000.0 / (n + 8.0)).min(6.0),
                barrel,
                rotation_deg: 5.0,
                ..Scene::default()
            };
            cases.push((format!("v{v} barrel {barrel}"), len_for(v), scene));
        }
    }
    expect("lens", cases, 0);
}

#[test]
fn the_app_default_frame_at_720p() {
    // QRS1 at the default chunk is 793 bytes: version 23 at M. The camera
    // loop hands over 1280×720; the code fills about two thirds of the
    // height.
    let mut cases = Vec::new();
    for (px, deg, tilt, blur) in [
        (4.0, 0.0, 0.0, 0),
        (4.0, 4.0, 0.1, 1),
        (3.5, 357.0, 0.15, 1),
        (3.0, 2.0, 0.05, 0),
        (2.6, 1.0, 0.0, 0),
    ] {
        let scene = Scene {
            width: 1280,
            height: 720,
            module_px: px,
            rotation_deg: deg,
            tilt: (tilt, tilt / 2.0),
            blur,
            noise: 12,
            gradient: 0.15,
            clutter: true,
            offset: (90.0, -20.0),
            ..Scene::default()
        };
        cases.push((format!("{px}px {deg}° tilt {tilt} blur {blur}"), 793, scene));
    }
    expect("720p default", cases, 0);
}

#[test]
fn randomized_sweep() {
    let cases = sweep(400, 0x5EED);
    let total = cases.len();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let failed: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .chunks(total.div_ceil(threads))
            .map(|part| {
                scope.spawn(move || {
                    let mut scanner = Scanner::new();
                    part.iter()
                        .filter_map(|c| {
                            let frame = render(
                                qr_codec::encode(&c.data, EcLevel::M).unwrap().matrix(),
                                &c.scene,
                            );
                            match scanner.scan(&frame, c.scene.width, c.scene.height) {
                                Ok(d) if d.bytes == c.data => None,
                                Ok(_) => panic!("{}: wrong bytes", c.label),
                                Err(e) => Some(format!("{}: {e:?} {:?}", c.label, c.scene)),
                            }
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    println!("sweep: {}/{total} read", total - failed.len());
    for f in &failed {
        println!("  {f}");
    }
    // 388 of 400 read when this was written; every miss is under 3 px a
    // module. The bound catches a regression, not noise: the sweep is
    // deterministic.
    assert!(
        failed.len() * 100 <= total * 4,
        "sweep: {} of {total} failed",
        failed.len()
    );
}

#[test]
fn small_in_a_720p_frame() {
    // A code filling 30 % of the frame's width, area or height, anywhere in
    // the frame but whole, with its quiet zone. Every case here is at least
    // 3 px a module; below that see the sweep.
    let w = 1280.0;
    let h = 720.0;
    let mut cases = Vec::new();
    for (len, version) in [(20, 2u8), (80, 5), (200, 10), (793, 23)] {
        for (fill, side) in [
            ("30% width", 0.3 * w),
            ("30% area", (0.3f64 * w * h).sqrt()),
            ("30% height", 0.3 * h),
        ] {
            let n = 17.0 + 4.0 * version as f64 + 8.0;
            let module_px = side / n;
            if module_px < 3.0 {
                continue;
            }
            for seed in 0..6u64 {
                let mut rng = Rng(seed * 7 + 3);
                // Room for 4° of rotation and the tilt, then anywhere.
                let (room_x, room_y) = ((w - side * 1.15) / 2.0, (h - side * 1.15) / 2.0);
                let scene = Scene {
                    width: w as usize,
                    height: h as usize,
                    module_px,
                    rotation_deg: rng.unit() * 8.0 - 4.0,
                    tilt: (rng.unit() * 0.08, rng.unit() * 0.05),
                    blur: 1,
                    noise: 8,
                    gradient: 0.2,
                    clutter: true,
                    offset: (
                        (rng.unit() * 2.0 - 1.0) * room_x,
                        (rng.unit() * 2.0 - 1.0) * room_y,
                    ),
                    seed,
                    ..Scene::default()
                };
                cases.push((
                    format!("v{version} {fill} #{seed} {module_px:.1}px"),
                    len,
                    scene,
                ));
            }
        }
    }
    expect("small in frame", cases, 0);
}

#[test]
fn nothing_in_view() {
    let mut scanner = Scanner::new();
    assert_eq!(
        scanner.scan(&vec![200; 640 * 480], 640, 480),
        Err(ScanError::NotFound)
    );
    let noise = Rng(9).bytes(640 * 480);
    assert_eq!(scanner.scan(&noise, 640, 480), Err(ScanError::NotFound));
    let mut clutter = Scene {
        clutter: true,
        ..Scene::default()
    };
    clutter.module_px = 0.0;
    let frame = render(&qr_codec::BitMatrix::new(21), &clutter);
    assert!(scanner.scan(&frame, 640, 480).is_err());
}

#[test]
fn bad_input() {
    let mut s = Scanner::new();
    assert_eq!(s.scan(&[0; 10], 5, 2), Err(ScanError::BadInput));
    assert_eq!(
        s.scan(&vec![0; 100 * 100], 100, 99),
        Err(ScanError::BadInput)
    );
    assert_eq!(
        s.scan(&vec![0; 2000 * 30], 2000, 30),
        Err(ScanError::BadInput)
    );
    assert_eq!(
        s.scan_rgba(&vec![0; 100 * 100], 100, 100),
        Err(ScanError::BadInput)
    );
}

#[test]
fn rgba_input() {
    let data = b"rgba frame".to_vec();
    let symbol = encode(&data, EcLevel::M).unwrap();
    let scene = Scene::default();
    let luma = render(symbol.matrix(), &scene);
    let rgba: Vec<u8> = luma.iter().flat_map(|&l| [l, l, l, 255]).collect();
    let d = Scanner::new()
        .scan_rgba(&rgba, scene.width, scene.height)
        .unwrap();
    assert_eq!(d.bytes, data);
}
