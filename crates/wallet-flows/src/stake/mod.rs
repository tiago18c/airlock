//! Stake accounts: reading them, and preparing each operation on one.

mod prepare;
mod read;

pub use prepare::{prepare, role_name, StakeRequest};
pub use read::{fetch_stakes, free_stake_index};
