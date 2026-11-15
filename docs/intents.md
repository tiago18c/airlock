# Intents: a request the wallet resolves

Status: **a draft proposal, with a prototype in Airlock on desktop** (2026-11-15). The plan it came from, the findings behind it and the prior art are in [plan-intents.md](plan-intents.md). This file is the reference: what an app sends, what a wallet answers, and the messages Airlock's prototype speaks.

A site sends a wallet a complete transaction, as today, with **notes** saying which of its accounts the wallet may replace, which are derived from others and how, and which the app signs for. The wallet **resolves** it however its account signs: at once or later, in one transaction or several, at the top of a transaction or through a program. The app gets an id and follows a status.

```
transaction + notes ─► resolve ─► id ─► status: awaiting-approval ─► … ─► done
```

A wallet without these methods signs the same transaction through `solana:signAndSendTransaction`. Nothing in the request is in a form only a new wallet reads.

## Rules

1. **The wallet reads what it signs.** The composed transaction is what review shows and what is signed. Notes never reach review.
2. **The wallet changes only what was declared.** A program, an account that carries no note and instruction data outside a `data` entry stay as the app wrote them, at the top of a transaction or inside a carrier. The blockhash or nonce, the compute budget, the format, the lookup tables and the carrier are always the wallet's.
3. **One approval covers the request, and nothing partial goes back.**
4. **Never `failed` for what may still land.** That is `unknown`.
5. **A requirement the wallet cannot meet refuses the request**, naming it. It is never dropped quietly.
6. **A profile goes to connected sites only.**
7. **Every note is checked against the transaction before it is used.**

## How an account signs

A connected site asks how an account signs before it builds anything.

```ts
interface SigningProfile {
  account: string;                              // base58
  timing: "sync" | "async";
  execution: "top-level" | "carrier";
  carrier?: string;                             // the carrier program, when there is one
  feePayer: ("account" | "wallet" | "app")[];   // who may pay
  newAccounts: ("wallet-key" | "carrier" | "app-key")[];   // how a new account may be signed for
  delivery: ("send" | "cosign" | "return")[];
  messages: ("signature" | "memo")[];
  maxTransactions: number;
}
```

- **`sync`**: the resolution finishes while the request is open. It may still take longer than a blockhash lives (a hardware wallet, an authorization signed before the transaction), which is why the wallet and not the app chooses the lifetime.
- **`async`**: it finishes later, and maybe never: a key on another device, or a multisig that needs votes this device cannot cast.
- **`carrier`**: the instructions run inside a program's instruction, not at the top of the transaction. A cosigner that has to check what it signs would need to read that program, so an app may decline these.

`newAccounts` says who can sign for an account the request creates:

- **`wallet-key`**: a key the wallet makes takes the place of the app's key and signs. It signs a transaction that runs while the request is open.
- **`carrier`**: the carrier program signs, at an address it derives. No key exists for that address, so it holds however much later the instructions run and whoever runs them.
- **`app-key`**: the app's key stays, and the app signs.

The profile is what the account promises now. It can go stale, so a wallet that cannot resolve a request refuses it (`unsupported`).

### Airlock's profiles

| Wallet | `timing` | `execution` | `feePayer` | `newAccounts` | `delivery` | `messages` |
| --- | --- | --- | --- | --- | --- | --- |
| Hot key, hardware wallet | `sync` | `top-level` | account, wallet, app | wallet-key, app-key | send, cosign, return | signature, memo |
| Offline key | `async` | `top-level` | account, wallet | wallet-key | send | memo |
| Programmatic signer or Vector wallet, authority on this device | `sync` | `carrier` | wallet | wallet-key | send | memo |
| The same, authority elsewhere | `async` | `carrier` | wallet | wallet-key | send | memo |
| Squads multisig this device can carry alone: its acting member and the wallets that approve with it reach the threshold, one of them may execute, and there is no time lock | `sync` | `carrier` | wallet | carrier | send | memo |
| Any other Squads multisig, and one whose threshold this device does not know | `async` | `carrier` | wallet | carrier | send | memo |

