# Working in this repository

Airlock is a Solana wallet that keeps preparing a transaction apart from signing it; the [README](README.md) says what it is and describes the crates. [docs/architecture.md](docs/architecture.md) covers how the app crates fit together. The other files in `docs/` cover signing, IDLs, the browser bridge, the design system and releases.

## Before opening a pull request

- **Changelog.** If people who use Airlock will notice the change, add a line under **Unreleased** in `CHANGELOG.md`, in Added, Changed, Fixed, Removed or Security, written for them. If they won't (tooling, CI, refactors, tests, dependency updates), open the pull request with the `no-changelog` label: `gh pr create --label no-changelog`. The Changelog check fails without one or the other. See [docs/releasing.md](docs/releasing.md#changelog-and-release-notes).
- **Screenshots.** If the pull request changes anything people see in the app, put before and after screenshots in the description. A completely new screen or feature has no before, so the after is enough. On Windows, `scripts/drive-desktop.mjs` screenshots the browser request modal without a browser: see [Driving the desktop app](#driving-the-desktop-app).
- **Base.** Branch from `main` and target `main`.
- **Format and lint as CI does.** Run `cargo fmt --all --check`, then the Clippy step of `.github/workflows/ci.yml`, with its `--no-deps` package list. Clippy over the whole workspace also lints the vendored Codama crates, which CI leaves alone.
- **Tests.** Run `cargo test` for the crates you touched. `crates/wallet-core/src/layers.rs` fails a new module that has no layer.

## Driving the desktop app

The browser request modal and other desktop-only screens can be driven without a browser or the extension, on Windows. `scripts/drive-desktop.mjs` does it for sign requests:

```bash
cargo build -p airlock -p bridge-host
node scripts/drive-desktop.mjs <outdir> [transfer|unread|batch|realms ...]
```

It sends one `signTransaction` request per scenario, screenshots the modal into `<outdir>/<scenario>.png`, prints its buttons and rejects the request. Add a scenario to its `scenarios` table for a transaction of your own. For the before screenshots, build `main` and run it into another folder.

What it does, for driving something else:

1. Build `airlock` and `bridge-host`.
2. Start `target/debug/airlock.exe` with `AIRLOCK_DATA_DIR` and `LOCALAPPDATA` set to an empty temp folder, so it opens on a new vault and has its own WebView2 profile. Set `USERNAME` to another name, so it listens on its own bridge pipe (`\\.\pipe\airlock-bridge-<user>`) and an Airlock already running is left alone. Set `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`. Set `CARGO_MANIFEST_DIR` to `apps/airlock`, as `cargo run` does: without it dioxus takes the exe for a bundle, finds no assets beside it, and the IDLs that ship in the IDL pack read raw.
3. Drive the page over the Chrome DevTools Protocol: take the page target from `http://127.0.0.1:<port>/json`, create a vault, add a wallet from a private key, and screenshot with `Page.captureScreenshot`. A debug build started this way does not load the stylesheets, so first add `crates/app-ui-kit/assets/tokens.css` and `main.css` to the page as a `<style>`.
4. Once the app is up, start `target/debug/bridge-host.exe` with the same environment and write to its stdin what the extension would: frames of a 32-bit little-endian length, then the JSON of [a message](docs/browser-bridge.md#messages). Replies come back on stdout the same way. `{"v":1,"id":"status-1","method":"status"}` answers `"role":"desktop"` once it reaches the app. Build the transactions of a `signTransaction` in the script, with the wallet added in step 3 as their fee payer.
5. A program the app has no IDL for is looked up on chain first, and Sign stays disabled until "Looking for the IDLs of its programs…" is gone. Wait for it before reading the buttons.
6. Stop the app and `bridge-host` when done. A killed app leaves WebView2 holding the profile, so give every run its own temp folder.
