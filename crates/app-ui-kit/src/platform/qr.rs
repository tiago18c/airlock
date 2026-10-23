use qr_codec::{encode, EcLevel, Symbol};
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct QrImage {
    pub svg: String,
    pub version: u8,
    pub modules: usize,
    pub encoded_bytes: usize,
}

/// Modules of light margin around the symbol, as the spec asks.
const QUIET_ZONE: usize = 4;
const DARK: &str = "#0c0f15";
const LIGHT: &str = "#ffffff";

pub fn render_svg(data: &[u8]) -> Result<QrImage, String> {
    let symbol = encode(data, EcLevel::M)
        .or_else(|_| encode(data, EcLevel::L))
        .map_err(|e| format!("QR encode failed ({e})"))?;
    Ok(QrImage {
        svg: svg(&symbol),
        version: symbol.version(),
        modules: symbol.size(),
        encoded_bytes: data.len(),
    })
}

/// One SVG unit per module, each row's dark runs as rectangles, at least
/// 280 px across in whole pixels per module.
fn svg(symbol: &Symbol) -> String {
    let m = symbol.matrix();
    let n = symbol.size();
    let side = n + 2 * QUIET_ZONE;
    let px = 280usize.div_ceil(side) * side;
    let mut path = String::new();
    for y in 0..n {
        let mut x = 0;
        while x < n {
            if !m.get(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < n && m.get(x, y) {
                x += 1;
            }
            let run = x - start;
            let _ = write!(
                path,
                "M{} {}h{run}v1h-{run}z",
                start + QUIET_ZONE,
                y + QUIET_ZONE
            );
        }
    }
    format!(
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{px}" height="{px}" "#,
            r#"viewBox="0 0 {side} {side}" shape-rendering="crispEdges">"#,
            r#"<rect width="{side}" height="{side}" fill="{light}"/>"#,
            r#"<path fill="{dark}" d="{path}"/></svg>"#
        ),
        px = px,
        side = side,
        light = LIGHT,
        dark = DARK,
        path = path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use qr_codec::{decode_matrix, BitMatrix};
    use wallet_core::protocol::{encode_frames, DEFAULT_CHUNK, MAX_PAYLOAD};

    /// Reads the modules back out of the SVG's path.
    fn modules_of(svg: &str, size: usize) -> BitMatrix {
        let d = svg
            .split(" d=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let mut m = BitMatrix::new(size);
        for rect in d.split('M').filter(|s| !s.is_empty()) {
            let (xy, rest) = rect.split_once('h').unwrap();
            let (x, y) = xy.split_once(' ').unwrap();
            let run: usize = rest.split('v').next().unwrap().parse().unwrap();
            let (x, y): (usize, usize) = (x.parse().unwrap(), y.parse().unwrap());
            for dx in 0..run {
                m.set(x + dx - QUIET_ZONE, y - QUIET_ZONE, true);
            }
        }
        m
    }

    #[test]
    fn every_code_is_the_same_version_whatever_the_payload() {
        for len in [1, 100, MAX_PAYLOAD] {
            let frames = encode_frames(&vec![42u8; len], DEFAULT_CHUNK).unwrap();
            for frame in frames {
                let img = render_svg(&frame.encode()).unwrap();
                assert_eq!(img.version, 23);
                assert!(img.svg.contains("<svg"));
            }
        }
    }

    #[test]
    fn the_svg_draws_the_symbol() {
        let data = b"QRS1 frame bytes, drawn and read back";
        let img = render_svg(data).unwrap();
        let decoded = decode_matrix(&modules_of(&img.svg, img.modules)).unwrap();
        assert_eq!(decoded.bytes, data);
        assert!(
            img.svg.contains(r#"viewBox="0 0 37 37""#),
            "{}",
            &img.svg[..120]
        );
    }
}