A Squads multisig whose acting member's own route ends at a key that is not on this device is `async` whatever its threshold. An account that signs through a programmatic signer or a Vector account, a multisig whose member does included, has `maxTransactions` 1, since each authorization is ordered by a nonce the one before it moves; a request of more is `unsupported` (`order`). Every other account has 16.

A Squads multisig signs for a `new` account the way it signs for its vault: the program does. The account gets an **ephemeral signer**, an address the Squads program derives from the account that stores the proposal's transaction (the program's seed prefix, that account's address, `"ephemeral_signer"`, a one-byte index), and the stored transaction declares how many it uses. The program signs for each when the proposal executes, at once or after other members have voted, whoever executes it. So a multisig that waits for votes can create an account too. The address follows from the proposal's number, so it is known once the wallet has prepared the request, and `replaced` gives it.

## The request

```ts
interface ResolveInput {
  id?: string;                    // the app's own, [A-Za-z0-9-]{1,80}, unique per site. The wallet makes one when absent
  account: string;                // the account acted as
  chain: string;                  // "solana:mainnet" | "solana:devnet" | "solana:testnet"
  transactions: Bytes[];          // 1 to 16, complete, legacy, v0 or v1
  order?: "atomic" | "serial" | "any";          // default "serial"
  delivery?: "send" | "cosign" | "return";      // default "send"
  accounts?: Record<string, AccountNote>;       // keyed by address as the transactions hold it
  data?: DataNote[];
}

type AccountNote =
  | { kind: "new"; replaceable: boolean }
  | { kind: "derived"; pda: { program: string; seeds: Seed[] } }
  | { kind: "derived"; withSeed: { base: string; seed: string; owner: string } }
  | { kind: "feePayer"; replaceable: boolean; signs?: "app" }
  | { kind: "signer"; signs: "app" };

type Seed = { account: string } | { bytes: Bytes };

type DataNote =
  | { transaction: number; instruction: number; offset: number; account: string }   // 32 bytes: that address
  | { transaction: number; instruction: number; offset: number; bumpOf: string };   // 1 byte: that derived account's bump
```

`instruction` counts the transaction's own instructions as it was sent, compute budget instructions included. `offset` is into that instruction's data. A note applies to every transaction of the request that holds its address. `Bytes` is a `Uint8Array` in a Wallet Standard method and base64 on the bridge.

The account acted as carries no note. When it is also the transaction's fee payer, who pays the fee is the wallet's to choose, as the blockhash is: a wallet without a key has another key pay, and every instruction stays as written. A `feePayer` note is for an address that only pays, such as a sponsor.

### What each note lets the wallet do

| Note | Says | The wallet may |
| --- | --- | --- |
| `new` | The request creates this account, and the app made its key | When `replaceable`: put an address it can sign for in its place, a key of its own or an address its carrier program signs for. When not: nothing, and the app signs for it |
| `derived` | The address follows from others | Derive it again when a seed, the base or the program's inputs were replaced, and write the new address and bump wherever the old ones were |
| `feePayer` | This address pays. With `signs: "app"` the app holds its key (a sponsor) | When `replaceable`: put its own payer wherever the address appears |
| `signer` | A key the app holds signs the result | Nothing. It asks the app for the signature |
| none | The account is as written | Nothing |

### `order`

| `order` | Means |
| --- | --- |
| `atomic` | Everything runs in one transaction, or nothing does |
| `serial` | Each runs once the one before it succeeded. The first failure stops the rest. The default |
| `any` | Each stands alone |

### Checks

A note that fails one refuses the request with `bad_notes`, naming the address or the entry.

