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
| `sync` | Each background job's last success, error and next retry |

**Commands change state.** Each command mutates under the slice's owner, publishes the new snapshot, and saves with one of two policies:

- **Checked** (user edits to the vault, the book and settings): save now, roll back and return the error if it fails.
- **Deferred** (caches that can be fetched again: holdings, balances, notices): mark the slice dirty. A saver task writes it at most once a second, off the UI thread, and reports a failure as an event without rolling back.

**Events are for things that happen once** (`app_core::events::Event`): `SaveFailed`, `SyncFailed`, `VaultLocked`, `RequestArrived`, `TxStatusChanged`. They go out on `tokio::sync::broadcast`.

**Ports keep the core free of platform code** (`app_core::ports`):

| Port | Native | Web | Tests |
| --- | --- | --- | --- |
| `Spawner` | `tokio::spawn` | `wasm_bindgen_futures::spawn_local` | current thread |
| `Clock` (sleep, now) | `tokio::time` | `gloo-timers` | a manual clock |
| `Notifier` | OS toasts (`app-desktop`) | none | a recorder |
| `Links` (open a URL outside the app) | the system browser | `window.open` with `noopener` | a recorder |

Storage is `app_core::storage`: files under the data directory on desktop, `localStorage` on the web. Every file is JSON, kept as text in both places. `vault.json` is the encrypted vault: a header naming the key derivation (Argon2id, its cost and salt) and the cipher (XChaCha20-Poly1305, its nonce), and the ciphertext, all as `wallet_vault`'s seal writes them. The header is the cipher's associated data, so it cannot be changed without the password. A file that cannot be read says nothing of what it holds, so the vault counts as unreadable and no new one can be made over it: `Vault::create` refuses a store that holds anything or cannot be read, and unlocking tries the file again. A file that turns out damaged is opened from the `.bak` of the save before, set aside as `.unreadable`, and written whole again, with a notice. Whether a vault is stored is read once and kept, not read on every call. Airlock Offline keeps its own data directory (`airlock-offline`), apart from the online app's. `AIRLOCK_DATA_DIR` (`AIRLOCK_OFFLINE_DATA_DIR` for Airlock Offline) overrides it, so tests and smoke runs never touch the real one. Futures on wasm are not `Send`, so core futures are bounded by `MaybeSend` (`Send` on native, nothing on wasm).

**Secrets stay behind a narrow handle.** The vault, unlocking and locking it, and signing are core commands. Services get a `PublicCore` handle that can read the published slices and run deferred commands. Its type has no way to reach key material, so a sync job could not touch a key even through a bug.

**Auto-lock lives in the core**, so the airgap build gets it too. The app reports whether the window is visible and pings on input, and `LockPolicy` locks the vault once the window has been hidden for the timeout set in Settings, or has had no input for as long while visible (`reason=hidden` or `idle`). Locking zeroizes the secrets and publishes the locked state. The window follows that state, however the vault locked: a key or phrase being typed or shown on Add wallet is wiped and the modal closes. Two kinds of key stay until the app closes, because a transaction in progress can wait on them across a lock: a keypair Build made for an account, and a key made in place of a site's one-time key. They sign nothing while the vault is locked, and they control only an account the transaction creates.

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
- **`sync::Scheduler`:** jobs with an interval, backoff and a cancellation token (SOL balances, the token accounts of the wallet in use, those of every other address, prices). A cluster change, a new RPC URL or the app going offline cancels and restarts them. Each job reads public slices and writes through deferred commands.
- **The activity feed** (`sync::activity`): one WebSocket to the node, with an account and a logs subscription per wallet. A pushed balance is shown at once. A transaction that mentions a wallet, or one the app sent landing, has the balances job read again from a node that has seen its slot. While the feed is up, the balances job runs only now and then, in case it missed something.
- **Online-only panes:** Portfolio, Stake and Proposals.

## app-desktop

The native services:

- **The bridge server** (online build only, the `bridge` feature). It binds the local socket through `bridge-host`'s endpoint and relay code and feeds the `BridgeQueue` slice. When a request arrives it publishes the queue, sends `RequestArrived`, and the app notifies and asks for attention. The browser request modals are here too.
- **Tray and window.** Close hides the window when close-to-tray is on; the tray menu has Open, Lock now and Quit; the window reports when it is shown or hidden.
- **Single instance.** Each app locks its data folder (`airlock.lock`, `app_core::storage::claim_data_dir`) before any store opens, and refuses to start when another process holds it: every store keeps its file in memory and writes it whole, so two processes would undo each other's saves, including the record of used XMSS leaves. The online app also binds the bridge's local socket first: a second launch that cannot bind it sends `show` to the running app and exits.
- **The `Notifier`** (OS toasts). Clicking one shows the window on what it is about.

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
| `transformed` | `transform` (a Squads, programmatic or Vector rebuild, the conversion to v1, a payer swap, a blockhash refresh), `from_message` |
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
| Transaction history | a store and slice in `app-core`, filled by an `app-online::sync` job, so the airgap build can show what it signed |
| A new kind of notice | a `NoticeKind` variant in `app-core`, pushed by whoever knows it happened |
| Sending a transaction from a new place | `app-online::tx::Submitter`, never `send_transaction` directly, so every send is logged, noticed and tracked |
| OS notifications for anything | a notice or event in `app-core`, a rule in the app, the toast in `app-desktop` |
| Browser bridge methods | `bridge-protocol` (types), `app-desktop::bridge` (transport and modals), `BridgeQueue` in `app-core` |
| A pane only the online build has | `app-online` (or `app-desktop` if native only), mounted by the online app |
| A new online action inside a shared pane | `wallet-flows` (the logic, over `SolanaRpc`) plus `use_network()` in the pane |
| Logging for a new area | a new `airlock::<area>` target, added to the table above |

The rules behind the table:

1. `app-core` and the UI crates never depend on `wallet-rpc`, `bridge-host`, `interprocess` or an HTTP or websocket crate. CI checks this with `scripts/check-airgap-deps.sh` on `airlock-offline`.
2. Logic that asks a node something belongs in `wallet-flows` over `SolanaRpc`. `app-online` schedules it, and the UI calls it through `Network`.
3. Background work runs in the core's `Spawner`, never in a Dioxus scope. `spawn_app_task` is only for UI follow-up work that must not stop when a screen is left.
4. Only the pump writes slice signals. Services change state through commands.
5. Services get `PublicCore`. Only UI actions the user started reach the vault's secrets.
6. A cache that can be fetched again is saved deferred. Anything only the user can recreate is saved checked.
7. Everything logs through `tracing` under the rules above, and every transaction stage is logged under `airlock::tx`.
8. Transactions are sent only through `tx::Submitter`.
