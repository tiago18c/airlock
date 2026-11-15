# Intents test page

A page that tries every method of Airlock's intents proposal ([docs/intents.md](../../docs/intents.md)) against a wallet, one row per method, in the manner of the wallet adapter's example page. It finds wallets through the Wallet Standard and lists those with `airlock:resolve`. A wallet that has only `solana:signAndSendTransaction` is listed too, and then only the last row, the fallback, can run.

The rows:

| # | Id | What it sends | A pass |
| --- | --- | --- | --- |
| 1 | `profile` | `getSigningProfile` for the account | A profile comes back. It says which rows below to expect a refusal on |
| 2 | `memo` | One memo the account signs and pays for, no notes | `done` |
| 3 | `transfer` | 0.001 SOL from the account to the app key, no notes | `done` |
| 4 | `mint` | Classic: a new mint (`new`, replaceable) and its token account (`derived`). Token-2022: the same with a metadata pointer that names the mint, so the mint's address is in `InitializeMetadataPointer`'s data, declared by a `data` entry at byte 34 | `done`, the mint and token account read back from the node, and for Token-2022 the pointer names the mint Airlock used |
| 5 | `cosign` | A memo whose fee the app key pays (`feePayer`, `signs: "app"`, replaceable or not), delivery `cosign` | The page checks what came back, signs as the sponsor, and it ends `done` |
| 6 | `return` | A memo the app key signs too (`signer`), delivery `return` | The page checks it, signs at the indexes `awaiting` names, sends it, and the node confirms it |
| 7 | `order` | Two memos, `serial`, `any` or `atomic` | `done` for `serial` and `any`; for `atomic`, a refusal `unsupported` (`order`) |
| 8 | `lying` | A `derived` note whose seeds give another address | A refusal `bad_notes` |
| 9 | `account-note` | The memo of row 2 with a `feePayer` note on the account acted as | A refusal `bad_notes`: the account acted as carries no note |
| 10 | `message` | A message, mode `signature`, `memo` or the wallet's choice | The signature verifies, or the memo transaction read from the node holds the message. The page names its chain, so a memo is on it (`resolution.chain`) whatever node Airlock reads |
| 11 | `fallback` | The memo of row 2 through `solana:signAndSendTransaction` | The node confirms it |

A refusal a row hopes for shows as a pass, with its code, its `requirement` when there is one, and Airlock's message. Rows that pay their own fee send no fee payer note: who pays is the wallet's to choose, and the comparison of what comes back (`src/compare.ts`) lets the fee payer change only then, or when a `feePayer` note says `replaceable`, and every instruction must be as sent.

Below the rows, a live table of every resolution the page started (kept in this browser, so it survives a reload), and a log of every call, reply, push and error. A resolution Airlock no longer has (`not_found`) shows as not found, with a button to forget it.

## Build and serve

```bash
cd examples/intents
npm install
npm run check     # tsc --noEmit
npm test          # the comparison, the wire reader, the memo check, the instructions, the relay
npm run build     # bundles to dist/, with Airlock's tokens.css beside it
npm run serve     # http://127.0.0.1:8124/, loopback only; a port as argument or PORT changes it
```

A content script does not run on `file://`, so the page has to be served for the extension to find it. Everything it loads comes from `dist/`. The only other addresses it talks to are the RPC URL chosen in Settings, and the explorer links you click.

## Point the page and Airlock at the same network

Settings has Devnet, Mainnet and Custom RPC. A custom RPC also takes the chain it is named as in requests (`solana:devnet`, `solana:testnet` or `solana:mainnet`): a local validator at `http://127.0.0.1:8899` is usually named `solana:devnet`. The page builds and reads transactions through that URL, and Airlock sends through its own node, so give Airlock the same network in its Settings (the same custom RPC for a local validator). Otherwise a blockhash the page fetched is unknown to the node Airlock sends through.

The **app key** is a throwaway keypair the page makes and keeps in `localStorage` in plain text. It stands for an app's own fee sponsor in row 5 and its own signer in row 6. Fund it with its Airdrop button on devnet or a local validator, and never send it real funds.

## A dev transport for end-to-end tests

Opened with `?wallet=ws://127.0.0.1:<port>`, the page registers a wallet of its own instead of waiting for the extension: the extension's own `AirlockWallet` class (`extension/src/wallet.ts`), whose calls go as JSON over that WebSocket. Only `ws://` to `127.0.0.1`, `localhost` or `[::1]` is accepted. A relay on the other end passes the calls to the app over the browser bridge and its replies back. Without the parameter the page does nothing of this.

Each message is one WebSocket text frame of JSON.

