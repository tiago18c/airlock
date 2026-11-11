# Architecture

How the app is put together: the two builds, the core that outlives the window, what each app crate owns, how the app logs, and where new work goes. The crates below the app (`wallet-core`, `wallet-flows`, `wallet-rpc` and the rest) are described in the README. How a transaction is signed is in [signing.md](signing.md).

## Decisions

| Question | Decision |
| --- | --- |
| How the airgap build is made | A separate app over the same UI crates, which have no network dependencies. No `cfg(feature = "offline")` in shared code. |
| What closing does | Hides the window and keeps the webview. Restoring is instant: the DOM, scroll positions and half-filled forms are still there. The cost is the webview's memory while hidden. |
| Auto-lock | Locks itself after a while hidden, or after the same time with no input while the window is open. A sign request that arrives while it is locked asks for the password when the window opens. |
| The web build | Stays. The core must therefore run without threads, and it gets no bridge, tray or OS notifications in the browser. |
| Logging | `tracing` everywhere. Libraries only emit. Each app installs the subscriber: a rolling file on desktop, the console on the web. |
| What logs may hold | Every transaction the app receives, transforms or sends, in base64, every signature, and full addresses: they are public, and they make a problem traceable on an explorer. Never a secret, and never the credentials in an RPC URL. |
| Notices | Every submitted transaction, browser request, lock and failed background job is a notice. Toasts and the notification center are two views of the same list, kept in the core and on disk, so nothing that happened while the window was hidden is lost. |

## Builds

```
┌──────────────────────────── app-core ────────────────────────────┐
│ Stores and saving · vault and auto-lock · published state        │
│ Commands · events · notices · ports (Spawner, Clock, Notifier,   │
│ Links) · logging setup · no Dioxus, no network · native and wasm │
└──────▲───────────────────────▲──────────────────────────▲────────┘
       │ commands, snapshots   │ commands (public only)   │ ports
┌──────┴────────┐   ┌──────────┴───────────┐   ┌──────────┴──────────┐
│ app-ui-kit,   │   │ app-online           │   │ app-desktop         │
│ app-ui-*,     │   │ wallet-rpc, network  │   │ bridge server, tray,│
│ app-ui        │   │ monitor, sync jobs,  │   │ window, OS toasts,  │
│ (no network)  │   │ online-only panes    │   │ single instance     │
│               │   │ (native and wasm)    │   │ (native only)       │
└───────────────┘   └──────────────────────┘   └─────────────────────┘

apps/airlock
  desktop: core + ui + online + desktop (with the bridge)
  web:     core + ui + online
apps/airlock-offline
  desktop: core + ui + desktop (no bridge)
apps/airlock-mobile
  android: core + ui + online + mobile (offline mode: core + ui + mobile, no Network)
```

Each app is a thin `main.rs`: it picks its tabs and services and hands them to `AppShell` in `app-ui`. `apps/airlock` picks its desktop services with `cfg(target_arch)`, which is the only `cfg` on build flavour the code allows. Each app has its own `Dioxus.toml` and icons, because `dx -p` reads them from the package directory.

## app-core

**State is published, not shared.** Each slice has one owner and goes out as a `tokio::sync::watch` of `Arc<T>` (`app_core::slices`). `watch` keeps only the latest value, which is what both the UI and the jobs want, and it builds on wasm. The slices:

| Slice | Holds |
| --- | --- |
| `wallets` | Public wallet data and summaries, addresses derived once |
| `book` | The address book |
| `holdings` | Token accounts, balances and market data |
| `network` | Cluster, mode, the airgap choice, probing, the RPC URL |
| `vault` | Uninitialized, locked, unlocked, or unreadable, never secrets |
| `settings` | Vector offered, the auto-lock timeout, close-to-tray, the tokens API key |
| `bridge` | Pending browser requests (`BridgeQueue`) |
| `notices` | What happened, newest first. Saved deferred to `notices.json` |
| `history` | The wallets' transactions, newest first, each read in full once. Saved deferred to `history.json` |
| `sync` | Each background job's last success, error and next retry |

