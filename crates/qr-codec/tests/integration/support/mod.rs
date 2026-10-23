//! A synthetic camera: renders a symbol into a luma frame the way a webcam
//! pointed at a screen would see it. Deterministic for a given scene.
//!
//! The geometry is solved here with its own 8×8 linear system, not the
//! crate's homography code, so the two check each other.

use qr_codec::BitMatrix;

#[derive(Clone, Debug)]
pub struct Scene {
    pub width: usize,
    pub height: usize,
    /// Pixels per module before tilt and distortion.
    pub module_px: f64,
    /// Quiet zone in modules.
    pub quiet: f64,
    pub rotation_deg: f64,
    /// Perspective: the top edge shrinks by `.0`, the left edge by `.1`.
    pub tilt: (f64, f64),
    /// Radial lens distortion; positive is barrel.
    pub barrel: f64,
    /// Box blur radius in pixels.
    pub blur: usize,
    /// Uniform noise amplitude.
    pub noise: u8,
    pub dark: u8,
    pub light: u8,
    /// Brightness falls by this fraction from left to right.
    pub gradient: f64,
    pub invert: bool,
    pub mirror: bool,
    /// Dark shapes around the symbol, like text and a screen bezel.
    pub clutter: bool,
    /// Offset of the symbol centre from the frame centre, in pixels.
    pub offset: (f64, f64),
    pub seed: u64,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            width: 640,
            height: 480,
            module_px: 4.0,
            quiet: 4.0,
            rotation_deg: 0.0,
            tilt: (0.0, 0.0),
            barrel: 0.0,
            blur: 0,
            noise: 0,
            dark: 30,
            light: 220,
            gradient: 0.0,
            invert: false,
            mirror: false,
            clutter: false,
            offset: (0.0, 0.0),
            seed: 1,
        }
    }
}

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// Solves the homography taking `from[i]` to `to[i]`.
// Gauss–Jordan elimination reads best with row and column indices.
#[allow(clippy::needless_range_loop)]
fn homography(from: [(f64, f64); 4], to: [(f64, f64); 4]) -> [f64; 9] {
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let ((x, y), (u, v)) = (from[i], to[i]);
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -x * u, -y * u, u];
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -x * v, -y * v, v];
    }
    for col in 0..8 {
        let pivot = (col..8)
            .max_by(|&p, &q| a[p][col].abs().total_cmp(&a[q][col].abs()))
            .unwrap();
        a.swap(col, pivot);
        for row in 0..8 {
            if row != col {
                let f = a[row][col] / a[col][col];
                for k in col..9 {
                    a[row][k] -= f * a[col][k];
                }
            }
        }
    }
    let mut h = [0.0; 9];
    for i in 0..8 {
        h[i] = a[i][8] / a[i][i];
    }
    h[8] = 1.0;
    h
}

fn apply(h: &[f64; 9], x: f64, y: f64) -> (f64, f64) {
    let w = h[6] * x + h[7] * y + h[8];
    (
        (h[0] * x + h[1] * y + h[2]) / w,
        (h[3] * x + h[4] * y + h[5]) / w,
    )
}

