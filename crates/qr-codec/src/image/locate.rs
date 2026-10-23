//! From three finder patterns to a grid of modules.
//!
//! 1. Rank triples of finders by how close they are to the corners of a
//!    square, and label each triple top-left, top-right, bottom-left.
//! 2. Model the perspective from the finders alone (`Model`): their
//!    centres, and how their widths shrink along each edge. Estimate the
//!    version from it, and read the version information when there is one.
//! 3. Find the bottom-right alignment pattern where the model predicts it,
//!    and map module space onto the image with a homography through the
//!    finder centres and that pattern.
//! 4. From version 7, find every alignment pattern and use one homography
//!    per cell between them, which follows a curved or lens-distorted
//!    picture where a single homography drifts.
//! 5. Sample each module's centre.

use super::binarize::Binary;
use super::finder::Finder;
use super::transform::{Homography, Point};
use crate::bch::decode_version;
use crate::matrix::BitMatrix;
use crate::tables::{alignment_positions, size, MAX_VERSION, MIN_VERSION};

/// A triple of finders, labelled.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Corners {
    pub tl: Finder,
    pub tr: Finder,
    pub bl: Finder,
}

/// The `max` best triples among `finders`, best first.
pub(crate) fn triples(finders: &[Finder], max: usize) -> Vec<Corners> {
    let mut scored: Vec<(f64, Corners)> = Vec::new();
    let n = finders.len();
    // A real finder is confirmed by every row through its 3-module core;
    // a look-alike in the data by one or two.
    let most = finders.iter().map(|f| f.count).max().unwrap_or(1) as f64;
    let weak = |f: &Finder| 1.0 - (f.count as f64 / most).min(1.0);
    for i in 0..n {
        for j in (i + 1)..n {
            for k in (j + 1)..n {
                let three = [finders[i], finders[j], finders[k]];
                if let Some((s, c)) = score(three) {
                    scored.push((s + three.iter().map(weak).sum::<f64>(), c));
                }
            }
        }
    }
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.into_iter().take(max).map(|(_, c)| c).collect()
}

fn score(f: [Finder; 3]) -> Option<(f64, Corners)> {
    let modules = f.map(|x| x.module);
    let lo = modules.iter().copied().fold(f64::MAX, f64::min);
    let hi = modules.iter().copied().fold(0.0, f64::max);
    if lo <= 0.0 || hi / lo > 2.0 {
        return None;
    }
    // The corner is opposite the longest side.
    let d = |a: usize, b: usize| f[a].center.distance(f[b].center);
    let sides = [(d(1, 2), 0), (d(0, 2), 1), (d(0, 1), 2)];
    let &(hyp, corner) = sides.iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
    let others: Vec<usize> = (0..3).filter(|&x| x != corner).collect();
    let (a, c) = (others[0], others[1]);
    let (leg_a, leg_c) = (d(corner, a), d(corner, c));
    let (short, long) = (leg_a.min(leg_c), leg_a.max(leg_c));
    if short <= 0.0 || long / short > 2.0 {
        return None;
    }
    // A symbol is 21 to 177 modules across: finder centres 14 to 170 apart.
    let span = short / ((lo + hi) / 2.0);
    if !(10.0..=200.0).contains(&span) {
        return None;
    }
    let pythagoras = (hyp * hyp - (leg_a * leg_a + leg_c * leg_c)).abs() / (hyp * hyp);
    let s = (long / short - 1.0) + 2.0 * pythagoras + (hi / lo - 1.0);

    let (b, mut bl, mut tr) = (f[corner], f[a], f[c]);
    let cross = (tr.center.x - b.center.x) * (bl.center.y - b.center.y)
        - (tr.center.y - b.center.y) * (bl.center.x - b.center.x);
    if cross < 0.0 {
        core::mem::swap(&mut bl, &mut tr);
    }
    Some((s, Corners { tl: b, tr, bl }))
}

/// What the three finders say about the symbol before its version is
/// known: a homography anchored at the top-left finder, in fractions of
/// the finder spacing.
///
/// Three centres fix an affine map. The two projective terms come from how
/// much narrower the far finders are than the top-left one: along a line, a
/// projective map scales by 1/(1 + g·t)², so the ratio of the finder widths
/// at t = 0 and t = 1 gives g.
pub(crate) struct Model {
    tl: Point,
    a: Point,
    b: Point,
    g: f64,
    h: f64,
    /// One module, as a fraction of the finder spacing, along each axis.
    du: f64,
    dv: f64,
}

