# Working in this repository

Airlock is a Solana wallet that keeps preparing a transaction apart from signing it; the [README](README.md) says what it is and describes the crates. [docs/architecture.md](docs/architecture.md) covers how the app crates fit together. The other files in `docs/` cover signing, IDLs, the browser bridge, the design system and releases.

## Before opening a pull request

- **Changelog.** If people who use Airlock will notice the change, add a line under **Unreleased** in `CHANGELOG.md`, in Added, Changed, Fixed, Removed or Security, written for them. If they won't (tooling, CI, refactors, tests, dependency updates), open the pull request with the `no-changelog` label: `gh pr create --label no-changelog`. The Changelog check fails without one or the other. See [docs/releasing.md](docs/releasing.md#changelog-and-release-notes).
- **Base.** Branch from `main` and target `main`.
- **Format and lint as CI does.** Run `cargo fmt --all --check`, then the Clippy step of `.github/workflows/ci.yml`, with its `--no-deps` package list. Clippy over the whole workspace also lints the vendored Codama crates, which CI leaves alone.
- **Tests.** Run `cargo test` for the crates you touched. `crates/wallet-core/src/layers.rs` fails a new module that has no layer.