1. Every address in `accounts`, and every `account` and `bumpOf` in `data`, is among the static accounts of a transaction of the request. A seed's `account` need not be.
2. A `derived` note's derivation gives the address it is filed under. A `bumpOf` names a `pda` note, and the bump is the canonical one.
3. A `new` account signs a transaction of the request, and the chain has never seen it. With no node the second half is skipped.
4. Each `data` entry is inside the instruction's data and names bytes that are that address, or that bump.
5. An address a note lets the wallet replace, and any derived account that rests on one, appears nowhere in instruction data that no `data` entry names: the wallet looks for its 32 bytes and refuses when it finds them.
6. An address carries one note, and the account acted as appears under no key of `accounts`.

A use the wallet cannot see, such as a hash of the key or the key written as text, cannot be declared. The app marks that account not replaceable and signs for it.

### What the wallet does with them

In this order, before anything is shown:

1. A `new` account that is `replaceable` gets an address the wallet can sign for: a key made on this device (`wallet-key`), or, for an account whose profile lists `carrier`, an address its carrier program signs for. One that is not `replaceable` needs the app's signature (`app-key`).
2. A `feePayer` that is `replaceable` is replaced by the payer of the account's signing route when that is another address, unless it is the app's (`signs: "app"`), the delivery is `cosign` or `return`, and the profile lists `app` as a payer: then the app's payer stays. One that is not `replaceable` stays, and needs the app's signature when it `signs: "app"`.
3. Every `derived` account that rests on a replaced address is derived again, to any depth.
4. Each replaced address and bump is written into the account lists and the `data` entries. Signatures made over the old message are dropped.
5. The result goes through what the wallet does to any site's transaction: conversion, the rebuild for a wallet without a key, review.

A request needs the app's signature when a `new` account was kept, a `feePayer` of the app's was kept, or it holds a `signer` note. That takes `cosign` or `return`, and a profile whose `newAccounts`, `feePayer` and `delivery` allow it. Otherwise it is `unsupported`.

## Delivery

| Delivery | What happens |
| --- | --- |
| `send` | The wallet composes, signs and sends |
| `cosign` | The wallet composes, the person approves and signs, the app adds its signatures (`provideSignatures`), the wallet sends |
| `return` | As `cosign` up to the person's signature. Then the signed transaction goes back, and the app signs and sends it where it must |

- The person approves before the app signs.
- For `cosign` the app is handed each composed transaction with every signature slot empty, and the signers it is asked for. The wallet's signatures stay in the wallet until it sends, so the resolution can still be cancelled.
- For `return` the app is handed the transaction with the wallet's signatures in it. Its signature is out, so it cannot be cancelled. The wallet watches the chain for it when the fee payer's signature is one it holds.
- The app compares what comes back with what it sent, allowing only what its notes allow (`replaced` says which address became which), before it signs.

## The resolution

Plain JSON at every layer: addresses and signatures are base58, transactions are base64.

```ts
interface Resolution {
  id: string;
  account: string;
  chain: string;
  timing: "sync" | "async";
  delivery: "send" | "cosign" | "return";
  state: State;
  replaced: Record<string, string>;         // address sent -> address used
  transactions: {                           // what the wallet made, once it has
    role: "direct" | "carrier" | "proposal" | "vote" | "execution";
    of: number[];                           // which transactions of the request it carries
    state: "pending" | "landed" | "failed" | "unknown";
    signature?: string;
    error?: string;
    wire?: string;                          // base64: in `awaiting-app` with empty signature slots, in `returned` as signed
  }[];
  ran: { transaction: number; signature: string }[];   // for each transaction sent in, where its instructions ran
  awaiting?: { transaction: number; signers: string[] }[];   // in `awaiting-app` and `returned`: index into `transactions`, and whose signatures the app owes
  message?: { memo: string };               // set when this resolves a message signed as a memo
  error?: { code: string; message: string; requirement?: string };   // `failed` when it landed and failed or cannot land
  updatedAt: number;                        // Unix seconds
}
```