Page to relay, one per call:

```json
{ "id": "<request id>", "method": "<bridge method>", "params": { } }
```

`method` is a bridge method of [browser-bridge.md](../../docs/browser-bridge.md#messages) or of [intents.md](../../docs/intents.md#airlocks-prototype-the-bridge-messages) (`connect`, `getSigningProfile`, `resolve`, `getResolution`, `provideSignatures`, `cancelResolution`, `resolveMessage`, `signAndSendTransaction`, …), and `params` are as those tables give them. The page sends no `v`, `origin` or `expiresAt`: the relay adds them as the extension's service worker does, with `origin` the page's own (`http://127.0.0.1:8124` by default).

Relay to page, one per call, with the call's `id`:

```json
{ "id": "<request id>", "result": { } }
{ "id": "<request id>", "error": { "code": "unsupported", "message": "…", "requirement": "delivery" } }
```

`error` is passed on whole, so `requirement` reaches the page.

A push from the app (a message with no `id`), for the page's origin only:

```json
{ "push": { "v": 1, "method": "resolutionChanged", "origin": "http://127.0.0.1:8124", "params": { "resolution": { } } } }
```

The page hands `push` to the wallet as the content script would: `resolutionChanged` reaches `onResolution` and settles a `resolve` waiting on the person, and `accountsChanged` changes the accounts. A call the relay never answers is given up on as the content script does: 15 seconds for one nobody decides, a little over ten minutes for one that waits for the person.

## End to end: the app, the relay and the page

`relay.mjs` is the other end of the dev transport. It starts the app's native messaging host, `bridge-host`, and does what the extension's service worker does: it adds `v`, an id of its own, the page's origin (from the WebSocket handshake, and only a page served from this machine) and `expiresAt` to each call, sends each reply back under the page's id, passes `resolutionChanged` and `accountsChanged` to the pages of their origin, tells the app which origins have a page open (`pages`), and cancels what a page that closed was waiting on (`cancel`). It listens on 127.0.0.1 only and logs every frame, one line each, unless given `--quiet`.

```bash
cargo build -p airlock -p bridge-host
# 1. The app, on a data folder and a pipe of its own, so an Airlock already running is left alone
#    (as scripts/drive-desktop.mjs starts it). Give it a vault and a wallet, and the network of step 3.
# 2. The relay, with the same USERNAME, LOCALAPPDATA and AIRLOCK_DATA_DIR as the app:
node examples/intents/relay.mjs 8125 target/debug/bridge-host.exe      # or BRIDGE_HOST=...
# 3. The page, built and served, opened with ?wallet=ws://127.0.0.1:8125:
cd examples/intents && npm run build && npm run serve
#    http://127.0.0.1:8124/?wallet=ws://127.0.0.1:8125
```

`drive-page.mjs` drives the page in headless Edge or Chrome instead of a person's browser. It serves `dist/` itself, connects (pressing Connect when a silent connect gives no accounts, so the connection can be approved in the app), picks the account, sets the node and chain, then presses each row's button in turn and waits for the row to settle. It only waits: for the app's answers and for whoever approves in the app's window, never for a set time.

```bash
node drive-page.mjs <outdir> --relay 8125 --rpc http://127.0.0.1:8899 --chain solana:devnet --account 0
node drive-page.mjs <outdir> --fake        # its own relay, with test/fake-app.mjs as the app and the node
```

| Option | Default | |
| --- | --- | --- |
| `--relay <port>` or `--fake` | | One of the two |
| `--only <ids>` | every run | Run ids or row ids, comma separated: `mint` runs `mint:classic` and `mint:token-2022` |
| `--rpc <url>` | `http://127.0.0.1:8899`, or the fake's node | |
| `--chain <id>` | `solana:devnet` | |
| `--account <index or label>` | `0` | |
| `--timeout <seconds>` | `300` | For one row, and for the connect |
| `--browser <path>` | Edge, then Chrome, where they are usually installed, or `BROWSER` | |
| `--airdrop` | off | Asks the node for 1 SOL for the app key before the rows: a local validator starts it empty |

The runs: `profile`, `memo`, `transfer`, `mint:classic`, `mint:token-2022`, `cosign:kept`, `cosign:replaceable`, `return`, `order:serial`, `order:any`, `order:atomic`, `lying`, `account-note`, `message:auto`, `message:signature`, `message:memo`, `fallback`.

It writes to `<outdir>`: `page-light.png` and `page-dark.png` (the whole page), `settings.png`, `resolutions.png`, `rows/<run>.png` (each row as it settled), and `outcome.json`: the page's address, the relay port, the node and chain, the account, how many passed and failed, and for each run its `id`, `row`, the options it set, `pass` (true, false, or null when the row could not run), the verdict's text, the expectation shown above the row, every step, `timedOut`, how long it took and its screenshot; then the resolutions table, the page's event log, `problems` (exceptions, console errors, and any request that left the machine) and whether the page scrolls sideways. With `--fake` it also writes `relay.log`. It exits 0 when every run passed and the page had no problems, 1 otherwise, and 2 when it could not run. Each run uses a new browser profile in the temp folder, and stops every process started with it when done.

## All of it in one command, against the real app

On Windows, `scripts/drive-intents-e2e.mjs` runs every row against the real desktop app and a local validator, from built binaries, with nothing else running:

```bash
cargo build -p airlock -p bridge-host
(cd examples/intents && npm install && npm run build)
node scripts/drive-intents-e2e.mjs <outdir> [--wallet <kind>] [--only <ids>] [--keep] [--hold <seconds>]
```

It starts `surfpool` offline (System, Memo, Token, Token-2022 and the associated token program are there without a remote node) with three new random keys funded, and plants Squads v4 (with its program config) and the programmatic signer's three programs from the binaries the flow tests run (`node scripts/fetch-programs.mjs` downloads them); the debug app on a data folder, WebView2 profile and bridge pipe of its own, with a vault, the first key as the hot wallet Alice and the node as its custom RPC; the wallets of the kind asked for; the relay to the app's `bridge-host`; an approver that plays the person in the app's window; whoever else signs; and `drive-page.mjs --airdrop` against the relay, with the node named `solana:devnet` and the kind's account chosen. The approver (`startApprover` in `scripts/drive-lib.mjs`) approves the connect, presses Sign in each request modal once it is armed, signs each message, and takes Review and signs on Review and Sign (every press it needs, an authorization and then the transaction) when the modal offers no Sign or the kind asks for Review. Once the page is done, every transaction each resolution made, and each one it says ran, is read back from the node.

`--wallet` picks the account the page acts as. Each kind sets up its wallets through the app, as a person does, runs the rows that make sense for it, and plays the other parties:

| Kind | Account | Rows | Who else signs | About |
| --- | --- | --- | --- | --- |
| `hot` (the default) | Alice, a hot key | every row; `cosign:replaceable` and `return` through Review | nobody | 1 minute |
| `multisig-alone` | a Squads v4 multisig of one member, Alice, made on Wallets | every row: each request is one transaction that creates, approves and executes a proposal. A new mint lands at an ephemeral signer of that proposal, which the script checks on the node | nobody | 1 minute |
| `multisig-waits` | a Squads v4 multisig of two, Alice and Bob, both hot keys, Alice acting, nothing ticked under "Approves together with" | every row but the fallback, the mints included: the multisig's program signs for a new mint at an ephemeral signer of its proposal, so it waits for Bob like the rest, and the script checks that the node holds the mint at the address `replaced` gave; then `profile`, `memo` and `message:memo` with Bob ticked (the account turns `sync`); then a `memo` Bob rejects, and one Alice and Bob approve and cancel | Bob: on the Proposals screen, made the acting member in the multisig's Settings, he approves and executes each proposal once the page has been shot waiting; a rejection or a cancellation he sends from another wallet, straight to the node | 4 minutes |
| `offline` | Olivia, an offline wallet: this script holds her key | every row but the fallback | Olivia's device: the script reads the transaction from Review and Sign's Share (Transaction, base64), signs its message with her key and pastes a signature object into Add a signature | 1 minute |
| `programmatic` | a programmatic signer whose authority is Alice, made on Wallets, with its executor nonce account | every row | nobody | 2 minutes |

It writes to `<outdir>` what `drive-page.mjs` writes, one folder per phase when a kind has several, with `waiting/` (each row whose resolution waited, and the Resolutions table, shot as it waited); and `modals/` (each modal the approver acted on, and Review and Sign before and after signing, named by the page run), `others/` (the app's window as the other parties acted: the Proposals screen, Review and Sign waiting for another device, Share), `approver.json`, `chain.json` (each resolution, the states it went through, and each transaction it made or ran as the node returns it, in each encoding), `summary.json` (the profiles the app reported, what the other parties did, and each check of the kind with whether it held), and the logs of the node, the app, the relay and the page driver. It stops everything it started, also when it fails, and exits 0 only when every run passed, the approver got past every modal, the node holds every transaction and every check of the kind held. `--only` narrows each phase to what `drive-page.mjs` takes, `--keep` leaves the run's temp folder, and `--hold` keeps everything up that many seconds after the page is done, to look at the node or the app.