impl Model {
    pub(crate) fn new(img: &Binary, c: &Corners) -> Option<Model> {
        let width = |from: &Finder, to: &Finder| {
            both_ways(img, from.center, to.center).unwrap_or(from.module * 7.0)
        };
        let (x0, x1) = (width(&c.tl, &c.tr), width(&c.tr, &c.tl));
        let (y0, y1) = (width(&c.tl, &c.bl), width(&c.bl, &c.tl));
        if x0.min(x1).min(y0).min(y1) <= 0.0 {
            return None;
        }
        let g = ((x0 / x1).sqrt() - 1.0).clamp(-0.5, 1.0);
        let h = ((y0 / y1).sqrt() - 1.0).clamp(-0.5, 1.0);
        let a = c.tr.center.sub(c.tl.center).scale(1.0 + g);
        let b = c.bl.center.sub(c.tl.center).scale(1.0 + h);
        let origin = Point::new(0.0, 0.0);
        let (la, lb) = (a.distance(origin), b.distance(origin));
        if la <= 0.0 || lb <= 0.0 {
            return None;
        }
        Some(Model {
            tl: c.tl.center,
            a,
            b,
            g,
            h,
            du: x0 / 7.0 / la,
            dv: y0 / 7.0 / lb,
        })
    }

    /// The image point at fractions `(fu, fv)` of the way from the top-left
    /// finder to the top-right and bottom-left ones.
    fn at(&self, fu: f64, fv: f64) -> Point {
        let w = 1.0 + self.g * fu + self.h * fv;
        self.tl
            .add(self.a.scale(fu).add(self.b.scale(fv)).scale(1.0 / w))
    }

    /// Module `(x, y)` of a symbol `n` modules wide.
    fn module(&self, n: f64, x: f64, y: f64) -> Point {
        self.at((x - 3.5) / (n - 7.0), (y - 3.5) / (n - 7.0))
    }

    /// The share of the three finder corners (finder and separator, 8×8
    /// each) that have the colour they must, sampled through the model at
    /// `version`. Near 1 for a symbol even at a neighbouring version; near
    /// one half for finders that are not one symbol's.
    pub(crate) fn corners_match(&self, img: &Binary, version: u8) -> f64 {
        let n = size(version);
        let s = n as f64;
        let mut right = 0;
        for (ox, oy, fx, fy) in [(0, 0, 3, 3), (n - 8, 0, 4, 3), (0, n - 8, 3, 4)] {
            for y in 0..8 {
                for x in 0..8 {
                    let d = (x as isize - fx).abs().max((y as isize - fy).abs());
                    let p = self.module(s, (ox + x) as f64 + 0.5, (oy + y) as f64 + 0.5);
                    if sample(img, p) == (d != 2 && d != 4) {
                        right += 1;
                    }
                }
            }
        }
        right as f64 / 192.0
    }

    /// Versions worth sampling at, most likely first.
    pub(crate) fn versions(&self, img: &Binary) -> Vec<u8> {
        let across = (1.0 / self.du + 1.0 / self.dv) / 2.0 + 7.0;
        let estimate = ((across - 17.0) / 4.0).round();
        if !estimate.is_finite() {
            return Vec::new();
        }
        let estimate = estimate.clamp(MIN_VERSION as f64, MAX_VERSION as f64) as u8;
        let mut out = Vec::with_capacity(4);
        // Version information exists from 7; at an estimate of 6 the symbol
        // may be either. Data next to a finder can land within 3 bits of a
        // version word, so a reading far from the estimate is ignored.
        if estimate >= 6 {
            if let Some(v) = self.read_version(img).filter(|v| v.abs_diff(estimate) <= 3) {
                out.push(v);
            }
        }
        for v in [
            estimate,
            estimate.saturating_add(1),
            estimate.saturating_sub(1),
        ] {
            if (MIN_VERSION..=MAX_VERSION).contains(&v) && !out.contains(&v) {
                out.push(v);
            }
        }
        out
    }

    /// Reads the version blocks beside the top-right and bottom-left
    /// finders. Their offsets from those finders are the same in every
    /// version, so the version does not have to be known first.
    fn read_version(&self, img: &Binary) -> Option<u8> {
        let mut top_right = 0u32;
        let mut bottom_left = 0u32;
        for i in 0..18 {
            let (near, far) = ((i % 3) as f64 - 7.0, (i / 3) as f64 - 3.0);
            if sample(img, self.at(1.0 + near * self.du, far * self.dv)) {
                top_right |= 1 << i;
            }
            if sample(img, self.at(far * self.du, 1.0 + near * self.dv)) {
                bottom_left |= 1 << i;
            }
        }
        [top_right, bottom_left]
            .into_iter()
            .filter_map(decode_version)
            .min_by_key(|&(_, d)| d)
            .map(|(v, _)| v)
    }
}

