//! Paths to `app-core` logging, plus naming for UI source tags.

pub use app_core::diagnostics::*;

use crate::state::forms::TxSource;

pub fn source_name(source: &TxSource) -> &'static str {
    match source {
        TxSource::Build => "build",
        TxSource::Wallets => "wallets",
        TxSource::Stake => "stake",
        TxSource::Browser { .. } => "browser",
        TxSource::Pasted => "pasted",
        TxSource::Scanned => "scanned",
    }
}
