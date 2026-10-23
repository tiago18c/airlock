//! The camera loop's decoder: `qr-codec`'s scanner behind a plain C ABI, so
//! the webview loads it with `WebAssembly.instantiate` and no generated
//! glue code. `scripts/build-qr-wasm.sh` builds it into
//! `crates/app-ui-kit/assets/qr_codec.wasm`.
//!
//! One frame at a time:
//!
//! 1. `qr_input(len)` returns where to write `len` bytes of RGBA in the
//!    module's memory. Memory may grow during the call, so take a fresh view
//!    of it afterwards.
//! 2. `qr_scan(width, height)` returns the decoded length, or a negative
//!    code: [`NOT_FOUND`], [`UNREADABLE`] or [`BAD_INPUT`].
//! 3. `qr_output()` points at that many bytes, valid until the next scan.
//!
//! There is no `unsafe` block here. The exports are `no_mangle`, which Rust
//! counts as unsafe because two symbols could clash; these three names are
//! the module's whole interface.
#![allow(unsafe_code)]

use qr_codec::{ScanError, Scanner};
use std::cell::RefCell;

/// No symbol in view.
pub const NOT_FOUND: i32 = -1;
/// A symbol in view that did not decode.
pub const UNREADABLE: i32 = -2;
/// The input length does not match the size, or a side is out of range.
pub const BAD_INPUT: i32 = -3;

struct State {
    scanner: Scanner,
    input: Vec<u8>,
    output: Vec<u8>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        scanner: Scanner::new(),
        input: Vec::new(),
        output: Vec::new(),
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn qr_input(len: u32) -> *mut u8 {
    STATE.with_borrow_mut(|s| {
        s.input.clear();
        s.input.resize(len as usize, 0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn qr_scan(width: u32, height: u32) -> i32 {
    STATE.with_borrow_mut(|s| {
        let State {
            scanner,
            input,
            output,
        } = s;
        match scanner.scan_rgba(input, width as usize, height as usize) {
            Ok(d) => {
                *output = d.bytes;
                output.len() as i32
            }
            Err(ScanError::NotFound) => NOT_FOUND,
            Err(ScanError::Unreadable(_)) => UNREADABLE,
            Err(ScanError::BadInput) => BAD_INPUT,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn qr_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}