/// The width of the finder at `from` along the line to `to`, through its
/// centre: from one outer edge to the other.
fn both_ways(img: &Binary, from: Point, to: Point) -> Option<f64> {
    let forward = dark_light_dark(img, from, to)?;
    let away = from.sub(to.sub(from));
    let mut t = 1.0f64;
    let (w, h) = ((img.width - 1) as f64, (img.height - 1) as f64);
    if away.x < 0.0 {
        t = t.min(from.x / (from.x - away.x));
    } else if away.x > w {
        t = t.min((w - from.x) / (away.x - from.x));
    }
    if away.y < 0.0 {
        t = t.min(from.y / (from.y - away.y));
    } else if away.y > h {
        t = t.min((h - from.y) / (away.y - from.y));
    }
    let back = from.add(away.sub(from).scale(t));
    let backward = dark_light_dark(img, from, back).unwrap_or(forward);
    Some(forward + backward - 1.0)
}

/// From the centre of a finder: through its dark core, its light ring and
/// its dark ring, to the light beyond. The distance to that light pixel.
fn dark_light_dark(img: &Binary, from: Point, to: Point) -> Option<f64> {
    let d = to.sub(from);
    let steps = d.x.abs().max(d.y.abs()).ceil();
    if !(1.0..=8192.0).contains(&steps) {
        return None;
    }
    let mut state = 0;
    for k in 0..=steps as usize {
        let p = from.add(d.scale(k as f64 / steps));
        let dark = sample(img, p);
        if (state == 1) == dark {
            if state == 2 {
                return Some(p.distance(from));
            }
            state += 1;
        }
    }
    None
}

#[inline]
fn sample(img: &Binary, p: Point) -> bool {
    img.dark(p.x.floor() as isize, p.y.floor() as isize)
}

/// One module's step along x and along y at module `(x, y)` under `map`.
fn local_frame(map: impl Fn(f64, f64) -> Point, x: f64, y: f64) -> (Point, Point) {
    (
        map(x + 0.5, y).sub(map(x - 0.5, y)),
        map(x, y + 0.5).sub(map(x, y - 0.5)),
    )
}

