//! Finder patterns: the three 7×7 squares whose every cross-section through
//! the centre runs dark, light, dark, light, dark in 1:1:3:1:1.
//!
//! Every row is scanned for that ratio. A hit is confirmed by a vertical
//! cross-section, re-centred by a horizontal one, and checked along a
//! diagonal. Hits close together are merged into one candidate.

use super::binarize::Binary;
use super::transform::Point;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Finder {
    pub center: Point,
    /// Mean module size of the cross-sections, in pixels.
    pub module: f64,
    /// Rows that confirmed it.
    pub count: u32,
}

/// The candidates in `img`, at most `limit` of them.
pub(crate) fn find(img: &Binary, out: &mut Vec<Finder>, limit: usize) {
    out.clear();
    for y in 0..img.height {
        let mut counts = [0usize; 5];
        let mut state = 0usize;
        for x in 0..=img.width {
            let dark = x < img.width && img.dark(x as isize, y as isize);
            if dark {
                if state % 2 == 1 {
                    state += 1;
                }
                counts[state] += 1;
            } else if state % 2 == 1 {
                counts[state] += 1;
            } else if state < 4 {
                state += 1;
                counts[state] += 1;
            } else {
                if ratio_ok(&counts, 0.5) {
                    if let Some(f) = confirm(img, &counts, x, y) {
                        add(out, f, limit);
                    }
                }
                counts = [counts[2], counts[3], counts[4], 1, 0];
                state = 3;
            }
        }
    }
}

fn ratio_ok(c: &[usize; 5], tolerance: f64) -> bool {
    let total: usize = c.iter().sum();
    if total < 7 || c.contains(&0) {
        return false;
    }
    let m = total as f64 / 7.0;
    let v = m * tolerance;
    (c[0] as f64 - m).abs() < v
        && (c[1] as f64 - m).abs() < v
        && (c[2] as f64 - 3.0 * m).abs() < 3.0 * v
        && (c[3] as f64 - m).abs() < v
        && (c[4] as f64 - m).abs() < v
}

/// `end` is one past the last dark pixel of the row's pattern.
fn confirm(img: &Binary, counts: &[usize; 5], end: usize, y: usize) -> Option<Finder> {
    let h_total: usize = counts.iter().sum();
    let row_cx = end as f64 - counts[4] as f64 - counts[3] as f64 - counts[2] as f64 / 2.0;
    let max = h_total;
    // Each cross-section is tried on its own line and on the two beside it.
    // Every row through a finder re-checks the same centre lines, so one
    // noisy line there would otherwise veto a pattern all the others show.
    const NUDGE: [f64; 3] = [0.0, -1.0, 1.0];

    let (v_counts, cy) = NUDGE.iter().find_map(|&d| {
        cross(img, Point::new(row_cx + d, y as f64 + 0.5), (0, 1), max).filter(|(c, _)| {
            ratio_ok(c, 0.5) && 5 * c.iter().sum::<usize>().abs_diff(h_total) < 2 * h_total
        })
    })?;
    let (h_counts, cx) = NUDGE
        .iter()
        .find_map(|&d| {
            cross(img, Point::new(row_cx, cy + d), (1, 0), max).filter(|(c, _)| ratio_ok(c, 0.5))
        })
        .unwrap_or((*counts, row_cx));
    NUDGE.iter().find_map(|&d| {
        cross(img, Point::new(cx + d, cy), (1, 1), max).filter(|(c, _)| ratio_ok(c, 0.75))
    })?;

    let v_total: usize = v_counts.iter().sum();
    let h_total: usize = h_counts.iter().sum();
    Some(Finder {
        center: Point::new(cx, cy),
        module: (h_total + v_total) as f64 / 14.0,
        count: 1,
    })
}

/// Runs through `at` along `(dx, dy)`: the five run lengths and the
/// centre's coordinate along the step axis (x for horizontal and diagonal,
/// y for vertical).
fn cross(
    img: &Binary,
    at: Point,
    (dx, dy): (isize, isize),
    max: usize,
) -> Option<([usize; 5], f64)> {
    let (x0, y0) = (at.x.floor() as isize, at.y.floor() as isize);
    let px = |i: isize| (x0 + i * dx, y0 + i * dy);
    let dark = |i: isize| {
        let (x, y) = px(i);
        img.dark(x, y)
    };
    let inside = |i: isize| {
        let (x, y) = px(i);
        img.in_bounds(x, y)
    };
    if !dark(0) {
        return None;
    }

    let mut c = [0usize; 5];
    let mut i = 0isize;
    while inside(i) && dark(i) && c[2] <= max {
        c[2] += 1;
        i -= 1;
    }
    while inside(i) && !dark(i) && c[1] <= max {
        c[1] += 1;
        i -= 1;
    }
    if !inside(i) || c[1] > max {
        return None;
    }
    while inside(i) && dark(i) && c[0] <= max {
        c[0] += 1;
        i -= 1;
    }
    if c[0] > max {
        return None;
    }

    let mut j = 1isize;
    while inside(j) && dark(j) && c[2] <= max {
        c[2] += 1;
        j += 1;
    }
    while inside(j) && !dark(j) && c[3] <= max {
        c[3] += 1;
        j += 1;
    }
    if !inside(j) || c[3] > max {
        return None;
    }
    while inside(j) && dark(j) && c[4] <= max {
        c[4] += 1;
        j += 1;
    }
    if c[4] > max || c[2] > max {
        return None;
    }

    // One past the last dark pixel, back to the middle of the centre run.
    let along = j as f64 - c[4] as f64 - c[3] as f64 - c[2] as f64 / 2.0;
    let base = if dy != 0 && dx == 0 { y0 } else { x0 };
    Some((c, base as f64 + along))
}

fn add(out: &mut Vec<Finder>, f: Finder, limit: usize) {
    for e in out.iter_mut() {
        let near = (e.center.x - f.center.x).abs() <= e.module
            && (e.center.y - f.center.y).abs() <= e.module;
        let diff = (e.module - f.module).abs();
        if near && (diff <= 1.0 || diff <= e.module) {
            let n = e.count as f64;
            e.center = Point::new(
                (e.center.x * n + f.center.x) / (n + 1.0),
                (e.center.y * n + f.center.y) / (n + 1.0),
            );
            e.module = (e.module * n + f.module) / (n + 1.0);
            e.count += 1;
            return;
        }
    }
    if out.len() < limit {
        out.push(f);
    } else if let Some(weak) = out.iter_mut().find(|e| e.count == 1) {
        // Full: a pattern seen on one row only gives way, so noise near the
        // top of the frame cannot crowd out a real finder lower down. A
        // real one evicted here is found again on the next row.
        *weak = f;
    }
}
