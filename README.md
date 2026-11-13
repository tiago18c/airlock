# Airlock

A Solana wallet that keeps preparing a transaction apart from signing it. An online device builds, reviews and submits. Another device, which may never connect to a network, reviews the same transaction on its own and signs it. Transactions and signatures move between the two as QR codes or pasted text. The same Dioxus app runs on the desktop, in a browser and on Android phones, and can play either role. Keys are kept in a password-sealed vault, or on a hardware wallet. On desktop, a browser extension can forward a site's request to the app, so the site gets a normal Wallet Standard wallet while the keys stay in the app; on Android, other apps and sites reach it through Mobile Wallet Adapter. Templates on Build write common transactions from a few answers, and the IDLs of over 90 programs ship with the app to explain their instructions before they are signed.

**Not ready for production, and not formally audited.** This is a prototype that handles private keys and signs transactions. Do not use it to hold or move real funds. Read the [disclaimer](#disclaimer) first.

**Download:** [the latest release](https://github.com/tiago18c/airlock/releases/latest). Its page says which file is for which computer or phone, and how to install each. To report a vulnerability, see [SECURITY.md](SECURITY.md).

## Scope

- **Solana transactions:** browser requests and Sign-screen paste accept legacy, v0 and v1. A hot-only request can be signed as is; anything else is converted to v1 (wire prefix `0x81`, SIMD-0385). The QR handoff (QRS1) stays v1 only; scanning a non-v1 QR is refused.
- **Wallet kinds:** hot (private key or seed phrase), offline (the key is on another device), Ed25519 programmatic signer, Vector wallet (once turned on in Settings), Squads multisig (v4 and smart account), and hardware: Ledger, Trezor and Keystone once **Pre-release hardware wallets** is on in Settings, and the Seed Vault of a Saga or Seeker on Android.
- **Online and offline:** the online app (`airlock`, on the desktop and in a browser), and `airlock-offline`, a desktop build which opens no connection of its own, and cannot simulate or submit. That is what it does, not something it is unable to do: its window is a WebView, and its UI library brings network code along. Keeping it on a device with no network is what keeps it offline. Airlock for Android is one app that runs in either mode, chosen on the first launch and in Settings.
- **Browser bridge:** desktop. It answers `connect`, `disconnect`, `signTransaction`, `signAndSendTransaction`, `signAndSendAllTransactions`, `signMessage` and `signIn`. One request holds one to 16 transactions, which go back together. Every other method returns `not_implemented`. On Android, online, Mobile Wallet Adapter requests from other apps and from sites in Chrome are read into the same requests and go through the same checks and modals ([docs/android.md](docs/android.md#requests-from-other-apps-mobile-wallet-adapter)).

How each kind of wallet signs, from the app and from a site, is in [docs/signing.md](docs/signing.md).

## The apps

Five things run, on up to two devices and a browser:

| App | Where it runs | What it does |
| --- | --- | --- |
| **Airlock** (`apps/airlock`) | Desktop or browser, online | Holds a vault, builds, reviews, rebuilds and submits transactions, and answers the browser extension (desktop only) |
| **Airlock Offline** (`apps/airlock-offline`) | Desktop, offline | Holds its own vault, reviews and signs. It opens no connection of its own, so it cannot simulate or submit |
| **Airlock for Android** (`apps/airlock-mobile`) | Android 14 and later, online or offline | The online app's screens on a phone, with a vault for each mode, the camera for QR codes, and the Seed Vault of a Saga or Seeker. Online, it answers other apps and sites through Mobile Wallet Adapter. See [docs/android.md](docs/android.md) |
| **Airlock Extension** (`extension`) | The browser | Shows sites a Wallet Standard wallet and forwards their requests. It holds no keys and approves nothing |
| **bridge-host** (`crates/bridge-host`) | Started by the browser | Relays between the extension and the running desktop app |

```mermaid
flowchart LR
  subgraph Browser["Browser (untrusted)"]
    Site[Site]
    Ext["Extension<br/>(no keys)"]
  end
  Host["bridge-host<br/>(native messaging)"]
  subgraph Online["Online device"]
    App["Airlock<br/>(vault, review, rebuild, submit)"]
  end
  subgraph Offline["Offline device (airgap)"]
    Cold["Airlock Offline<br/>(vault, review, sign)"]
  end
  RPC[(Solana RPC)]

  Site <-->|Wallet Standard| Ext
  Ext <-->|stdio| Host
  Host <-->|per-user local socket| App
  App <-->|JSON-RPC| RPC
  App <-->|QRS1 QR or text| Cold
```

A transaction starts on the online app: built there, or sent by a site through the extension and `bridge-host`. The online app reviews it and signs with its own keys. For a key on the offline device, it shows the transaction as QR codes (or text to paste). The offline app reads them, reviews the transaction on its own and signs, and its signatures go back the same way. The online app then submits, or returns the signed transaction to the site.

The site, the extension and the browser are trusted for nothing except reporting the origin. The online app decides for its own keys. For a key on the offline device, the offline app decides from its own reading of the transaction. The node is trusted for what it says the chain holds, and never for what is signed. The threat model is in [docs/browser-bridge.md](docs/browser-bridge.md#threat-model).

## Running it

**Prerequisites:**

- Rust through `rustup`. `rust-toolchain.toml` pins the compiler and the wasm target, and `rustup` installs them on first use.
- The Dioxus CLI at the version releases use: `cargo binstall dioxus-cli@0.7.10` (or `cargo install dioxus-cli@0.7.10`).
- On Windows, a WebView2 runtime, which recent Windows already has.
- For the extension, Node 22.

Run the apps through `dx`, not plain `cargo run` or `cargo build`: `dx` collects the assets the WebView loads.

### Airlock (online)

```bash
dx serve -p airlock --desktop                 # desktop
dx serve -p airlock --web --addr 0.0.0.0      # web, at http://127.0.0.1:8080
```

The camera works on `localhost`, but browsers block it on a plain-HTTP LAN address. To scan from a phone on the LAN: set `[web.https] enabled = true` in `apps/airlock/Dioxus.toml`, run `scripts/gen-dev-cert.ps1`, restart the web server and accept the certificate warning. On macOS, run the desktop app through `dx` (it writes the camera permission into `Info.plist`) and allow the camera under System Settings → Privacy & Security → Camera if asked. On Windows, restart `dx serve` fully after changing the WebView2 camera flags.

### Airlock Offline

```bash
dx serve -p airlock-offline --desktop
```

It keeps its own data folder (`airlock-offline`), so both apps can run on one machine with different vaults.

### Airlock for Android

```bash
dx build --android -p airlock-mobile --target aarch64-linux-android
```

It needs the Android SDK with an NDK, and JDK 17. Setting them up, installing on a phone or an emulator, and driving the app in a test are in [docs/android.md](docs/android.md#building-and-running).

### Hardware wallets on Linux

A Ledger is opened through `hidraw`, and a Trezor through USB, which Linux lets only root open until a udev rule says otherwise. Install the rules once, then unplug the device and plug it in again:

```bash
sudo cp resources/linux/*.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Windows and macOS need nothing. A Keystone is read through the camera, and needs nothing either. Hardware wallets are offered once **Pre-release hardware wallets** is on in Settings: Ledger's Solana app 1.18 and Trezor's next firmware, the first to read v1 transactions, are pre-release builds so far.

### Extension and bridge-host (desktop only)

```bash
cd extension && npm install && npm run build && cd ..
cargo run -p bridge-host -- install
```

1. Load `extension/dist` as an unpacked extension in a Chromium browser (Chrome, Edge, Brave).
2. `install` registers `bridge-host` with the browsers for the current user. Run it again if the binary moves; `bridge-host uninstall` removes it.
3. Restart the browser, then start Airlock on the desktop, unlock it and set it to Online.

`extension/demo.html` is a test page; details and the message format are in [docs/browser-bridge.md](docs/browser-bridge.md).

### Installers

```bash
dx bundle -p airlock --release --desktop --features bundle                          # portable folder
dx bundle -p airlock --release --desktop --features bundle --package-types nsis     # Windows installer (or msi, with WiX)
dx bundle -p airlock-offline --release --desktop --features bundle --package-types nsis
```

Output is under `target/dx/<app>/bundle/`. `--features bundle` hides the console window on Windows. The target PC still needs WebView2. Releases are built by CI from a tag; see [docs/releasing.md](docs/releasing.md).

### Tests

```bash
# every crate except flow-tests and the vendored RPC client
cargo test --workspace --exclude flow-tests --exclude solana-client-wasm

# flow-tests: built transactions against the real programs in an in-process VM (LiteSVM).
# The program binaries are downloaded from devnet, or from mainnet for a program whose devnet
# build differs, and checked against pinned hashes.
node scripts/fetch-programs.mjs
cargo test -p flow-tests

# the extension
cd extension && npm test && npm run check
```

### Secrets

Keys for tests against live services come from [Doppler](https://www.doppler.com), project `airlock`, config `dev` (set in `doppler.yaml`). Log in once with `doppler login`, run `doppler setup` once in each checkout to pick up `doppler.yaml`, then run a command under `doppler run --`, which sets each secret as an environment variable:

```bash
# the Tokens API client against the real service; needs TOKENS_API_KEY
doppler run -- cargo test -p tokens-api --test live -- --ignored
```

Never put a key in a file in the repository. The app does not read these variables: it keeps the Tokens API key in Settings.

## The code

```mermaid
flowchart TD
  Online["apps/airlock<br/>online app"] --> Shell["app-ui<br/>shell, settings"]
  Airgap["apps/airlock-offline<br/>airgap app"] --> Shell
  Online --> OnlinePieces["app-online<br/>network, sync, submit, Portfolio, History, Stake, proposals"]
  Online --> Desktop["app-desktop<br/>tray, window, toasts, bridge"]
  Airgap --> Desktop
  Mobile["apps/airlock-mobile<br/>Android app"] --> Shell
  Mobile --> OnlinePieces
  Mobile --> MobileSvc["app-mobile<br/>Android services, Seed Vault, MWA"]
  MobileSvc --> Kit
  Shell --> Screens["app-ui-wallets, app-ui-book,<br/>app-ui-build, app-ui-sign<br/>screens"]
  Screens --> Kit["app-ui-kit<br/>actions, session, platform, widgets"]
  OnlinePieces --> Kit
  Desktop --> Kit
  Desktop --> BridgeHost["bridge-host<br/>(online only)"]
  Kit --> AppCore["app-core<br/>state, saving, logging"]
  Kit --> Flows["wallet-flows<br/>use cases"]
  OnlinePieces --> RPCc["wallet-rpc"]
  Flows --> Core["wallet-core<br/>domain, SolanaRpc port"]
  AppCore --> Core
  RPCc --> Core
  RPCc --> WasmClient["vendor/solana-client-wasm<br/>(browser only)"]
  Core --> Vault["wallet-vault"]
  Core --> QR["qr-protocol"]
  Core --> Codama["crates/codama"]
  Core --> Pack["idl-pack"]
  Kit --> HW["hw-protocol<br/>hardware wallet messages"]
  HW --> UR["ur-codec"]
  FlowTests["flow-tests"] --> Core
```

The apps are thin: each picks its tabs and services and hands them to the shared shell in `app-ui`. Airlock Offline does not link `app-online` or `wallet-rpc`, so it cannot talk to a node; the online app and the Android app do.

A screen collects input and shows state. To do something, it calls an action in `app-ui-kit` (or in `app-online`, for Stake and proposals), which reads the session, calls one use case in `wallet-flows`, and writes the result back to the vault, a form or the Sign screen. The screen crates do not talk to a node themselves. Every transaction, whatever screen or request it came from, is signed on the Sign screen (`Session::open_in_sign`).

- `wallet-core` is the domain: address book, keys, transactions, IDLs, wallet kinds, and the review of a transaction before it is signed. It defines the `SolanaRpc` port and opens no socket.
- `wallet-flows` holds the use cases. Each takes a `SolanaRpc` and plain data, and returns plain data, so it runs the same behind a screen and against the fake node in its tests.
- `wallet-rpc` implements `SolanaRpc` and is the only crate that talks to a node: Agave's `solana-client` on desktop, the vendored `solana-client-wasm` in a browser.
- `wallet-vault` seals whatever document it is given (Argon2id at 256 MiB and three passes, then XChaCha20-Poly1305) and wipes secrets. It does not know what a wallet is.
- `qr-protocol` is the QRS1 format. It knows bytes, not Solana.
- `qr-codec` draws and reads QR codes, with no dependencies. `qr-codec-wasm` builds its scanner into the WebAssembly module the camera loop runs.
- `hw-protocol` is what Ledger, Trezor and Keystone devices say, as bytes, with no I/O. `hw-transport` is the only crate that opens one: HID for a Ledger and USB for a Trezor on desktop, both over USB on Android. `ur-codec` is the BC-UR encoding of a Keystone's QR codes.
- `idl-pack` is the format of the compressed file of IDLs (`resources/idls`) that ships beside the app.
- `flow-tests` runs transactions built by `wallet-core` against the real on-chain programs in an in-process VM.

QRS1 carries a payload of one to 64 typed objects, written with wincode, as one QR code or a numbered set of codes of the same size: a transaction, its signatures, a context note that lists the IDLs it was read with, or any of them together, and also wallet keys and Vector key rotations. [docs/signing.md](docs/signing.md#the-pipeline) lists the kinds. A reader accepts the payload only when every frame is present and the CRC matches. The frame layout is documented on the `qr-protocol` crate.

### Layout

| Path | What it is |
| --- | --- |
| `apps/airlock` | The online app (desktop and web), with its `Dioxus.toml` and icons |
| `apps/airlock-offline` | The airgap desktop app, which opens no connection of its own |
| `apps/airlock-mobile` | The Android app, with its `Dioxus.toml`, manifest and activity (the Seed Vault and Mobile Wallet Adapter bridges) |
| `crates/app-ui` | The shared shell: navigation, settings, notices and toasts |
| `crates/app-ui-wallets`, `app-ui-book`, `app-ui-build`, `app-ui-sign` | Screens: wallets; address book and programs; build; sign and QR transfer |
| `crates/app-ui-kit` | Actions, the session and each screen's form, platform glue (files, camera, QR drawing), CSS and shared widgets such as the lock dialog |
| `crates/app-online` | The node monitor, balance sync, submitting and tracking, and the Portfolio, History, Stake and proposals screens |
| `crates/app-desktop` | Desktop services: tray, window, OS toasts, and the browser bridge listener and request modals |
| `crates/app-mobile` | Android services over JNI: the private folder, links, online or offline, the Seed Vault, USB hardware wallets and Mobile Wallet Adapter requests |
| `crates/app-core` | The app's state, saving and logging, with no UI and no network |
| `crates/wallet-core` | Domain. Groups: `book`, `keys`, `tx`, `idl`, `wallets`, `review`, `edit` (a transaction read back into Build's blocks), `template` (Build's templates, and the ones that ship), `transport` |
| `crates/wallet-flows` | Use cases: refresh, compile, simulate, send, stake, Squads, programmatic signers, Vector wallets, hardware wallets, templates, rebuilds for wallets without a key |
| `crates/wallet-rpc` | The node client, and the WebSocket feed of wallet activity |
| `crates/tokens-api` | Prices and token checks from the Tokens API, for the Portfolio |
| `crates/wallet-vault` | Password seal, secret wiping, and stores |
| `crates/qr-protocol` | QRS1 frames |
| `crates/ur-codec` | Uniform Resources (BC-UR): bytewords, fountain codes and CBOR, for Keystone's QR codes |
| `crates/hw-protocol`, `hw-transport` | Hardware wallets: what Ledger, Trezor and Keystone say, as bytes; and the only crate that opens a device (HID, USB, and a scripted fake) |
| `crates/idl-pack` | The compressed file of IDLs the app ships beside its binary |
| `crates/program-watch` | Checks the programs whose IDLs ship against the builds they were reviewed for. Run daily by CI, never shipped |
| `crates/qr-codec`, `qr-codec-wasm` | The QR encoder and camera decoder, and the WebAssembly module built from it (see its [README](crates/qr-codec/README.md)) |
| `crates/bridge-protocol` | The bridge's framing and message shapes, shared by the app and `bridge-host` |
| `crates/bridge-host` | Native messaging host for the extension |
| `crates/flow-tests` | Transactions and use cases run in an in-process Solana VM (programmatic, Vector, Squads, …) |
| `crates/codama` | The Codama dynamic client: reads IDLs, builds and explains instructions, for `wallet-core` (see its [README](crates/codama/README.md)) |
| `extension` | The browser extension |
| `resources/idls` | The IDL pack: third-party programs' IDLs, listed with their sources in `manifest.toml` |
| `resources/program-builds.toml` | Which build of each program its shipped IDL was reviewed against |
| `resources/linux` | udev rules for hardware wallets on Linux |
| `scripts` | Bundling, release, dependency-age and notices scripts |
| `vendor/solana-client-wasm` | The browser RPC client |

## Documentation

| Document | What it covers |
| --- | --- |
| [docs/signing.md](docs/signing.md) | How each wallet kind signs, starting in the app or from a site |
| [docs/browser-bridge.md](docs/browser-bridge.md) | The extension, `bridge-host` and the desktop listener, the message format and the threat model |
| [docs/architecture.md](docs/architecture.md) | The two builds, the core that outlives the window, logging, and where new work goes |
| [docs/vector-fork.md](docs/vector-fork.md) | The Vector program fork and deployment, how to rebase it, and the decisions and risks behind Vector wallets |
| [docs/idls.md](docs/idls.md) | How IDLs read and build instructions, the IDLs that ship with the app, and the rules that tie what is shown to what is signed |
| [docs/templates.md](docs/templates.md) | Templates on Build: the TOML format, how one is checked and expanded, and the templates that ship |
| [docs/android.md](docs/android.md) | Airlock for Android: how it is put together, the Seed Vault, Mobile Wallet Adapter, building and running |
| [docs/design-system.md](docs/design-system.md) | Tokens and components shared by the app and the extension popup |
| [docs/releasing.md](docs/releasing.md) | Tagging, building and publishing a release, and what each file of a release is |
| [SECURITY.md](SECURITY.md) | How to report a vulnerability, and what is in scope |
| [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) | Where each shipped IDL file comes from, and its licence |

## License

MIT. See [LICENSE](LICENSE).

The vendored RPC client under `vendor/solana-client-wasm` is Apache-2.0; see `vendor/ORIGIN.md`.

The IDL files under `crates/wallet-core/idls` and `resources/idls` are not under the MIT licence. They are other projects' interface descriptions, taken from their repositories or from the chain, or written by hand from their source, and then modified. Each keeps its source's terms. [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) lists every one with its source and that source's licence, and ships with each release.

## Disclaimer

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE, AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR CONTRIBUTORS BE LIABLE FOR ANY CLAIM, DAMAGES, OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT, OR OTHERWISE, ARISING FROM, OUT OF, OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

Use at your own risk. This code has not been formally audited. You — not the
authors — are responsible for any loss of funds, keys, or data resulting from
its use, misuse, or integration into other systems.