| State | Means | Ends there |
| --- | --- | --- |
| `awaiting-approval` | The person has not decided | |
| `awaiting-signatures` | Approved here. Another device or other people still sign | |
| `awaiting-app` | Composed and signed here. The app owes signatures | |
| `returned` | Handed to the app to sign and send | When the wallet cannot watch for it |
| `sending` | Going out | |
| `done` | Every transaction sent in has run. `ran` says where | Yes |
| `failed` | It landed and failed, or it can no longer land | Yes |
| `unknown` | It may still land. Sending it again as something new could do it twice | Yes |
| `declined`, `expired`, `cancelled` | As named | Yes |

A proposal that landed is not `done`: its instructions have not run, and the state is `awaiting-signatures` until they have. `ran` gives the app a signature to read from the chain itself.

A request waits for the person ten minutes when the account is `sync`, and a day when it is `async`. Once approved it does not expire.

## Signing a message

No key signs as a program's address, so a multisig or a programmatic signer cannot sign a message the usual way. It can sign a transaction, so it signs the message as a memo:

- **`signature`**: a key signs the bytes. The app gets the signature, as from `solana:signMessage`.
- **`memo`**: the wallet builds a transaction whose one instruction is the SPL Memo program's (`MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr`), with the account as its only account, a signer, and the message as its data. It is resolved like any request with `delivery: "send"`, and the app gets the resolution. Once it is `done`, `ran[0].signature` is the proof.

The message is the memo, byte for byte. It has to be UTF-8 text of at most 512 bytes, or the request is `unsupported`.

To check a memo, the app reads the transaction by its signature and looks for an instruction of the Memo program, at the top or among the inner instructions, whose data is the message and whose accounts are exactly the account. The Memo program fails unless every account it is given signed, so a transaction that succeeded with that instruction in it is the account's word. As with a signed message, anyone can show the same proof again, so the app puts its own domain and a nonce in the message.

The app names the mode, or leaves it to the wallet: `signature` when the profile lists it, otherwise `memo`. A memo is a transaction, so the app names the chain it is for, as in any request. With none named it goes out on the network the wallet is on, when that is one it can name.

## The methods

Wallet Standard features. The prototype registers them under `airlock:`; the proposal would name them `solana:`. Each is version `0.1.0`.

```ts
"airlock:getSigningProfile": { getSigningProfile(input: { account: WalletAccount }): Promise<SigningProfile> }

"airlock:resolve": {
  resolve(input: ResolveInput & { account: WalletAccount; transactions: Uint8Array[] }): Promise<Resolution>;
  // Every change to a resolution of this site, while the page is open.
  onResolution(listener: (resolution: Resolution) => void): () => void;
}

"airlock:getResolution": { getResolution(input: { id: string }): Promise<Resolution> }

"airlock:provideSignatures": {
  provideSignatures(input: {
    id: string;
    signatures: { transaction: number; signer: string; signature: Uint8Array }[];
  }): Promise<Resolution>;
}

"airlock:cancelResolution": { cancelResolution(input: { id: string }): Promise<Resolution> }

"airlock:signMessage": {
  signMessage(input: { account: WalletAccount; message: Uint8Array; mode?: "signature" | "memo"; chain?: string; id?: string }):
    Promise<
      | { kind: "signature"; signedMessage: Uint8Array; signature: Uint8Array }
      | { kind: "memo"; resolution: Resolution }
    >;
}
```

- `resolve` settles when the resolution first leaves `awaiting-approval`: with the resolution, or with a rejection when it was declined (`rejected`), expired (`expired`) or cancelled (`cancelled`). What happens after that reaches the app through `onResolution` and `getResolution`.
- In `provideSignatures`, `transaction` is an index into the resolution's `transactions`, as in `awaiting`.
- `getResolution` answers for a resolution of the same site, after the page was loaded again too.
- `cancelResolution` works in `awaiting-approval` and `awaiting-app`. In any other state it is `bad_state`.
- `signMessage` with `memo` settles as `resolve` does.

An app that finds no `airlock:resolve` on the wallet sends the same transaction to `solana:signAndSendTransaction`.

## Airlock's prototype: the bridge messages

