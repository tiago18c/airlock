//! Plane homographies: the map from module space to image space.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub(crate) fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
    pub(crate) fn distance(self, o: Point) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    pub(crate) fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
    pub(crate) fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
    pub(crate) fn scale(self, k: f64) -> Point {
        Point::new(self.x * k, self.y * k)
    }
}

/// A 3×3 projective map, row-major, acting on column vectors (x, y, 1).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Homography([f64; 9]);

impl Homography {
    /// Maps the unit square's corners (0,0), (1,0), (1,1), (0,1) to `q`
    /// (Heckbert, "Fundamentals of Texture Mapping", 1989).
    fn square_to_quad(q: [Point; 4]) -> Option<Self> {
        let [p0, p1, p2, p3] = q;
        let sx = p0.x - p1.x + p2.x - p3.x;
        let sy = p0.y - p1.y + p2.y - p3.y;
        let (g, h) = if sx.abs() < 1e-9 && sy.abs() < 1e-9 {
            (0.0, 0.0)
        } else {
            let (dx1, dx2) = (p1.x - p2.x, p3.x - p2.x);
            let (dy1, dy2) = (p1.y - p2.y, p3.y - p2.y);
            let den = dx1 * dy2 - dx2 * dy1;
            if den.abs() < 1e-12 {
                return None;
            }
            ((sx * dy2 - dx2 * sy) / den, (dx1 * sy - sx * dy1) / den)
        };
        Some(Homography([
            p1.x - p0.x + g * p1.x,
            p3.x - p0.x + h * p3.x,
            p0.x,
            p1.y - p0.y + g * p1.y,
            p3.y - p0.y + h * p3.y,
            p0.y,
            g,
            h,
            1.0,
        ]))
    }

    /// Maps quadrilateral `from` onto `to`, corner by corner.
    pub(crate) fn quad_to_quad(from: [Point; 4], to: [Point; 4]) -> Option<Self> {
        let a = Self::square_to_quad(from)?.inverse()?;
        let b = Self::square_to_quad(to)?;
        Some(b.then(&a))
    }

    fn inverse(&self) -> Option<Self> {
        let m = &self.0;
        let c = [
            m[4] * m[8] - m[5] * m[7],
            m[2] * m[7] - m[1] * m[8],
            m[1] * m[5] - m[2] * m[4],
            m[5] * m[6] - m[3] * m[8],
            m[0] * m[8] - m[2] * m[6],
            m[2] * m[3] - m[0] * m[5],
            m[3] * m[7] - m[4] * m[6],
            m[1] * m[6] - m[0] * m[7],
            m[0] * m[4] - m[1] * m[3],
        ];
        let det = m[0] * c[0] + m[1] * c[3] + m[2] * c[6];
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        Some(Homography(c.map(|v| v / det)))
    }

    /// `self ∘ other`: apply `other` first.
    fn then(&self, other: &Self) -> Self {
        let (a, b) = (&self.0, &other.0);
        let mut out = [0.0; 9];
        for r in 0..3 {
            for c in 0..3 {
                out[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
            }
        }
        Homography(out)
    }

    pub(crate) fn map(&self, x: f64, y: f64) -> Point {
        let m = &self.0;
        let w = m[6] * x + m[7] * y + m[8];
        Point::new(
            (m[0] * x + m[1] * y + m[2]) / w,
            (m[3] * x + m[4] * y + m[5]) / w,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn maps_corners_to_corners() {
        let from = [
            Point::new(3.5, 3.5),
            Point::new(41.5, 3.5),
            Point::new(38.5, 38.5),
            Point::new(3.5, 41.5),
        ];
        let to = [
            Point::new(100.0, 120.0),
            Point::new(410.0, 90.0),
            Point::new(450.0, 380.0),
            Point::new(80.0, 400.0),
        ];
        let h = Homography::quad_to_quad(from, to).unwrap();
        for (f, t) in from.iter().zip(&to) {
            assert!(close(h.map(f.x, f.y), *t), "{f:?} -> {:?}", h.map(f.x, f.y));
        }
    }

    #[test]
    fn affine_case() {
        let unit = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(0.0, 1.0),
        ];
        let to = [
            Point::new(10.0, 10.0),
            Point::new(30.0, 15.0),
            Point::new(25.0, 35.0),
            Point::new(5.0, 30.0),
        ];
        let h = Homography::quad_to_quad(unit, to).unwrap();
        assert!(close(h.map(0.5, 0.5), Point::new(17.5, 22.5)));
    }

    #[test]
    fn degenerate_quads_are_rejected() {
        let line = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(3.0, 0.0),
        ];
        let unit = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(0.0, 1.0),
        ];
        assert!(Homography::quad_to_quad(line, unit).is_none());
    }
}
