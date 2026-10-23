# qr-codec

QR codes for Airlock: an encoder, and a decoder for camera frames. No dependencies, no `unsafe`, about 3,200 lines including its unit tests. It replaces the `qrcode` crate, jsQR and the browser's `BarcodeDetector`, so the app has no QR code we did not write.

Scope: Model 2 symbols, versions 1–40, ECC levels L/M/Q/H, all eight masks. The encoder writes one byte-mode segment. The decoder reads numeric, alphanumeric and byte segments, and returns `UnsupportedMode` for ECI, Kanji, Hanzi, structured append and FNC1. Micro QR, rMQR and Model 1 are not recognised.

## Where each step is

Section numbers are ISO/IEC 18004:2015. The spec is paid; every table here is also checked against an independent implementation (see [Tests](#tests)).

| Step | File | Spec |
| --- | --- | --- |
| GF(2⁸), polynomial 0x11D | `src/gf256.rs` | 7.5.2 |
| Reed–Solomon encode; decode by Berlekamp–Massey, Chien, Forney | `src/rs.rs` | 7.5.2, Annex B |
| Format information BCH(15,5), version information BCH(18,6) | `src/bch.rs` | 7.9, 7.10, Annexes C and D |
| ECC per block, number of blocks, alignment centres, misdecode protection | `src/tables.rs` | Tables 9, E.1 |
| Function patterns, zigzag placement, masks | `src/matrix.rs` | 6.3, 7.7, 7.8 |
| Segments, terminator, padding | `src/bitstream.rs` | 7.4 |
| Encoding and mask choice (penalty N1–N4) | `src/encode.rs` | 7.1, 7.8.3 |
| Grid to bytes: format, version, unmask, de-interleave, correct, parse | `src/decode.rs` | 7.6, 12 |
| Smoothing and adaptive threshold | `src/image/binarize.rs` | 12 (reference algorithm, step a) |
| Finder patterns: 1:1:3:1:1 runs, cross-checks | `src/image/finder.rs` | 12 (steps b–c) |
| Triples, perspective model, version estimate, alignment search, per-cell homographies, sampling | `src/image/locate.rs` | 12 (steps d–h) |
| Homographies | `src/image/transform.rs` | — |
| One frame in, one symbol out, with work limits | `src/scanner.rs` | — |

## The decoder on a camera frame

1. Smooth with a [1 2 1] filter, then threshold each pixel against the mean of the 8×8-block black points around it. Flat blocks borrow from their neighbours, so a plain background stays light.
2. Scan every row for dark/light runs in 1:1:3:1:1. Confirm each with vertical, horizontal and diagonal cross-sections, each tried on its own line and the two beside it. Merge hits within a module of each other.
3. Rank triples by how square they are and by how many rows confirmed each finder.
4. Build a homography from the three centres and from how the finders' widths shrink along each edge (`Model`). From it, estimate the version, and read the version information near the top-right and bottom-left finders.
5. Before sampling, check that the three 8×8 finder corners match. A triple that is not one symbol's corners costs 192 samples.
6. Find the bottom-right alignment pattern by template matching where the model predicts it. From version 7, find every alignment pattern, from the top-left outward, each predicted from the drift of its found neighbours. Use one homography per cell between them.
7. Sample module centres, then decode the grid and its mirror image.
8. If nothing decodes, do it all again on the negative, for light-on-dark codes.

Every frame's work is bounded (`scanner.rs`): at most 64 finder candidates, 32 triples per polarity, and 24 sampled grids. A pure-noise 720p frame, the worst case found, takes about 11 ms natively.

## Tests

- `tests/integration/oracle.rs`: the `qrcode` crate as a reference (a dev-dependency only). For all 40 versions × 4 levels × 8 masks, our decoder reads its symbol and our encoder draws the same modules.
- Unit tests check the generator polynomial, ECC vectors, all 32 format words and 34 version words, capacities and block splits against published tables. Reed–Solomon is tested up to and past its bound.
- `tests/integration/scan.rs`: a synthetic camera (`tests/integration/support`) with rotation, perspective, lens distortion, blur, noise, uneven lighting, clutter, inversion and mirroring. A 400-case randomized sweep must read at least 96 %; 388 read when this was written, and every miss is under 3 px a module.
- `tests/integration/robustness.rs`: random and damaged grids, and frames built to be hard (finder stripes, a wall of finder patterns, many symbols at once), must not panic or run long.
- `fuzz/`: four libFuzzer targets, run in CI (`.github/workflows/fuzz.yml`).

`cargo run --release -p qr-codec --example bench` times a frame. `examples/dump_sweep.rs` writes the sweep's frames, so another decoder can be measured on the same pictures.

## In the app

`crates/qr-codec-wasm` wraps the scanner in three WebAssembly exports for `camera.js`. `scripts/build-qr-wasm.sh` builds it into `crates/app-ui-kit/assets/qr_codec.wasm`, byte for byte the same on every machine. CI rebuilds it and fails if the committed file differs, so **rebuild and commit it with every change to this crate.**
