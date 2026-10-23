//! The module grid, the function patterns and the order data is placed in.
//!
//! Coordinates are `(x, y)`: column, then row, from the top-left corner.

use crate::tables::{alignment_positions, raw_data_modules, size};

/// A square grid of modules; `true` is dark.
#[derive(Clone, PartialEq, Eq)]
pub struct BitMatrix {
    size: usize,
    bits: Vec<bool>,
}

impl BitMatrix {
    pub fn new(size: usize) -> Self {
        BitMatrix {
            size,
            bits: vec![false; size * size],
        }
    }

    /// From row-major modules. `None` unless there are `size * size`.
    pub fn from_modules(size: usize, modules: Vec<bool>) -> Option<Self> {
        (modules.len() == size * size).then_some(BitMatrix {
            size,
            bits: modules,
        })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Outside the grid is light, like the quiet zone.
    pub fn get(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.bits[y * self.size + x]
    }

    /// Ignores coordinates outside the grid.
    pub fn set(&mut self, x: usize, y: usize, dark: bool) {
        if x < self.size && y < self.size {
            self.bits[y * self.size + x] = dark;
        }
    }

    /// The mirror image across the main diagonal: how a symbol read from
    /// behind, or with its corners mislabelled, looks.
    pub fn transposed(&self) -> Self {
        let mut t = BitMatrix::new(self.size);
        for y in 0..self.size {
            for x in 0..self.size {
                t.set(y, x, self.get(x, y));
            }
        }
        t
    }

    pub(crate) fn flip(&mut self, x: usize, y: usize) {
        if x < self.size && y < self.size {
            let i = y * self.size + x;
            self.bits[i] = !self.bits[i];
        }
    }
}

impl core::fmt::Debug for BitMatrix {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for y in 0..self.size {
            for x in 0..self.size {
                f.write_str(if self.get(x, y) { "█" } else { "·" })?;
            }
            f.write_str("\n")?;
        }
        Ok(())
    }
}

/// Where everything is in a symbol of one version.
pub(crate) struct Layout {
    pub size: usize,
    /// The colours of the fixed function modules. Format and version
    /// information are left light.
    pub base: BitMatrix,
    /// Data module positions in placement order: bit 7 of codeword 0 first.
    pub data: Vec<(u8, u8)>,
}

impl Layout {
    pub(crate) fn new(version: u8) -> Self {
        let size = size(version);
        // Modules that are not data: finders, separators, timing,
        // alignment, format and version information, and the dark module.
        let mut function = BitMatrix::new(size);
        let mut base = BitMatrix::new(size);
        let mut mark = |x: usize, y: usize, dark: bool| {
            function.set(x, y, true);
            base.set(x, y, dark);
        };

        for i in 0..size {
            mark(6, i, i % 2 == 0);
            mark(i, 6, i % 2 == 0);
        }

        // Finders with their separators: dark at Chebyshev distance 0, 1
        // and 3 from the centre, light at 2 and 4.
        for (cx, cy) in [(3, 3), (size - 4, 3), (3, size - 4)] {
            for dy in -4isize..=4 {
                for dx in -4isize..=4 {
                    let (x, y) = (cx as isize + dx, cy as isize + dy);
                    if x >= 0 && y >= 0 && (x as usize) < size && (y as usize) < size {
                        let d = dx.abs().max(dy.abs());
                        mark(x as usize, y as usize, d != 2 && d != 4);
                    }
                }
            }
        }

        let align = alignment_positions(version);
        let n = align.len();
        for i in 0..n {
            for j in 0..n {
                if (i == 0 && j == 0) || (i == 0 && j == n - 1) || (i == n - 1 && j == 0) {
                    continue;
                }
                let (cx, cy) = (align[i] as isize, align[j] as isize);
                for dy in -2isize..=2 {
                    for dx in -2isize..=2 {
                        let d = dx.abs().max(dy.abs());
                        mark((cx + dx) as usize, (cy + dy) as usize, d != 1);
                    }
                }
            }
        }

        for copy in format_positions(size) {
            for (x, y) in copy {
                mark(x, y, false);
            }
        }
        mark(8, size - 8, true);

        if version >= 7 {
            for copy in version_positions(size) {
                for (x, y) in copy {
                    mark(x, y, false);
                }
            }
        }

        let data = data_positions(&function);
        debug_assert_eq!(data.len(), raw_data_modules(version));
        Layout { size, base, data }
    }
}