**Commands change state.** Each command mutates under the slice's owner, publishes the new snapshot, and saves with one of two policies:

- **Checked** (user edits to the vault, the book and settings): save now, roll back and return the error if it fails.
- **Deferred** (caches that can be fetched again: holdings, balances, notices, history): mark the slice dirty. A saver task writes it at most once a second, off the UI thread, and reports a failure as an event without rolling back.

**Events are for things that happen once** (`app_core::events::Event`): `SaveFailed`, `SyncFailed`, `VaultLocked`, `RequestArrived`, `TxStatusChanged`. They go out on `tokio::sync::broadcast`.

**Ports keep the core free of platform code** (`app_core::ports`):

| Port | Native | Web | Tests |
| --- | --- | --- | --- |
| `Spawner` | `tokio::spawn` | `wasm_bindgen_futures::spawn_local` | current thread |
| `Clock` (sleep, now) | `tokio::time` | `gloo-timers` | a manual clock |
| `Notifier` | OS toasts (`app-desktop`) | none | a recorder |
| `Links` (open a URL outside the app) | the system browser | `window.open` with `noopener` | a recorder |

Storage is `app_core::storage`: files under the data directory on desktop, `localStorage` on the web. Every file is JSON, kept as text in both places. `workspace.json` holds what the user wrote: settings and the address book. `grants.json` holds which sites were given what, and `chain-cache.json` what a node said about the addresses. They are one `Workspace` in memory, and apart on disk because they do not share a fate: a sync writes only the last, and damage to one loses only that one. Damage to the grants means sites ask again, and to the cache that it is read again. A file from before they were apart holds all of it, and the first save moves it. `vault.json` is the encrypted vault: a header naming the key derivation (Argon2id, its cost and salt) and the cipher (XChaCha20-Poly1305, its nonce), and the ciphertext, all as `wallet_vault`'s seal writes them. The header is the cipher's associated data, so it cannot be changed without the password. The cost is what stands between a copied file and its password, so it is as high as a person will wait for, and a vault sealed at less is raised to it when it is next unlocked. Deriving a key takes about a second, so it runs apart from the vault's lock and off the window's thread: `begin_unlock`, then `Unlocking::open` anywhere, then `finish_unlock`. A file that cannot be read says nothing of what it holds, so the vault counts as unreadable and no new one can be made over it: `Vault::create` refuses a store that holds anything or cannot be read, and unlocking tries the file again. A file that turns out damaged is opened from the `.bak` of the save before, set aside as `.unreadable`, and written whole again, with a notice. Whether a vault is stored is read once and kept, not read on every call. The data directory is `%LOCALAPPDATA%\airlock` on Windows, `~/Library/Application Support/airlock` on macOS (the folder the bridge socket is in), and `$XDG_DATA_HOME/airlock` on Linux, or `~/.local/share/airlock` when that is not set (`bridge_protocol::data_dir`, which `bridge-host` reads too, since it logs there). Builds before kept it at `~/airlock` on Unix; the first start moves it, before anything in it is opened. On Unix only its owner may read it: the folder and every folder under it are `0700`, and every file the stores and the lock write is `0600`. Log files are made with the default mode, and the folder is what keeps them private. Airlock Offline keeps its own data directory (`airlock-offline`), apart from the online app's. `AIRLOCK_DATA_DIR` (`AIRLOCK_OFFLINE_DATA_DIR` for Airlock Offline) overrides it, so tests and smoke runs never touch the real one. Futures on wasm are not `Send`, so core futures are bounded by `MaybeSend` (`Send` on native, nothing on wasm).

**Secrets stay behind a narrow handle.** The vault, unlocking and locking it, and the record of what each hash-based key has signed with are the core's. A press of Sign runs in `app_ui_kit`, with the keys it asks the core for, for that transaction alone (`Session::local_signers_for`). The IDL catalog stays a signal of the window's, since nothing but a screen reads it. Services get a `PublicCore` handle that can read the published slices and run deferred commands. Its type has no way to reach key material, so a sync job could not touch a key even through a bug. `Core` dereferences to its `PublicCore`, so the commands both have are written once, and adds only the vault and what is the user's to do, such as answering a browser request.

