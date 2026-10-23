# Vendored Solana WASM RPC client

Source: [regolith-labs/solana-playground](https://github.com/regolith-labs/solana-playground)
Path: `wasm/solana-client`
Commit: `eabcd5873c1520fc3d5106235b87b74c87411bb9` (2025-04-18)

Upstream targeted `solana-sdk 2.1.9` and a sibling `solana-extra-wasm` kitchen-sink crate.
This fork:

- Keeps the JSON-RPC `WasmClient` and method modules
- Replaces `solana-sdk` / `solana-extra-wasm` with Agave 4.2 client-types plus split `solana-*` crates that speak transaction v1 (SIMD-0385)
- Drops the CLI `clap` helpers
- Adds `WasmClient::new_with_timeout`, which limits every request
- Adds `simulate_raw_transaction` beside `send_raw_transaction`, for transaction v1 wire bytes, with `innerInstructions` in the request and inner instructions and the loaded accounts data size in the result
- Adds `get_stake_minimum_delegation`
- Fixes `get_multiple_accounts`, which dropped accounts that do not exist and so answered out of order
- Adds `get_multiple_accounts_with_config_and_slot`, which answers with the slot the node read at, and `get_token_accounts_by_owner_with_config`, which can ask for a `minContextSlot`

License: Apache-2.0 (see `solana-client-wasm/LICENSE-APACHE`).
