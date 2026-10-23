#!/usr/bin/env bash
# Builds the camera loop's QR decoder (crates/qr-codec-wasm) into
# crates/app-ui-kit/assets/qr_codec.wasm, which is committed.
#
#   scripts/build-qr-wasm.sh          rebuild and write the module
#   scripts/build-qr-wasm.sh --check  rebuild and fail if it differs
#
# CI runs --check, so the committed module is always what its source builds
# to. Rebuild and commit it with every change to qr-codec.
#
# The build is byte-for-byte the same on every machine with the pinned
# toolchain. That is why it calls rustc directly: cargo derives each
# crate's symbol hashes from its absolute path, which changes the code
# layout from one checkout to the next. Both crates have no dependencies,
# so two rustc calls are the whole build. Source paths in panic messages
# are remapped to fixed names for the same reason.
set -euo pipefail

cd "$(dirname "$0")/.."

# Ensure rustc is on PATH when this runs under Git Bash on Windows.
if ! command -v rustc >/dev/null 2>&1; then
  if [ -n "${CARGO_HOME:-}" ] && [ -x "${CARGO_HOME}/bin/rustc" ]; then
    export PATH="${CARGO_HOME}/bin:${PATH}"
  elif [ -x "${HOME}/.cargo/bin/rustc" ]; then
    export PATH="${HOME}/.cargo/bin:${PATH}"
  fi
fi

out=crates/app-ui-kit/assets/qr_codec.wasm
dir="${QR_WASM_TARGET_DIR:-target/qr-wasm}"
rm -rf "$dir"
mkdir -p "$dir"

# With the rust-src component installed, rustc points standard library
# paths at the local toolchain. Point them back where a toolchain without
# it does.
sysroot="$(rustc --print sysroot)"
hash="$(rustc -vV | sed -n 's/^commit-hash: //p')"
remap=("--remap-path-prefix=${sysroot}/lib/rustlib/src/rust=/rustc/${hash}")
# rustc joins submodule paths with the platform's separator. Prefixes match
# by path component, so each file maps to one forward-slash name.
while IFS= read -r file; do
  remap+=("--remap-path-prefix=${file}=${file}")
done < <(find crates/qr-codec/src crates/qr-codec-wasm/src -name '*.rs' | LC_ALL=C sort)

common=(
  --edition 2021
  --target wasm32-unknown-unknown
  -C opt-level=3
  -C codegen-units=1
  -C panic=abort
  -C debuginfo=0
  -C strip=symbols
  "${remap[@]}"
)
rustc "${common[@]}" --crate-type rlib --crate-name qr_codec -C metadata=qr-codec \
  --out-dir "$dir" crates/qr-codec/src/lib.rs
rustc "${common[@]}" --crate-type cdylib --crate-name qr_codec_wasm -C metadata=qr-codec-wasm \
  -C lto=fat --extern qr_codec="$dir/libqr_codec.rlib" \
  --out-dir "$dir" crates/qr-codec-wasm/src/lib.rs
built="$dir/qr_codec_wasm.wasm"

if [ "${1:-}" = --check ]; then
  if ! cmp -s "$built" "$out"; then
    echo "::error::$out does not match its source. Run scripts/build-qr-wasm.sh and commit the result."
    exit 1
  fi
  echo "$out matches its source."
else
  cp "$built" "$out"
  echo "Wrote $out ($(wc -c < "$out") bytes)."
fi