/// Renders `m` and returns the luma frame.
pub fn render(m: &BitMatrix, s: &Scene) -> Vec<u8> {
    let n = m.size() as f64;
    let (w, h) = (s.width, s.height);
    let (cx, cy) = (w as f64 / 2.0 + s.offset.0, h as f64 / 2.0 + s.offset.1);
    let half = n * s.module_px / 2.0;
    let (sin, cos) = s.rotation_deg.to_radians().sin_cos();
    let corner = |dx: f64, dy: f64| {
        // Tilt, then rotate, then place.
        let dx = dx * if dy < 0.0 { 1.0 - s.tilt.0 } else { 1.0 };
        let dy = dy * if dx < 0.0 { 1.0 - s.tilt.1 } else { 1.0 };
        (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
    };
    let image = [
        corner(-half, -half),
        corner(half, -half),
        corner(half, half),
        corner(-half, half),
    ];
    let to_module = homography(image, [(0.0, 0.0), (n, 0.0), (n, n), (0.0, n)]);

    let mut rng = Rng(s.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    // Clutter: dark bars, drawn wherever the symbol and its quiet zone
    // are not.
    let mut bars = vec![false; w * h];
    if s.clutter {
        for _ in 0..60 {
            let (bx, by) = (
                (rng.unit() * w as f64) as usize,
                (rng.unit() * h as f64) as usize,
            );
            let (bw, bh) = (
                2 + (rng.unit() * 30.0) as usize,
                2 + (rng.unit() * 8.0) as usize,
            );
            for y in by..(by + bh).min(h) {
                bars[y * w + bx..y * w + (bx + bw).min(w)].fill(true);
            }
        }
    }
    let quiet = s.quiet;
    let r_norm = ((w * w + h * h) as f64).sqrt() / 2.0;

    let mut dark = vec![0.0f64; w * h];
    const SS: usize = 3;
    for py in 0..h {
        for px in 0..w {
            let mut acc = 0.0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let mut x = px as f64 + (sx as f64 + 0.5) / SS as f64;
                    let mut y = py as f64 + (sy as f64 + 0.5) / SS as f64;
                    if s.barrel != 0.0 {
                        let (dx, dy) =
                            ((x - w as f64 / 2.0) / r_norm, (y - h as f64 / 2.0) / r_norm);
                        let k = 1.0 + s.barrel * (dx * dx + dy * dy);
                        x = w as f64 / 2.0 + dx * r_norm / k;
                        y = h as f64 / 2.0 + dy * r_norm / k;
                    }
                    let (mut u, mut v) = apply(&to_module, x, y);
                    if s.mirror {
                        core::mem::swap(&mut u, &mut v);
                    }
                    let inside = u >= 0.0 && v >= 0.0 && u < n && v < n;
                    let in_quiet = u >= -quiet && v >= -quiet && u < n + quiet && v < n + quiet;
                    let is_dark = if inside {
                        m.get(u as usize, v as usize)
                    } else if in_quiet {
                        false
                    } else {
                        bars[py * w + px]
                    };
                    if is_dark {
                        acc += 1.0;
                    }
                }
            }
            dark[py * w + px] = acc / (SS * SS) as f64;
        }
    }

    if s.blur > 0 {
        dark = box_blur(&dark, w, h, s.blur);
    }

    let mut out = vec![0u8; w * h];
    for py in 0..h {
        for px in 0..w {
            let d = dark[py * w + px];
            let mut l = s.light as f64 - (s.light as f64 - s.dark as f64) * d;
            if s.invert {
                l = s.light as f64 + s.dark as f64 - l;
            }
            l *= 1.0 - s.gradient * px as f64 / w as f64;
            if s.noise > 0 {
                l += (rng.unit() * 2.0 - 1.0) * s.noise as f64;
            }
            out[py * w + px] = l.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn box_blur(src: &[f64], w: usize, h: usize, r: usize) -> Vec<f64> {
    let mut tmp = vec![0.0; w * h];
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = src[y * w + a..=y * w + b].iter().sum::<f64>() / (b - a + 1) as f64;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
            out[y * w + x] = (a..=b).map(|yy| tmp[yy * w + x]).sum::<f64>() / (b - a + 1) as f64;
        }
    }
    out
}

/// One case of the randomized sweep: what to encode and how to film it.
pub struct Case {
    pub label: String,
    pub data: Vec<u8>,
    pub scene: Scene,
}

/// `count` random scenes mixing every distortion, at sizes a webcam sees
/// when someone holds a screen up to it. The same seed gives the same cases.
pub fn sweep(count: usize, seed: u64) -> Vec<Case> {
    let mut rng = Rng(seed | 1);
    (0..count)
        .map(|i| {
            // Mostly the sizes Airlock sends (default chunk ≈ v23), some
            // small and some up to the maximum.
            let len = match rng.next() % 4 {
                0 => 20 + (rng.next() % 200) as usize,
                1 | 2 => 793,
                _ => 800 + (rng.next() % 1531) as usize,
            };
            let data = rng.bytes(len);
            let version = qr_codec::encode(&data, qr_codec::EcLevel::M)
                .unwrap()
                .version();
            let n = 17.0 + 4.0 * version as f64 + 8.0;
            let (width, height) = (1280usize, 720usize);
            // Upright or nearly so, sometimes at any angle, but always with
            // the whole symbol and its quiet zone in frame: fill 45–85 % of
            // the height with the rotated square, never under 2.5 px per
            // module.
            let mut rotation_deg = if rng.unit() < 0.5 {
                rng.unit() * 10.0 - 5.0
            } else {
                rng.unit() * 360.0
            };
            let extent = |deg: f64| {
                let (sin, cos) = deg.to_radians().sin_cos();
                sin.abs() + cos.abs()
            };
            let mut fill = 0.45 + rng.unit() * 0.4;
            let mut module_px = height as f64 * fill / (n * extent(rotation_deg));
            if module_px < 2.5 {
                rotation_deg = rng.unit() * 4.0 - 2.0;
                fill = 0.85;
                module_px = (height as f64 * fill / (n * extent(rotation_deg))).max(2.5);
            }
            let scene = Scene {
                width,
                height,
                module_px,
                rotation_deg,
                tilt: (rng.unit() * 0.2, rng.unit() * 0.12),
                barrel: rng.unit() * 0.08,
                blur: (rng.next() % 3) as usize / 2,
                noise: (rng.next() % 25) as u8,
                dark: 20 + (rng.next() % 50) as u8,
                light: 170 + (rng.next() % 80) as u8,
                gradient: rng.unit() * 0.35,
                invert: rng.unit() < 0.05,
                mirror: rng.unit() < 0.05,
                clutter: rng.unit() < 0.7,
                offset: (
                    (rng.unit() - 0.5) * 200.0,
                    (rng.unit() - 0.5) * height as f64 * (0.9 - fill) / 2.0,
                ),
                seed: i as u64,
                ..Scene::default()
            };
            Case {
                label: format!("#{i} v{version} {module_px:.1}px"),
                data,
                scene,
            }
        })
        .collect()
}
