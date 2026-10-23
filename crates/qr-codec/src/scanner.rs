//! One camera frame in, at most one symbol out, with bounded work.

use crate::decode::{decode_matrix, looks_like_symbol, version_info, Decoded};
use crate::image::binarize::{smooth_into, Binary};
use crate::image::finder::{self, Finder};
use crate::image::locate::{sample_grid, triples, Model};
use crate::matrix::BitMatrix;
use crate::DecodeError;

/// The longest side a frame may have, in pixels.
pub const MAX_SIDE: usize = 1920;
/// The shortest: a version 1 symbol at one pixel per module.
pub const MIN_SIDE: usize = 21;
/// Finder candidates kept per frame and polarity.
const MAX_FINDERS: usize = 64;
/// Finder candidates triples are formed from, most confirmed first.
const TRIPLE_POOL: usize = 16;
/// Triples tried per polarity. Most cost one check of the finder corners;
/// only those that pass are sampled, against `MAX_DECODES`.
const MAX_TRIPLES: usize = 32;
/// The share of the finder corners that must match before a triple is
/// sampled at all.
const MIN_CORNERS: f64 = 0.8;
/// Grids sampled and decoded per frame, over both polarities. A grid and
/// its mirror image count once.
const MAX_DECODES: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanError {
    /// Wrong buffer length, or a side outside `MIN_SIDE..=MAX_SIDE`.
    BadInput,
    /// No symbol in view.
    NotFound,
    /// A symbol is in view but did not decode: blurred, cut off or damaged.
    Unreadable(DecodeError),
}

impl core::fmt::Display for ScanError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ScanError::BadInput => f.write_str("frame size does not match its buffer"),
            ScanError::NotFound => f.write_str("no QR code in view"),
            ScanError::Unreadable(e) => write!(f, "QR code in view but unreadable: {e}"),
        }
    }
}

impl std::error::Error for ScanError {}

/// Scans frames. Keep one per camera: its buffers are reused, so a frame
/// allocates little beyond the bytes it returns.
#[derive(Default)]
pub struct Scanner {
    luma: Vec<u8>,
    binary: Option<Binary>,
    smooth: Vec<u8>,
    black: Vec<u8>,
    finders: Vec<Finder>,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// `luma` is `width * height` bytes, row-major, 0 black to 255 white.
    pub fn scan(&mut self, luma: &[u8], width: usize, height: usize) -> Result<Decoded, ScanError> {
        check(luma.len(), 1, width, height)?;
        let mut binary = self.binary.take().unwrap_or_else(Binary::new);
        smooth_into(luma, width, height, &mut self.smooth);
        let mut budget = Budget {
            decodes: 0,
            seen: None,
        };
        let mut found = None;
        for inverted in [false, true] {
            // Light-on-dark symbols: threshold the negative, so flat
            // background is treated the same way as on paper.
            if inverted {
                for v in &mut self.smooth {
                    *v = 255 - *v;
                }
            }
            binary.threshold(&self.smooth, width, height, &mut self.black);
            found = self.search(&binary, &mut budget);
            if found.is_some() {
                break;
            }
        }
        self.binary = Some(binary);
        found.unwrap_or(Err(budget
            .seen
            .map_or(ScanError::NotFound, ScanError::Unreadable)))
    }

    /// `rgba` is `width * height * 4` bytes, as canvas `getImageData` gives.
    pub fn scan_rgba(
        &mut self,
        rgba: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Decoded, ScanError> {
        check(rgba.len(), 4, width, height)?;
        let mut luma = core::mem::take(&mut self.luma);
        luma.clear();
        // Rec. 601 weights, out of 256.
        let (pixels, _) = rgba.as_chunks::<4>();
        luma.extend(
            pixels
                .iter()
                .map(|&[r, g, b, _]| ((r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8) as u8),
        );
        let result = self.scan(&luma, width, height);
        self.luma = luma;
        result
    }

    /// One polarity. `None` to go on to the next; `Some` when a symbol
    /// decoded or the budget ran out.
    fn search(
        &mut self,
        binary: &Binary,
        budget: &mut Budget,
    ) -> Option<Result<Decoded, ScanError>> {
        finder::find(binary, &mut self.finders, MAX_FINDERS);
        if self.finders.len() < 3 {
            return None;
        }
        let mut pool = self.finders.clone();
        pool.sort_by_key(|f| core::cmp::Reverse(f.count));
        pool.truncate(TRIPLE_POOL);
        for corners in triples(&pool, MAX_TRIPLES) {
            let Some(model) = Model::new(binary, &corners) else {
                continue;
            };
            let mut queue = model.versions(binary);
            // Not a symbol's corners: no version or pass will make it one.
            if queue
                .first()
                .is_none_or(|&v| model.corners_match(binary, v) < MIN_CORNERS)
            {
                continue;
            }
            let mut next = 0;
            while let Some(&version) = queue.get(next) {
                next += 1;
                let passes: &[bool] = if version >= 7 {
                    &[true, false]
                } else {
                    &[true]
                };
                for &refine in passes {
                    if budget.decodes == MAX_DECODES {
                        return Some(Err(budget
                            .seen
                            .map_or(ScanError::NotFound, ScanError::Unreadable)));
                    }
                    budget.decodes += 1;
                    let Some(grid) = sample_grid(binary, &corners, &model, version, refine) else {
                        continue;
                    };
                    match attempt(&grid) {
                        Ok(d) => return Some(Ok(d)),
                        Err(Failure {
                            error,
                            symbol,
                            named,
                        }) => {
                            if symbol {
                                budget.seen = Some(error);
                            }
                            // The grid's own version information names
                            // another size: sample at that one too.
                            if let Some(v) = named.filter(|v| !queue.contains(v)) {
                                queue.push(v);
                            }
                        }
                    }
                }
            }
        }
        None
    }
}

struct Budget {
    decodes: usize,
    /// The last failure on a grid that looked like a symbol.
    seen: Option<DecodeError>,
}

struct Failure {
    error: DecodeError,
    /// Both format copies agreed, so a symbol is there.
    symbol: bool,
    /// The version the grid's version information names, when it
    /// disagreed with the size sampled.
    named: Option<u8>,
}

/// Decodes a grid, and its mirror image for a symbol seen from behind or
/// with its corners swapped.
fn attempt(grid: &BitMatrix) -> Result<Decoded, Failure> {
    let mirror = grid.transposed();
    let mut failure = Failure {
        error: DecodeError::Format,
        symbol: false,
        named: None,
    };
    for g in [grid, &mirror] {
        match decode_matrix(g) {
            Ok(d) => return Ok(d),
            Err(e) => {
                if e == DecodeError::Version {
                    failure.named = failure.named.or(version_info(g));
                }
                if looks_like_symbol(g) {
                    failure.symbol = true;
                    failure.error = e;
                }
            }
        }
    }
    Err(failure)
}

fn check(len: usize, channels: usize, width: usize, height: usize) -> Result<(), ScanError> {
    let sides = MIN_SIDE..=MAX_SIDE;
    if sides.contains(&width) && sides.contains(&height) && len == width * height * channels {
        Ok(())
    } else {
        Err(ScanError::BadInput)
    }
}