**Auto-lock lives in the core**, so the airgap build gets it too. The app reports whether the window is visible and pings on input, and `LockPolicy` locks the vault once the window has been hidden for the timeout set in Settings, or has had no input for as long while visible (`reason=hidden` or `idle`). It sleeps at most 15 seconds at a time and checks the wall clock as well as the time slept, so time the machine spent asleep counts, a clock set back does not put the lock off, and a timeout changed in Settings applies to the wait already running. Locking zeroizes the secrets and publishes the locked state. The window follows that state, however the vault locked: a key or phrase being typed or shown on Add wallet is wiped and the modal closes. Two kinds of key stay until the app closes, because a transaction in progress can wait on them across a lock: a keypair Build made for an account, and a key made in place of a site's one-time key. They sign nothing while the vault is locked, and they control only an account the transaction creates.

**The window is held to the app.** A Content Security Policy in the page (`app_ui_kit::platform::page_policy`, put in by the desktop and Android shells) lets it load only the app's own files and make requests only to the loopback websocket Dioxus sends its edits on. A second policy, added once the page's own scripts have been read, stops inline scripts, and a navigation handler keeps another page from replacing it. It is not a seal: [the threat model](browser-bridge.md#threats) has what it leaves.

**Slow work and the window.** Deriving a key and reading a transaction with its IDLs run away from the window (`app_ui_kit::rt::off_the_window`). The catalog itself stays in a signal on the window's thread, and `IdlStore::view` gives a copy of it that can go to another thread, sharing the clients already parsed. Clients are kept by program and checksum, so a view made before an IDL changed cannot put the old one back.

**Locks and slices.** The core's locks have an order (`app_core::ranked`), and a debug build panics when one is taken while a later one is held. A slice goes out only when its value changed: a reader is not woken and nothing draws again for a write that changed nothing, and a large value is copied only then.

**Asking a node.** A book can hold more addresses than a node will answer for at once, so a background read of token accounts takes turns (`wallet_flows::refresh::TokenTurns`): at most 20 owners, two requests each, wallets first and more often. A node that says it is asked too often (`RpcError::RateLimited`) or does not answer stops the read, what was read is kept, and the next read goes on from there, after the scheduler's backoff. What a node refuses for one address is reported rather than swallowed. Refresh reads every address, and says how far it got.

**What runs while locked.** Background jobs work while the vault is locked, so what they sync comes from public data, never from the vault. Each time the vault is open and changes, the core copies its wallets without their secrets to `wallets.json` (`WalletListStore`). While it is locked, the `wallets` slice, routing (`Core::with_wallets`) and the sync targets come from that copy, so the wallets, their balances and their settings stay on screen. The copy cannot sign. Choosing the wallet to act as is the one change kept in it while locked; the vault takes the choice at the next unlock. Every other change to a wallet, and signing, asks for the password (`Session::when_unlocked`).

## The UI crates

`app-ui-kit` holds what every screen shares: the session and each screen's form, the actions screens call, platform glue (files, the camera, QR drawing), the CSS and the shared widgets. The screens are in `app-ui-wallets`, `app-ui-book` (address book and programs), `app-ui-build` and `app-ui-sign` (sign and QR transfer). `app-ui` is the shell over them: navigation, settings, notices and toasts.

- **The pump.** `use_core(core)` (`app-ui-kit`, `state/pump.rs`) starts one task per slice. Each task waits on `changed()` and writes a `Signal<Arc<T>>`. It is the only code that writes those signals, so the core never touches Dioxus. Screens read the slices as `ReadSignal`s, with memos on top.
- **Screen state** (drafts, tabs, the Sign view, filters) stays in plain Dioxus signals. It belongs to the window, and it survives hiding because the webview does.
- **Optional capabilities.** A shared pane does its online parts through `use_network() -> Option<Network>`, a context the online app provides. `Network` carries a `dyn SolanaRpc` and the network status. Without it, which is how the airgap build works and how the online build behaves while offline: Sign has no Simulate, Submit, Refresh or Rebuild; Wallets has no balances or Vector account state; Build takes its lifetime from a durable nonce.
- **The app builds the navigation.** Panes only one build has, and slots only the online build fills (a balances column, the network and sync chips), are passed in by the app. They are not branches inside shared panes.
- **`ToastHost`**, mounted once beside the panes, shows the notices whose toast is still up. The notification center lists them all.

## app-online

Native and wasm:

- **`wallet-rpc` as the `SolanaRpc` implementation.**
- **`network::Monitor`:** probes the node, corrects the cluster, and publishes the network status.
- **`tx::Submitter` and `tx::Tracker`:** the only way a transaction is sent. Submitting logs it, adds a notice and hands the signature to the tracker, which follows it to the end on the cluster it was sent to.
- **`sync::Scheduler`:** jobs with an interval, backoff and a cancellation token (SOL balances, the token accounts of the wallet in use, those of every other address, prices, transaction history). A cluster change, a new RPC URL or the app going offline cancels and restarts them. Each job reads public slices and writes through deferred commands.
- **Prices and token logos** (`sync::prices`, `sync::logos`): while a Tokens API key is set and the cluster is mainnet, the prices job sends the Tokens API (`api.tokens.xyz`, through `tokens-api`) the mints held by the wallets and the watched addresses, never an address or an amount. Each price answer can name a token's logo (`logoURI`). Once the prices are applied, the job downloads the logos it has not asked for yet, at most 8 a run, one at a time, from whatever host the answer names: the Tokens API's own storage (`storage.googleapis.com`) or a third party's, such as `static.jup.ag`. Only the URL is sent: the request goes through a client of its own, with no key, cookie, `Referer` or header of ours, over `https://` only, with at most 3 redirects, a 10 second timeout and 128 KiB. Only a PNG, JPEG or WebP, told from its first bytes, is kept. A logo is downloaded once per mint, ever: the file goes to `logos/<mint>.<png|jpg|webp>` in the data folder, and `holdings.json` records that it was asked for, kept or not, so it is never asked again. One whose host could not be reached is tried once more while the app runs. A token the Tokens API thinks impersonates another gets none. The page shows a kept logo as a `data:` URL, read from its file once per run, and never loads a remote image. The browser build downloads no logos.
- **The activity feed** (`sync::activity`): one WebSocket to the node, with an account and a logs subscription per wallet. A pushed balance is shown at once. A transaction that mentions a wallet, or one the app sent landing, has the balances job read again from a node that has seen its slot. While the feed is up, the balances job runs only now and then, in case it missed something.
- **Transaction history** (`sync::history`): the latest transactions of each wallet (`getSignaturesForAddress`), merged into the `history` slice, then those not yet read in full (`getTransaction`), newest first and a few at a time. What is read is kept, so a transaction is read from a node once, and one the node does not have is asked for only a few times. The activity feed, a transaction landing and a new wallet run it at once. Older pages are the History pane's to ask for (`sync::load_older`): each wallet's next page before the oldest it keeps, read in full at once. A page shorter than asked marks where that wallet's history starts, so nothing older is asked for. The submitter puts what it sends in the history as it sends it, read from its own bytes, and the tracker's last word settles it: kept with its outcome once it lands, dropped if it never did.
- **Online-only panes:** Portfolio, Stake, History and Proposals. History opens a transaction as Review and Sign shows one, with what it did to each balance. Submit on Review and Sign waits on screen for the transaction to land, then offers History or a clear screen, or the same transaction again when it did not land.

## app-desktop

The native services:

- **The bridge server** (online build only, the `bridge` feature). It binds the local socket through `bridge-host`'s endpoint and relay code and feeds the `BridgeQueue` slice. When a request arrives it publishes the queue, sends `RequestArrived`, and the app notifies and asks for attention. The browser request modals are here too.
- **Tray and window.** Close hides the window when close-to-tray is on; the tray menu has Open, Lock now and Quit; the window reports when it is shown or hidden.
- **Single instance.** Each app locks its data folder (`airlock.lock`, `app_core::storage::claim_data_dir`) before any store opens, and refuses to start when another process holds it: every store keeps its file in memory and writes it whole, so two processes would undo each other's saves, including the record of used XMSS leaves. The online app also binds the bridge's local socket first: a second launch that cannot bind it sends `show` to the running app and exits.
- **The `Notifier`** (OS toasts). Clicking one shows the window on what it is about.

## app-mobile

The Android services, for `apps/airlock-mobile`. [android.md](android.md) has the whole of it.

- **The private folder.** `prepare()` puts the data folder under `Context.getNoBackupFilesDir()` (`app_core::storage::set_base_dir`) before anything opens, and picks the mode's folder: online and offline each keep their own vault.
- **Links** open through an Android intent (`AndroidLinks`, handed to `open_core_on`), `https` and `http` only.
- **The phone's device hub** opens no USB device, and has the phone's Seed Vault when it has one (`DeviceHub::seed_vault`).
- **Lifecycle.** The app in the background counts as a hidden window for auto-lock.

## Logging

1. **One facade.** Every crate logs through `tracing`. `eprintln!` is left only in `bridge-host`'s `main.rs`, for text a person reads in a terminal.
2. **Libraries emit, apps subscribe.** The subscriber is installed by each app's `main` through `app_core::diagnostics::init(LogConfig)`, before `dioxus::launch`.
3. **Targets name the area.**

| Target | What it covers |
| --- | --- |
| `airlock::app` | startup, shutdown, window shown or hidden, tray |
| `airlock::core` | commands (name, outcome), saves (store, policy, result), auto-lock |
| `airlock::vault` | unlock, lock (with the reason), never anything inside the vault |
| `airlock::network` | probes, cluster corrections, mode changes |
| `airlock::bridge` | requests in and out, by request code, method and origin |
| `airlock::device` | a hardware wallet ready (vendor, model, app version), a signature made (path, pubkey), a refusal and why; never a key it does not show |
| `airlock::tx` | every transaction stage, with its wire and signatures |
| `airlock::sync` | job start, end, backoff, cancellation |
| `airlock::ui` | UI-side errors that are not shown to the user |
| `perf` | probe events, at `trace` |

4. **Levels.** `error`: something failed that the user did not see. `warn`: something failed and was retried or shown. `info`: lifecycle and state changes a support request would ask about. `debug`: commands and requests in detail. `trace`: `perf` and per-render detail. The default is `info`. `AIRLOCK_LOG` (`AIRLOCK_OFFLINE_LOG` for Airlock Offline; `EnvFilter` syntax, for example `airlock::bridge=debug,info`) overrides it. Settings → Diagnostic logging sets `debug` until the app restarts.
5. **Public data is logged in full.** Transactions (base64 wires), signatures (base58) and addresses (full base58, never shortened) are on chain or about to be, and the log is where they are traced.
6. **What must never be logged:** passwords, seed phrases, private keys, Vector secret keys or hash-based key state, vault bytes, and the credentials in an RPC URL (its user info and query string; URLs go through `redact_url`). Fields are logged one by one; a struct that might hold a secret is never logged with `?value`. Every type that holds key material has a `Debug` that hides it.
7. **Spans carry the context:** `command{name}`, `bridge_request{code, method}`, `tx{flow}`, `sync_job{job, run}`, `probe{cluster}`.

### Transactions in the log

Every transaction gets a **flow id** when it enters the app (`app_core::diagnostics::tx`), carried in a `tx{flow}` span through every later stage, even after a rebuild changes its message. Each stage is one `info` line under `airlock::tx` with the stage, the flow id, the hex SHA-256 of the message (so the lines of one message can be matched across flows), the whole wire in base64, the signatures present, and the cluster.

| Stage | Extra fields |
| --- | --- |
| `received` | `source` (build, wallets, stake, pasted, scanned, browser; Proposals logs as wallets), and for a browser request `origin` and `code` |
| `transformed` | `transform` (a Squads, programmatic or Vector rebuild, the conversion to v1, a payer swap, a blockhash refresh, `squads_checks` for what a Squads proposal holds added to its approval), `from_message` |
| `signed` | `by` (hot key, Vector key, pasted, imported) |
| `returned` | `origin`, `code` |
| `sent` | `signature` |
| `status` | `signature`, `status` (confirmed, failed, expired, unknown), `error`, `slot` |
| `rejected` | `error`: sending failed before the node took it |

`rg "flow=<id>" <log>` shows one transaction's whole path.

### Where logs go

| Build | Sink |
| --- | --- |
| Desktop (online and airgap) | `<data dir>/logs/airlock.YYYY-MM-DD.log`, rotated daily, the last 7 files kept, plus stderr in debug builds |
| Web | the browser console |
| `bridge-host` | `<data dir>/logs/bridge-host.YYYY-MM-DD.log`. Its stdout carries native messaging, so nothing else may write there. |

Settings has **Open log folder** (desktop) and **Copy diagnostics**: the version, build flavour, OS, data dir and the last log lines, for a bug report. The logs hold the user's addresses and transaction history, so Copy diagnostics says so before it copies.

## Where new work goes

| Feature | Lives in |
| --- | --- |
| Data kept fresh in the background | an `app-online::sync` job writing through deferred commands |
| Transaction history | the `history` store and slice in `app-core` (`app_core::history`), filled by the `history` job in `app-online::sync` and by `tx::Submitter`, shown by the History pane in `app-online` |
| A new kind of notice | a `NoticeKind` variant in `app-core`, pushed by whoever knows it happened |
| Sending a transaction from a new place | `app-online::tx::Submitter`, never `send_transaction` directly, so every send is logged, noticed and tracked |
| OS notifications for anything | a notice or event in `app-core`, a rule in the app, the toast in `app-desktop` |
| Browser bridge methods | `bridge-protocol` (types), `app-desktop::bridge` (transport and modals), `BridgeQueue` in `app-core` |
| A pane only the online build has | `app-online` (or `app-desktop` if native only), mounted by the online app |
| A new online action inside a shared pane | `wallet-flows` (the logic, over `SolanaRpc`) plus `use_network()` in the pane |
| Logging for a new area | a new `airlock::<area>` target, added to the table above |
| A template that ships with the app | a file in `crates/wallet-core/templates/`, listed in `BUILTIN_TEMPLATES`, with its flow tests: see [templates.md](templates.md#adding-a-shipped-template) |

The rules behind the table:

1. `app-core` and the UI crates never depend on `wallet-rpc`, `bridge-host`, `interprocess` or an HTTP or websocket crate. CI checks this with `scripts/check-airgap-deps.sh` on `airlock-offline`.
2. `app-core` and the UI crates never depend on `hw-transport`, the only crate that opens a device. Each shell hands `hw_transport::hub()` to `open_core`, and the core keeps it in `Ports::devices`. The jobs on a device are in `app-ui-kit`'s `actions::device`, over `wallet_flows::device`, and show in one dialog the shell mounts (`ui::DeviceDialog`).
3. Logic that asks a node something belongs in `wallet-flows` over `SolanaRpc`. `app-online` schedules it, and the UI calls it through `Network`.
4. Background work runs in the core's `Spawner`, never in a Dioxus scope. `spawn_app_task` is only for UI follow-up work that must not stop when a screen is left.
5. Only the pump writes slice signals. Services change state through commands.
6. Services get `PublicCore`. Only UI actions the user started reach the vault's secrets.
7. A cache that can be fetched again is saved deferred. Anything only the user can recreate is saved checked.
8. Everything logs through `tracing` under the rules above, and every transaction stage is logged under `airlock::tx`.
9. Transactions are sent only through `tx::Submitter`.