/// The zigzag: two-column strips from the right, alternately upward and
/// downward, skipping the vertical timing column.
fn data_positions(function: &BitMatrix) -> Vec<(u8, u8)> {
    let size = function.size();
    let mut out = Vec::with_capacity(size * size);
    let mut right = size as isize - 1;
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        let upward = (right + 1) & 2 == 0;
        for vert in 0..size {
            let y = if upward { size - 1 - vert } else { vert };
            for j in 0..2 {
                let x = (right - j) as usize;
                if !function.get(x, y) {
                    out.push((x as u8, y as u8));
                }
            }
        }
        right -= 2;
    }
    out
}

/// The two copies of the format information, bit 0 first.
pub(crate) fn format_positions(size: usize) -> [[(usize, usize); 15]; 2] {
    let mut a = [(0, 0); 15];
    let mut b = [(0, 0); 15];
    for (i, p) in a.iter_mut().enumerate().take(6) {
        *p = (8, i);
    }
    a[6] = (8, 7);
    a[7] = (8, 8);
    a[8] = (7, 8);
    for (i, p) in a.iter_mut().enumerate().skip(9) {
        *p = (14 - i, 8);
    }
    for (i, p) in b.iter_mut().enumerate().take(8) {
        *p = (size - 1 - i, 8);
    }
    for (i, p) in b.iter_mut().enumerate().skip(8) {
        *p = (8, size - 15 + i);
    }
    [a, b]
}

/// The two copies of the version information (versions 7 and up), bit 0
/// first: above the bottom-left finder and left of the top-right finder.
pub(crate) fn version_positions(size: usize) -> [[(usize, usize); 18]; 2] {
    let mut top_right = [(0, 0); 18];
    let mut bottom_left = [(0, 0); 18];
    for i in 0..18 {
        let a = size - 11 + i % 3;
        let b = i / 3;
        top_right[i] = (a, b);
        bottom_left[i] = (b, a);
    }
    [top_right, bottom_left]
}

/// Whether mask pattern `mask` inverts the module at `(x, y)`. Written the
/// way ISO/IEC 18004 Table 10 writes them.
#[allow(clippy::manual_is_multiple_of)]
pub(crate) fn mask_bit(mask: u8, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y) % 2 == 0,
        1 => y % 2 == 0,
        2 => x % 3 == 0,
        3 => (x + y) % 3 == 0,
        4 => (x / 3 + y / 2) % 2 == 0,
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3) % 2 == 0,
        _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_modules_match_the_formula() {
        for v in 1..=40 {
            let layout = Layout::new(v);
            assert_eq!(layout.data.len(), raw_data_modules(v), "version {v}");
            let mut seen = BitMatrix::new(layout.size);
            for &(x, y) in &layout.data {
                assert!(!seen.get(x as usize, y as usize));
                seen.set(x as usize, y as usize, true);
            }
        }
    }

    #[test]
    fn version_1_function_patterns() {
        let l = Layout::new(1);
        // 21×21, of which 208 modules carry data.
        assert_eq!(l.size, 21);
        assert!(l.base.get(0, 0) && l.base.get(6, 6) && !l.base.get(7, 7));
        assert!(l.base.get(8, 13), "dark module");
        assert!(l.base.get(8, 6) && !l.base.get(9, 6), "timing");
        // The first data bit is the bottom-right module.
        assert_eq!(l.data[0], (20, 20));
        assert_eq!(l.data[1], (19, 20));
        assert_eq!(l.data[2], (20, 19));
    }

    #[test]
    fn transpose_is_an_involution() {
        let mut m = BitMatrix::new(21);
        m.set(3, 5, true);
        m.set(20, 0, true);
        let t = m.transposed();
        assert!(t.get(5, 3) && t.get(0, 20) && !t.get(3, 5));
        assert_eq!(t.transposed(), m);
    }
}
