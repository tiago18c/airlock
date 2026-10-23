//! Writes the sweep's frames, so another decoder can be measured on the
//! same pictures: `<dir>/<i>.gray` (luma), `<dir>/<i>.bin` (expected bytes)
//! and `<dir>/index.txt` (`i width height module_px`).

// Each example uses only part of the synthetic camera.
#[allow(dead_code)]
#[path = "../tests/integration/support/mod.rs"]
mod support;
use qr_codec::{encode, EcLevel};
use std::io::Write;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("output dir"));
    let count: usize = std::env::args().nth(2).map_or(1000, |a| a.parse().unwrap());
    let seed: u64 = std::env::args().nth(3).map_or(77, |a| a.parse().unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let mut index = std::fs::File::create(dir.join("index.txt")).unwrap();
    for (i, c) in support::sweep(count, seed).into_iter().enumerate() {
        let frame = support::render(encode(&c.data, EcLevel::M).unwrap().matrix(), &c.scene);
        std::fs::write(dir.join(format!("{i}.gray")), &frame).unwrap();
        std::fs::write(dir.join(format!("{i}.bin")), &c.data).unwrap();
        writeln!(
            index,
            "{i} {} {} {}",
            c.scene.width, c.scene.height, c.scene.module_px
        )
        .unwrap();
    }
}
