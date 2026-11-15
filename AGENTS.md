# Working in this repository

Airlock is a Solana wallet that keeps preparing a transaction apart from signing it; the [README](README.md) says what it is and describes the crates. [docs/architecture.md](docs/architecture.md) covers how the app crates fit together. The other files in `docs/` cover signing, IDLs, templates on Build, the browser bridge, the design system and releases.

## Before opening a pull request

- **Changelog.** If people who use Airlock will notice the change, add a line under **Unreleased** in `CHANGELOG.md`, in Added, Changed, Fixed, Removed or Security, written for them. If they won't (tooling, CI, refactors, tests, dependency updates), open the pull request with the `no-changelog` label: `gh pr create --label no-changelog`. The Changelog check fails without one or the other. See [docs/releasing.md](docs/releasing.md#changelog-and-release-notes).
- **Screenshots.** If the pull request changes anything people see in the app, put before and after screenshots in the description. A completely new screen or feature has no before, so the after is enough. On Windows, `scripts/drive-desktop.mjs` screenshots the browser request modal without a browser: see [Driving the desktop app](#driving-the-desktop-app).
- **Dependencies.** A lockfile takes no version under 7 days old. If the update fixes a vulnerability, label the pull request `security-override`. See [docs/releasing.md](docs/releasing.md#updating-dependencies).
- **Base.** Branch from `main` and target `main`.
- **Format and lint as CI does.** Run `cargo fmt --all --check`, then the Clippy step of `.github/workflows/ci.yml`, with its `--no-deps` package list. Clippy over the whole workspace also lints the vendored Codama crates, which CI leaves alone.
- **Tests.** Run `cargo test` for the crates you touched. `crates/wallet-core/src/layers.rs` fails a new module that has no layer.

## Driving the desktop app

The browser request modal and other desktop-only screens can be driven without a browser or the extension, on Windows. `scripts/drive-desktop.mjs` does it for sign requests:

```bash
cargo build -p airlock -p bridge-host
node scripts/drive-desktop.mjs <outdir> [transfer|unread|batch|realms ...]
```

It sends one `signTransaction` request per scenario, screenshots the modal into `<outdir>/<scenario>.png`, prints its buttons and rejects the request. With `DRIVE_REVIEW=1` it also opens each request on Review and Sign and screenshots that page into `<outdir>/<scenario>-review.png`. Add a scenario to its `scenarios` table for a transaction of your own. For the before screenshots, build `main` and run it into another folder.

It needs a debug build: a release build drops every `WEBVIEW2_*` variable at start, so the debugging port below is never opened.

`scripts/drive-intents.mjs` does the same for the intents prototype ([docs/intents.md](docs/intents.md)):

```bash
node scripts/drive-intents.mjs <outdir> [memo|mint|cosign|return|message ...]
```

For a hot key it sends `getSigningProfile`, then each scenario of its `scenarios` table (a `resolve` or a `resolveMessage`), screenshots the modal into `<outdir>/<scenario>.png`, prints the modal's text and buttons and every reply and `resolutionChanged` push, and declines. `DRIVE_REVIEW=1` opens each on Review and Sign instead (`<scenario>-review.png`), and `DRIVE_SIGN=cosign,return` presses Sign for those scenarios and prints what their resolutions became. Its helpers (starting the app, adding a wallet, setting a custom RPC, the frame reader and writer, the DevTools calls, program-derived addresses, and an approver that plays the person in the app's window) are in `scripts/drive-lib.mjs`, for a script of your own.

`scripts/drive-intents-e2e.mjs` runs the intents test page (`examples/intents`) against the real app end to end, from built binaries: a local validator (`surfpool --offline`, which must be installed) with a random key funded, the app with that key as a hot wallet and the node as its custom RPC, the page's relay, the approver, and the page driver pressing every row. It takes about a minute, stops everything it started, and exits non-zero unless every run passed and the node holds every transaction the app said ran. Build the page first (`npm install && npm run build` in `examples/intents`). See [its README](examples/intents/README.md#all-of-it-in-one-command-against-the-real-app).

```bash
node scripts/drive-intents-e2e.mjs <outdir> [--only <runs>] [--keep] [--hold <seconds>]
```

What it does, for driving something else:

1. Build `airlock` and `bridge-host`.
2. Start `target/debug/airlock.exe` with `AIRLOCK_DATA_DIR` and `LOCALAPPDATA` set to an empty temp folder, so it opens on a new vault and has its own WebView2 profile. Set `USERNAME` to another name, so it listens on its own bridge pipe (`\\.\pipe\airlock-bridge-<user>`) and an Airlock already running is left alone. Set `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`. Set `CARGO_MANIFEST_DIR` to `apps/airlock`, as `cargo run` does: without it dioxus takes the exe for a bundle, finds no assets beside it, and the IDLs that ship in the IDL pack read raw.
3. Drive the page over the Chrome DevTools Protocol: take the page target from `http://127.0.0.1:<port>/json`, create a vault, add a wallet from a private key, and screenshot with `Page.captureScreenshot`. A debug build started this way does not load the stylesheets, so first add `crates/app-ui-kit/assets/tokens.css` and `main.css` to the page as a `<style>`.
4. Once the app is up, start `target/debug/bridge-host.exe` with the same environment and write to its stdin what the extension would: frames of a 32-bit little-endian length, then the JSON of [a message](docs/browser-bridge.md#messages). Replies come back on stdout the same way. `{"v":1,"id":"status-1","method":"status"}` answers `"role":"desktop"` once it reaches the app. Build the transactions of a `signTransaction` in the script, with the wallet added in step 3 as their fee payer.
5. A program the app has no IDL for is looked up on chain first, and Sign stays disabled until "Looking for the IDLs of its programs…" is gone. Wait for it before reading the buttons.
6. Stop the app and `bridge-host` when done. A killed app leaves WebView2 holding the profile, so give every run its own temp folder.

### On Android

`scripts/drive-intents-android.mjs` is the twin of `drive-intents-e2e.mjs` on an Android emulator, with the same `--wallet` kinds and `--only`. It builds nothing: build the debug APK for the emulator (`dx build --android -p airlock-mobile --target x86_64-linux-android`) and the page first. It starts the `A16` emulator headless when it is not up (`--avd`, `--serial`), installs the APK, clears the app's data, starts surfpool on this machine and `adb reverse`s its port and the page's, sets up the app's wallets through its WebView over the DevTools protocol, opens the page in the emulator's Chrome with `?wallet=mwa` and presses each row with a touch sent through Chrome's DevTools protocol (`--tap adb` uses `adb shell input tap` instead). Chrome counts that touch as the person's tap, so the page may open a session. The approver of `scripts/drive-lib.mjs` plays the person in Airlock, and Bob or Olivia's device act in Airlock as on desktop. Over MWA nobody tells the page that a resolution moved on, so the script presses Check once another party is done, or once no session is open. Its helpers for adb, the emulator, the app's WebView, Chrome and taps are in `scripts/drive-android.mjs`.

```bash
node scripts/drive-intents-android.mjs <outdir> [--wallet <kind>] [--only <ids>] [--keep] [--no-install] [--tap cdp|adb]
```
