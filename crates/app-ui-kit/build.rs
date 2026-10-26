//! Pack `resources/idls` into `assets/idls.pack`, which `asset!` ships beside
//! the app, and pin its SHA-256 for the app to check when it reads the pack.

use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let source = crate_dir.join("../../resources/idls");
    let out = crate_dir.join("assets/idls.pack");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", out.display());

    let packed =
        idl_pack::pack_dir(&source).unwrap_or_else(|err| panic!("packing resources/idls: {err}"));
    // Written only when it changed, so the asset is not rebuilt for nothing.
    if std::fs::read(&out).ok().as_deref() != Some(packed.bytes.as_slice()) {
        std::fs::write(&out, &packed.bytes)
            .unwrap_or_else(|err| panic!("writing {}: {err}", out.display()));
    }
    let hex: String = packed
        .sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!("cargo:rustc-env=AIRLOCK_IDL_PACK_SHA256={hex}");
}
