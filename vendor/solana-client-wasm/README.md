# Solana WASM Client (vendored)

Fork of [regolith-labs/solana-playground](https://github.com/regolith-labs/solana-playground) `wasm/solana-client`
(commit `eabcd5873c1520fc3d5106235b87b74c87411bb9`).

This copy is wired to Agave 4.2/4.3 split crates (transaction v1) instead of
`solana-sdk 2.1.9` / `solana-extra-wasm`. Desktop builds of qr-sign use the
full `solana-client` crate; this crate is compiled only for `wasm32`.

See `vendor/ORIGIN.md`.