/// Samples a grid of `version`'s size. `refine` uses every alignment
/// pattern from version 7; without it one homography covers the symbol.
pub(crate) fn sample_grid(
    img: &Binary,
    c: &Corners,
    model: &Model,
    version: u8,
    refine: bool,
) -> Option<BitMatrix> {
    let n = size(version);
    let s = n as f64;
    let (tl, tr, bl) = (c.tl.center, c.tr.center, c.bl.center);
    let predict = |x: f64, y: f64| model.module(s, x, y);

    let (br_module, br) = if version == 1 {
        let m = Point::new(s - 3.5, s - 3.5);
        (m, predict(m.x, m.y))
    } else {
        let m = Point::new(s - 6.5, s - 6.5);
        let estimate = predict(m.x, m.y);
        let (ex, ey) = local_frame(predict, m.x, m.y);
        let found = [4.0, 8.0]
            .into_iter()
            .find_map(|r| find_alignment(img, estimate, ex, ey, r));
        (m, found.unwrap_or(estimate))
    };
    let modules = [
        Point::new(3.5, 3.5),
        Point::new(s - 3.5, 3.5),
        br_module,
        Point::new(3.5, s - 3.5),
    ];
    let global = Homography::quad_to_quad(modules, [tl, tr, br, bl])?;

    let mut m = BitMatrix::new(n);
    if version < 7 || !refine {
        for y in 0..n {
            for x in 0..n {
                m.set(
                    x,
                    y,
                    sample(img, global.map(x as f64 + 0.5, y as f64 + 0.5)),
                );
            }
        }
        return Some(m);
    }

    // Nodes: every alignment centre, with the finder centres standing in
    // for the three corners that have none. They are found from the
    // top-left outward, each predicted by the global map plus the drift its
    // found neighbours showed, so the search follows a lens or a curve.
    let pos = alignment_positions(version);
    let k = pos.len();
    let mut module_at = vec![Point::new(0.0, 0.0); k * k];
    for j in 0..k {
        for i in 0..k {
            module_at[j * k + i] = match (i, j) {
                (0, 0) => Point::new(3.5, 3.5),
                (i, 0) if i == k - 1 => Point::new(s - 3.5, 3.5),
                (0, j) if j == k - 1 => Point::new(3.5, s - 3.5),
                (i, j) if i == k - 1 && j == k - 1 => br_module,
                _ => Point::new(pos[i] as f64 + 0.5, pos[j] as f64 + 0.5),
            };
        }
    }
    let mut image_at: Vec<Option<Point>> = vec![None; k * k];
    image_at[0] = Some(tl);
    image_at[k - 1] = Some(tr);
    image_at[(k - 1) * k] = Some(bl);
    image_at[k * k - 1] = Some(br);
    let mut order: Vec<(usize, usize)> = (0..k).flat_map(|j| (0..k).map(move |i| (i, j))).collect();
    order.sort_by_key(|&(i, j)| i + j);
    for (i, j) in order {
        if image_at[j * k + i].is_some() {
            continue;
        }
        let mp = module_at[j * k + i];
        let mut drift = Point::new(0.0, 0.0);
        let mut known = 0.0;
        for (di, dj) in [(-1isize, 0isize), (0, -1), (-1, -1), (1, -1), (-1, 1)] {
            let (ni, nj) = (i as isize + di, j as isize + dj);
            if ni < 0 || nj < 0 || ni as usize >= k || nj as usize >= k {
                continue;
            }
            let q = nj as usize * k + ni as usize;
            if let Some(p) = image_at[q] {
                drift = drift.add(p.sub(global.map(module_at[q].x, module_at[q].y)));
                known += 1.0;
            }
        }
        if known > 0.0 {
            drift = drift.scale(1.0 / known);
        }
        let guess = global.map(mp.x, mp.y).add(drift);
        let (lx, ly) = local_frame(|x, y| global.map(x, y), mp.x, mp.y);
        image_at[j * k + i] = Some(find_alignment(img, guess, lx, ly, 2.5).unwrap_or(guess));
    }
    let image_at: Vec<Point> = image_at.into_iter().map(|p| p.unwrap_or(tl)).collect();

    let mut cells = Vec::with_capacity((k - 1) * (k - 1));
    for j in 0..k - 1 {
        for i in 0..k - 1 {
            let idx = [
                j * k + i,
                j * k + i + 1,
                (j + 1) * k + i + 1,
                (j + 1) * k + i,
            ];
            let h = Homography::quad_to_quad(idx.map(|q| module_at[q]), idx.map(|q| image_at[q]));
            cells.push(h.unwrap_or(global));
        }
    }
    let cell_of = |v: usize| pos[1..k - 1].iter().filter(|&&p| p as usize <= v).count();
    for y in 0..n {
        let cj = cell_of(y);
        for x in 0..n {
            let h = &cells[cj * (k - 1) + cell_of(x)];
            m.set(x, y, sample(img, h.map(x as f64 + 0.5, y as f64 + 0.5)));
        }
    }
    Some(m)
}

/// The centre of the alignment pattern nearest `estimate`, within `radius`
/// modules, by matching its 5×5 template in the local frame `ex`, `ey`.
fn find_alignment(
    img: &Binary,
    estimate: Point,
    ex: Point,
    ey: Point,
    radius: f64,
) -> Option<Point> {
    const STEP: f64 = 0.25;
    const MIN_SCORE: u32 = 23;
    let steps = (radius / STEP) as i32;
    let at = |u: f64, v: f64| estimate.add(ex.scale(u)).add(ey.scale(v));

    let mut best = 0u32;
    let mut hits: Vec<(f64, f64)> = Vec::new();
    for oy in -steps..=steps {
        for ox in -steps..=steps {
            let (u, v) = (ox as f64 * STEP, oy as f64 * STEP);
            if !sample(img, at(u, v)) {
                continue;
            }
            let mut score = 0;
            for j in -2i32..=2 {
                for i in -2i32..=2 {
                    let expected = i.abs().max(j.abs()) != 1;
                    if sample(img, at(u + i as f64, v + j as f64)) == expected {
                        score += 1;
                    }
                }
            }
            if score > best {
                best = score;
                hits.clear();
            }
            if score == best {
                hits.push((u, v));
            }
        }
    }
    if best < MIN_SCORE {
        return None;
    }
    // The best scores form a plateau about a module wide around each true
    // centre. Take the plateau nearest the estimate and its mean.
    let &(nu, nv) = hits
        .iter()
        .min_by(|a, b| a.0.hypot(a.1).total_cmp(&b.0.hypot(b.1)))?;
    let near: Vec<&(f64, f64)> = hits
        .iter()
        .filter(|h| (h.0 - nu).abs() <= 1.0 && (h.1 - nv).abs() <= 1.0)
        .collect();
    let (su, sv) = near
        .iter()
        .fold((0.0, 0.0), |acc, h| (acc.0 + h.0, acc.1 + h.1));
    Some(at(su / near.len() as f64, sv / near.len() as f64))
}
