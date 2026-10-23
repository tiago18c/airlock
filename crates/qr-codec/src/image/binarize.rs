//! Luma to dark/light, with a threshold that follows local lighting.
//!
//! The frame is cut into 8×8 blocks. Each block's black point is its mean,
//! or, when the block is nearly flat, an estimate taken from its neighbours,
//! so flat paper or screen does not turn into noise. A pixel is dark when it
//! is at or below the mean black point of the 5×5 blocks around its own.
//! This is the hybrid binarizer ZXing and jsQR use, after a light
//! [1 2 1] smoothing that takes most of the sensor noise out first.

const BLOCK: usize = 8;
const MIN_RANGE: u8 = 24;

/// A binary image; outside its bounds is light.
pub(crate) struct Binary {
    pub width: usize,
    pub height: usize,
    bits: Vec<bool>,
}

impl Binary {
    pub(crate) fn new() -> Self {
        Binary {
            width: 0,
            height: 0,
            bits: Vec::new(),
        }
    }

    #[inline]
    pub(crate) fn dark(&self, x: isize, y: isize) -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < self.width
            && (y as usize) < self.height
            && self.bits[y as usize * self.width + x as usize]
    }

    #[inline]
    pub(crate) fn in_bounds(&self, x: isize, y: isize) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height
    }

    /// Fills `self` from `luma` (`width * height` bytes, row-major),
    /// already smoothed by [`smooth_into`]. `black` is scratch space kept
    /// between frames.
    pub(crate) fn threshold(
        &mut self,
        luma: &[u8],
        width: usize,
        height: usize,
        black: &mut Vec<u8>,
    ) {
        self.width = width;
        self.height = height;
        self.bits.clear();
        self.bits.resize(width * height, false);

        let bw = width.div_ceil(BLOCK);
        let bh = height.div_ceil(BLOCK);
        black.clear();
        black.resize(bw * bh, 0);

        for by in 0..bh {
            for bx in 0..bw {
                let (x0, y0) = (bx * BLOCK, by * BLOCK);
                let (x1, y1) = ((x0 + BLOCK).min(width), (y0 + BLOCK).min(height));
                let (mut sum, mut min, mut max) = (0u32, 255u8, 0u8);
                for y in y0..y1 {
                    for &l in &luma[y * width + x0..y * width + x1] {
                        sum += l as u32;
                        min = min.min(l);
                        max = max.max(l);
                    }
                }
                let count = ((x1 - x0) * (y1 - y0)) as u32;
                let mut point = sum / count;
                if max - min <= MIN_RANGE {
                    // Flat: assume background, half the minimum, unless the
                    // neighbours already found it darker than that.
                    point = min as u32 / 2;
                    if bx > 0 && by > 0 {
                        let up = black[(by - 1) * bw + bx] as u32;
                        let left = black[by * bw + bx - 1] as u32;
                        let diag = black[(by - 1) * bw + bx - 1] as u32;
                        let neighbours = (up + 2 * left + diag) / 4;
                        if (min as u32) < neighbours {
                            point = neighbours;
                        }
                    }
                }
                black[by * bw + bx] = point as u8;
            }
        }

        for by in 0..bh {
            let top = by.saturating_sub(2).min(bh.saturating_sub(5));
            let bottom = (top + 5).min(bh);
            for bx in 0..bw {
                let left = bx.saturating_sub(2).min(bw.saturating_sub(5));
                let right = (left + 5).min(bw);
                let mut sum = 0u32;
                for row in black[top * bw..bottom * bw].chunks_exact(bw) {
                    sum += row[left..right].iter().map(|&b| b as u32).sum::<u32>();
                }
                let t = (sum / ((bottom - top) * (right - left)) as u32) as u8;

                let (x0, y0) = (bx * BLOCK, by * BLOCK);
                let (x1, y1) = ((x0 + BLOCK).min(width), (y0 + BLOCK).min(height));
                for y in y0..y1 {
                    let row = y * width;
                    for x in x0..x1 {
                        self.bits[row + x] = luma[row + x] <= t;
                    }
                }
            }
        }
    }
}

/// A separable [1 2 1]/4 filter in each direction, edges clamped.
/// Independent per-pixel noise drops to about 0.375 of its amplitude.
pub(crate) fn smooth_into(luma: &[u8], width: usize, height: usize, out: &mut Vec<u8>) {
    let mut rows = vec![0u16; width * height];
    for y in 0..height {
        let row = &luma[y * width..(y + 1) * width];
        for x in 0..width {
            let l = row[x.saturating_sub(1)] as u16;
            let r = row[(x + 1).min(width - 1)] as u16;
            rows[y * width + x] = l + 2 * row[x] as u16 + r;
        }
    }
    out.clear();
    out.resize(width * height, 0);
    for y in 0..height {
        let (up, down) = (y.saturating_sub(1), (y + 1).min(height - 1));
        for x in 0..width {
            let v = rows[up * width + x] + 2 * rows[y * width + x] + rows[down * width + x];
            out[y * width + x] = ((v + 8) / 16) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_gradient_lit_checkerboard() {
        // 64×64, squares of 8, under a left-to-right lighting ramp.
        let (w, h) = (64, 64);
        let luma: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let light = 120 + x as u32 * 2;
                let dark = (x / 8 + y / 8) % 2 == 0;
                if dark {
                    (light / 4) as u8
                } else {
                    light as u8
                }
            })
            .collect();
        let mut b = Binary::new();
        b.threshold(&luma, w, h, &mut Vec::new());
        for y in 0..h {
            for x in 0..w {
                assert_eq!(
                    b.dark(x as isize, y as isize),
                    (x / 8 + y / 8) % 2 == 0,
                    "({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn flat_frame_is_all_light() {
        let mut b = Binary::new();
        b.threshold(&vec![180; 40 * 30], 40, 30, &mut Vec::new());
        assert!((0..30).all(|y| (0..40).all(|x| !b.dark(x, y))));
        assert!(!b.dark(-1, 0) && !b.dark(40, 0));
    }
}
