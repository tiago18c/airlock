//! Nothing ships from this crate. Its tests run transactions that
//! `wallet-core` builds, and use cases of `wallet-flows`, against the real
//! on-chain programs, loaded into an in-process Solana VM.
//!
//! The program binaries are not in the repository. Fetch them once with
//! `node scripts/fetch-programs.mjs`.