These ride the browser bridge of [browser-bridge.md](browser-bridge.md#messages): the same frames, `v`, `id`, `origin` and errors. In these messages an address or a signature is base58, and a transaction or a message is base64. None of them waits for the person before it answers, so each is answered within seconds; what the person decides arrives as a `resolutionChanged` push.

| Method | `params` | `result` |
| --- | --- | --- |
| `getSigningProfile` | `{ "account" }` | `{ "profile": SigningProfile }` |
| `resolve` | `ResolveInput`, with `transactions` as base64 strings | `{ "resolution": Resolution }`, in `awaiting-approval`. The request has joined the queue the window shows |
| `getResolution` | `{ "id" }` | `{ "resolution" }` |
| `provideSignatures` | `{ "id", "signatures": [ { "transaction", "signer", "signature" } ] }` | `{ "resolution" }`: `sending` once every signature owed is in, `awaiting-app` until then |
| `cancelResolution` | `{ "id" }` | `{ "resolution" }`, in `cancelled` |
| `resolveMessage` | `{ "id"?, "account", "message", "mode"?: "signature" \| "memo", "chain"? }` | `{ "kind": "memo", "resolution" }` at once for a memo. For a signature it waits for the person, as `signMessage` does, and answers `{ "kind": "signature", "signedMessage": "<base64>", "signature": "<base58>" }` |

The app sends one push for each change to a resolution, to every browser, and the extension passes it only to pages of that origin:

```json
{ "v": 1, "method": "resolutionChanged", "origin": "https://app.example", "params": { "resolution": { "id": "…", "state": "done" } } }
```

Errors, beside those of the bridge:

| Code | When |
| --- | --- |
| `bad_notes` | A note fails a [check](#checks). The message names it |
| `unsupported` | The account cannot do something the request needs. `error.requirement` names it: `delivery` when the profile does not list the delivery asked for; `newAccounts`, `feePayer` or `signer` for what would need the app's signature when the delivery or the profile gives it no way to sign; `newAccounts` also for a new account the wallet cannot sign for; `order`, `messages`, `chain` |
| `not_implemented` | More than 16 transactions, as for any request |
| `duplicate_id` | The site used this id before |
| `not_found` | No resolution of this site has this id |
| `bad_state` | The resolution is not in a state that takes this |
| `bad_signature` | A signature given does not verify for its signer and transaction, or that signer is not owed |
| `rejected` | The account is not connected to the site, or the site is blocked |

## Airlock's prototype on Android: Mobile Wallet Adapter

On a phone a page reaches Airlock through Mobile Wallet Adapter (MWA), not the extension. MWA is JSON-RPC over an encrypted local WebSocket, and a wallet may answer methods beyond the standard ones. Airlock answers six more in a session, with the same `params` and `result` as the bridge messages above:

| MWA method | Bridge message | A page's MWA client calls it as |
| --- | --- | --- |
| `airlock_get_signing_profile` | `getSigningProfile` | `wallet.airlockGetSigningProfile(params)` |
| `airlock_resolve` | `resolve` | `wallet.airlockResolve(params)` |
| `airlock_get_resolution` | `getResolution` | `wallet.airlockGetResolution(params)` |
| `airlock_provide_signatures` | `provideSignatures` | `wallet.airlockProvideSignatures(params)` |
| `airlock_cancel_resolution` | `cancelResolution` | `wallet.airlockCancelResolution(params)` |
| `airlock_sign_message` | `resolveMessage` | `wallet.airlockSignMessage(params)` |

- `get_capabilities` lists them in `features` as `airlock:getSigningProfile`, `airlock:resolve`, `airlock:getResolution`, `airlock:provideSignatures`, `airlock:cancelResolution` and `airlock:signMessage`.
- They answer only in a session that was authorized (`authorize`, or `reauthorize` with a valid token), and the site is the identity given there. Otherwise the error is `rejected`.
- An error is a JSON-RPC error with code `-1000`, the bridge's message, and `data` `{ "code": "<the bridge's code>", "requirement"?: "…" }`. An `airlock_` method that is not one of the six is MWA's own `-32601`. Solana Mobile's `@solana-mobile/mobile-wallet-adapter-protocol` (2.2 to 3.0.2) drops a JSON-RPC error's `data` when it throws, so a page that uses it has to put `data` back (one line, where it builds `SolanaMobileWalletAdapterProtocolError`).
- `airlock_resolve` answers once the resolution leaves `awaiting-approval`, as the Wallet Standard `resolve` settles, so the person decides while the page waits on one request. Declined, expired and cancelled are errors (`rejected`, `expired`, `cancelled`). `airlock_sign_message` waits the same way, for either mode.
- It waits through `sending` as well, so a resolution delivered `send` comes back `done`, `failed` or `unknown`: Airlock sends what was approved from its screen, and a phone stops that screen once Airlock is behind the browser, which it is a moment after the answer. A `sign_and_send_transactions` waits for its send in the same way.
- A request still awaiting approval when the session ends stays, as it does when a page closes on desktop: the person may still decide, and the page asks for the resolution in a new session. Airlock stays in front for it.
- MWA has no way for a wallet to speak first, so there is no `resolutionChanged`. A page asks `airlock_get_resolution`: in the same session while it stays open, or in a new one later. A request that never waits for the person (a profile, a resolution, signatures given, a cancel) does not bring Airlock to the front, and Airlock does not step back after it.
- A phone may stop a browser that is behind from hearing anything, and stop an app that is behind from answering. So a page that waits on a resolution that finishes later ends the session, and asks again in a new session when it wants to know: Airlock comes to the front for a moment as the session opens.

What the prototype leaves out:

- `order: "atomic"` with more than one transaction is `unsupported`.
- A note on an account that a v0 transaction reads from a lookup table is `bad_notes`.
- An endpoint for the app's signatures. They are given from the page, so `cosign` and `return` are offered to `sync` accounts that sign at the top of a transaction, and to no other.
- Resolutions are kept in `resolutions.json`, the latest 200. Signed transactions that have not gone out are kept in memory only, so after a restart a resolution that was `awaiting-approval` or `awaiting-app` is `failed`, and one that was `sending` is `unknown`.
- A Squads multisig's profile is judged from its wallet record until the Proposals pane has read the multisig on the cluster in use: every listed member is taken to hold every permission, and no time lock is assumed. After that it is judged from what the chain said: threshold, permissions and time lock.
- A proposal a resolution waits on is followed only until Airlock restarts.
- A returned transaction is watched for three minutes. One never seen in that time stays `returned`.
- A message signed as a memo goes in as a legacy transaction when a key of this device signs at the top, and as a version 1 transaction for every other account, since only that can be rebuilt or handed to another device. A node shows a version 1 transaction from `getTransaction` only when asked with `maxSupportedTransactionVersion: 1`, in any encoding and with its logs (where the Memo program writes who signed and the memo); surfpool, offline or forking devnet, does, and answers "Encoding does not support transaction version 1" when asked for less. A node that does not know version 1 at all gives only the transaction's status, so the test page then says that the proof was not checked, rather than passing or failing it.
- A `new` account of a Squads multisig signs in one transaction of the request. One that signs in several is `unsupported` (`newAccounts`): an ephemeral signer belongs to one proposal. Other transactions of the request may still name the account.
- An ephemeral signer's address depends on the proposal's number. Each preparation reads the number again and derives the addresses again when it moved, and `replaced` changes with them. When another proposal is stored after that, a rebuild before signing refuses and asks to prepare again, and a transaction already signed fails as any proposal with a taken number does.
- A smart account that could run a request at once (`executeTransactionSync`) stores a proposal when the request has a `new` account, and creates, approves and executes it in one transaction: there is no stored transaction to derive an address from otherwise.
- The transactions of one request for a Squads multisig are stored as proposals with consecutive numbers, so they land in order: with `order: "any"`, one that fails leaves those after it unable to land.
