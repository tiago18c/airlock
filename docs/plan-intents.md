# Plan: a wallet standard where a request is an intent

Status: **the text is written, and a prototype is being built in Airlock on desktop** (2026-11-15). The reference is [intents.md](intents.md): where it and this plan differ, it is right. The decisions under [Decisions](#decisions) are Tiago's, from the sessions of 2026-11-14 and 15. What is still undecided is under [Open](#open).

Today a site hands a wallet finished transaction bytes and gets a signature back. That fits a wallet whose key signs the transaction on the spot, and nothing else. Airlock already treats a site's transaction as a suggestion: it rebuilds it for a multisig, moves it onto a durable nonce for a key on another device, swaps a key the site made for one of its own, and offers to pay the fee itself. It does all of this by guessing, and the site learns none of it.

This plan is for a proposal to the Wallet Standard that makes that explicit:

```
transaction + notes on its accounts ─► the wallet resolves it ─► an id, and a status the app can follow
```

- The **transaction is the intent**. The app sends a complete transaction, as today, with notes saying which of its accounts the wallet may replace, which are derived from others and how, and which the app signs for.
- The **wallet resolves it** however its account signs: in one transaction, in several, now or later.
- The **wallet says how each account signs** before the app builds anything, and the app decides what to offer.

Background: [signing.md](signing.md) for how each wallet kind signs today, [browser-bridge.md](browser-bridge.md) for how a site reaches the app, [templates.md](templates.md) for the same idea inside the app (`$wallet`, `$fee-payer`, `new-keypair`, derived addresses).

## What Airlock found

Each row is something the app does now that the Wallet Standard cannot express.

| What Airlock does | Where | What a standard needs |
| --- | --- | --- |
| Rebuilds a request for a Squads vault, a programmatic signer or a Vector account. The site "gets back a transaction that creates a proposal, which may not be what it expects" | [Squads vault](signing.md#squads-vault), [Rebuilding](signing.md#rebuilding-for-a-wallet-without-a-key) | The wallet picks the carrier, and says what it did |
| Answers a request whose action has not happened: with a threshold above 1, the site's instructions run after other members approve | [Squads vault](signing.md#squads-vault) | An id and a status that outlive the request, and "landed" told apart from "ran" |
| Moves a site's transaction onto a durable nonce, because its blockhash does not outlive a QR round trip | [Offline wallet](signing.md#offline-wallet-1), [Known gaps](signing.md#known-gaps) | The wallet chooses the lifetime |
| Reads `serial` and `parallel` its own way: "The standard does not say what the modes mean or which is the default" | [Sign and send](signing.md#sign-and-send) | Order and atomicity that are defined |
| Invented `send_unknown` beside `send_failed`, so a site does not send the same thing twice | [Messages](browser-bridge.md#messages) | Three outcomes, and an id that makes a retry safe |
| Finds a site's one-time keys by reading `CreateAccount` and simulating, then swaps them in accounts and in instruction data | [Keys the site made](signing.md#keys-the-site-made-for-one-transaction), `wallet_flows::request` | New accounts are declared, with where they appear |
| Offers "Pay with this wallet instead" when a site's sponsor cannot sign what was rebuilt | [A site that names its own fee payer](signing.md#a-site-that-names-its-own-fee-payer) | The fee payer is declared, and a sponsor signs after the wallet composes |
| Watches the chain to find out whether a site sent what was returned to it | [After the transaction goes back](signing.md#after-the-transaction-goes-back-to-the-site) | The app says who sends |
| Advertises only what it can do, "so a site never picks a feature that is certain to fail" | [Extension](browser-bridge.md#extension) | A profile of how each account signs |

## Prior art

Read on 2026-11-15. Links are under [Sources](#sources).

| What | What it gives | What it leaves |
| --- | --- | --- |
| Wallet Standard, Solana features (`@solana/wallet-standard-features`) | `solana:signTransaction`, `solana:signAndSendTransaction` and `solana:signAndSendAllTransactions`, all at 1.0.0, for versions `legacy`, `0` and `1`. `signTransaction` returns a transaction, not a signature, so that a multisig or program wallet can return a modified one | Nothing says what a wallet may modify, or what it did. `mode` is `'parallel' \| 'serial'` with no meaning and no default written. Every method answers once |
| sRFC 19, `solana:getEphemeralSigners` (Squads, 2023) | The same problem as our new accounts: a keypair the app made is gone when a stored transaction runs. The app asks the wallet for addresses first and builds with them | A round trip before building, no fallback for a wallet without it, and nothing for addresses derived from the new one. The thread has no replies. Our notes cover the same case from the other side: the app builds as always and marks the account |
| sRFC 30, account abstraction interfaces (2024) | An on-chain interface for smart accounts | It does not touch what an app says to a wallet |
| EIP-5792 (Final) | The closest model. `wallet_sendCalls` returns an id, `wallet_getCallsStatus` reads it (pending, confirmed, offchain failure, reverted, partly reverted), `wallet_getCapabilities` says what an account can do. The app may give the id, and a duplicate is refused. A capability the wallet lacks refuses the request unless it is marked optional. `atomicRequired` | It sends calls, not a transaction, so there is nothing to fall back to. Sponsorship is left to another ERC. Nothing for a batch that resolves over days |
| Kora (Solana Foundation) | A JSON-RPC server that signs as fee payer: `signTransaction`, `estimateTransactionFee`, `getPayerSigner` | It expects to be handed a finished transaction, which is what a sponsor's endpoint would be here |

What we take: the id, the status and the capabilities of EIP-5792, with its rule that an unsupported requirement refuses the request. What we keep from Solana: the transaction itself, so a wallet without the new methods signs the same bytes.

## Decisions

| Question | Decision |
| --- | --- |
| What an intent is | The transaction, with notes. Not a list of instructions in a new format, and not an outcome such as "swap A for B" |
| How the notes reach the wallet | New methods. `signTransaction` and the two sends stay as they are, as the fallback |
| New accounts | Declared in the notes as replaceable, not reserved from the wallet first |
| New accounts of a Squads multisig | The Squads program signs for them, with its ephemeral signers: an address derived from the proposal's transaction account. So a multisig that waits for votes can create an account too, whoever executes the proposal |
| Derived accounts | Declared with how to derive them, so the wallet derives them again after a replacement |
| The fee payer | Declared like any other account, so the wallet may put its own wherever that address appears |
| Signatures of the app's own keys | Made after the wallet composes. In a resolution that finishes at once, the wallet can hand the transaction back for the app to sign and send itself |
| Flows that cannot finish later | The wallet states how each account signs. The app decides what it supports: it may sponsor fees for a resolution that finishes at once and for no other |
| Checking a transaction that came back inside a carrier | The app's affair, as part of deciding whether it supports that kind of account. The standard asks no wallet for decoders |
| Who sees an account's profile | A connected site only |
| Timing | Two values, `sync` and `async`. A Squads multisig this device cannot carry past its threshold is `async`, as an offline wallet is. Every other account is `sync`, though it may take longer than a blockhash lives. This replaces the three values in the tables below |
| Signing a message | A key returns a signature. An account with no key signs the message as a memo in a transaction, and the app gets that transaction's signature |
| The prototype | Airlock on desktop as the wallet, and a test page for every method, in the manner of the wallet adapter's example. Then the Android app, over Mobile Wallet Adapter, with the same page |

## The notes

The request is the transactions, complete and valid, signed by the app's own keys where it has them, exactly as it would send them today. The notes are keyed by address as the transaction holds it, so they read the same for legacy, v0 and v1.

```ts
{
  account,                      // the account acted as
  chain,
  id,                           // the app's, unique per account and origin; a second use is refused
  transactions: [tx0, tx1],
  order: "atomic" | "serial" | "any",
  delivery: "send" | "cosign" | "return",
  accounts: {
    "<mint>":  { kind: "new", replaceable: true },
    "<ata>":   { kind: "derived", pda: { program: "<ATA program>",
                 seeds: [{ account: "<owner>" }, { bytes: "<token program>" }, { account: "<mint>" }] } },
    "<payer>": { kind: "feePayer", replaceable: true, signs: "app" },
    "<admin>": { kind: "signer", signs: "app" },
  },
  data: [
    { transaction: 0, instruction: 2, offset: 4, account: "<mint>" },
    { transaction: 0, instruction: 3, offset: 0, bumpOf: "<ata>" },
  ],
}
```

| Kind | Says | The wallet may |
| --- | --- | --- |
| `new` | The transaction creates this account, and the app made its key | Replace it with a key of its own, or with a signer its program provides (a Squads ephemeral signer), when `replaceable` |
| `derived` | The address follows from others: a program-derived address from seeds, or `createWithSeed` | Derive it again when a seed or the base was replaced. Nothing else |
| `feePayer` | This address pays. With `signs: "app"` it is a sponsor | Replace it with a payer of its own, everywhere the address appears, when `replaceable` |
| `signer` | A key the app holds signs the result | Nothing. It asks the app for the signature |
| none | The account is as written | Nothing |

Whatever the notes say, these are always the wallet's: the blockhash or nonce, the compute budget, the transaction's format and its lookup tables, and the carrier.

`order` is defined, which `mode` never was:

| `order` | Means |
| --- | --- |
| `atomic` | Everything runs in one transaction, or nothing does. A wallet that cannot do that refuses |
| `serial` | Each runs once the one before it succeeded. The first failure stops the rest. The default |
| `any` | Each stands alone |

### What the wallet checks

A note is a claim about a transaction the wallet holds, so every one is checked before it is used. One that fails refuses the request (`bad_notes`), naming it.

1. A `derived` note's derivation gives the address the transaction holds.
2. A `new` account signs the transaction, the transaction creates it, and the chain has never seen it.
3. Each `data` entry names bytes that are that address, or that bump.
4. A key the wallet replaces appears nowhere in instruction data that no `data` entry names. The wallet looks for its 32 bytes and refuses when it finds them.
5. Every address in `accounts` is in the transaction.

A use the wallet cannot see, such as a hash of the key or the key written as text, cannot be declared. The app marks that account not replaceable, signs for it itself, and gives up the wallets that must replace it.

Review does not change: the wallet reads the transaction it composed, never the notes, as Review and Sign reads a transaction and never the template it came from. A lying note can only produce a transaction the user is then shown.

## How an account signs

A connected site can ask how an account signs, before it builds anything.

| Property | Values | In Airlock |
| --- | --- | --- |
| Timing | `immediate`: within the request. `deferred`: the same person, minutes later. `collective`: other people, with no bound | A hot key or a hardware wallet. An offline key over QR. A Squads multisig above one vote, or with a time lock |
| Execution | At the top of a transaction, or through a carrier program, which it names | A key. A programmatic signer, a Vector account, a Squads vault |
| When the bytes are final | Before the wait, or at the end | An offline key on its durable nonce. An authorization, then a fee payer on a fresh blockhash |
| Fee payer | The account, another key of the wallet, the app's | A hot key pays for itself. Every wallet without a key is paid for |
| New accounts | A key the wallet makes, the app's key kept, a signer the program provides | All three. Squads provides its own |
| Delivery | `send`, `cosign`, `return` | Below |
| The app's signatures | While the page is open, through an endpoint, or not at all | |

The profile is the least the account promises now. A multisig that says `collective` still finishes at once when this device holds enough members, and the status says so. It can go stale, since a threshold can change, so a wallet that cannot resolve a request refuses it naming what it could not do, as EIP-5792 does.

### What each timing allows

| In the request | Immediate | Deferred | Collective |
| --- | --- | --- | --- |
| A new account, replaceable | Yes | Yes. The wallet holds the key until it is used | With a signer the program provides, or when it is created, approved and executed in one transaction |
| A new account, the app's key kept | Yes | Only if the bytes are final before the wait, or through an endpoint | Through an endpoint, when it executes |
| A signature of the app's | Yes | The same | The same |
| A fee the app pays | Yes | Through an endpoint, signing last | Every transaction needs a payer: the proposal, each vote and the execution |
| `atomic` | If it fits one transaction | The same | If one proposal holds it |

The right-hand columns are where an app chooses. Sponsoring fees for immediate accounts only, and letting the wallet pay otherwise, is a choice the app makes from the profile. So is whether it will sign a transaction whose instructions sit inside a carrier it would have to read.

## Delivery

| Delivery | What happens | Offered when |
| --- | --- | --- |
| `send` | The wallet composes, signs and sends | Always |
| `cosign` | The wallet composes, the person approves and signs, the app adds its signatures, the wallet sends | Immediate, with the page open. Otherwise only through an endpoint |
| `return` | As `cosign`, but the transaction goes back to the app, which signs and sends it where it must | Immediate only |

- The person approves before the app signs. The app's signature is not needed to read or simulate a transaction, and the app never signs what is then declined.
- What goes back is the transaction the wallet read, its signatures (those of new keys it made among them), and which address replaced which. The app compares it with what it sent, allowing only what its notes allow, before it signs.
- A returned transaction may have a signature slot still empty. One the wallet sends may not. Airlock draws the same line today between `signTransaction` and a send.
- Once a transaction is returned the wallet's signature is out, so it cannot be cancelled. The wallet watches for it, as `wallet_flows::track::watch_landed` does, and the status ends as landed or unknown.
- `return` is what a permissioned setting needs: a key only the app's own systems hold, or a node only they may send through. A collective resolution has no one transaction to return, and a deferred one finishes after the page may be gone.

## The methods

Working names. Until the proposal is accepted the prototype registers them under `airlock:`, and the `solana:` names are what it proposes.

| Method | Does |
| --- | --- |
| `solana:getSigningProfile` | Says how an account signs. Connected sites only |
| `solana:resolve` | Takes the request. Answers with the id once the person approves, or with a refusal |
| `solana:getResolution` | The status for an id. Also pushed as an event, and readable after the page is loaded again |
| `solana:provideSignatures` | The app's signatures, when the wallet asks for them |
| `solana:cancelResolution` | Best effort, and only while no signature has left the wallet |

An app that finds no `solana:resolve` sends the same transaction to `solana:signAndSendTransaction`.

### The status

```ts
{
  id,
  state,                        // below
  replaced: { "<sent>": "<used>" },
  transactions: [               // what the wallet made
    { role: "direct" | "carrier" | "proposal" | "vote" | "execution",
      signature, state: "pending" | "landed" | "failed" | "unknown", error? },
  ],
  ran: [ { transaction: 0, signature } ],   // for each transaction sent in: where its instructions ran
  awaiting?: { transaction, signers },      // what the app is asked to sign
}
```

| State | Means |
| --- | --- |
| `awaiting-approval` | The person has not decided |
| `awaiting-signatures` | Approved here. Another device or other people still sign |
| `awaiting-app` | Composed and signed here. The app is asked for its signatures |
| `returned` | Handed to the app to sign and send |
| `sending` | Going out |
| `done` | Every instruction sent in has run. `ran` says where |
| `failed` | It landed and failed, or it can no longer land |
| `unknown` | It may still land. Sending it again as something new could do it twice |
| `declined`, `expired`, `cancelled` | As named |

A proposal that landed is not `done`: its instructions have not run. `ran` gives the app a signature to read from the chain itself, so the wallet is not taken at its word.

## Rules

1. **The wallet reads what it signs.** The composed transaction is what review shows and what is signed. Notes, labels and the app's own words never reach review.
2. **The wallet changes only what was declared.** A program, an account that carries no note and instruction data outside a `data` entry are as the app wrote them, at the top of a transaction or inside a carrier.
3. **One approval covers the request, and nothing partial goes back.** As a batch does today.
4. **Never `failed` for what may still land.** That is what `unknown` is for.
5. **A requirement the wallet cannot meet refuses the request.** It is not dropped quietly.
6. **A profile goes to connected sites only.** It says that an account is a multisig, or kept on another device.
7. **A wallet without the methods signs the same bytes.** Nothing in the request is in a form only a new wallet reads.

## Phases

- [x] **0. Prior art.** Read on 2026-11-15, above.
- [x] **1. The text.** The request, the notes and their checks, the profile, the states and the refusals: [intents.md](intents.md), with the messages the prototype speaks. A document that stands without Airlock, for the forum, comes with phase 9.
- [x] **2. Notes in the core.** Types, the checks and the replacement in `wallet-core`, with a layer in `layers.rs`. It takes over from the guessing in `wallet_flows::request` (`created_accounts`, `replaceable_signers`, `one_time_signers`), which stays for a plain request. The profile comes from `signing_route` and `plan_adaptation`.
- [x] **3. Resolutions in `app-core`.** A store that is saved, ids, states and an event. A resolution outlives the ten minutes a request waits, and an app restart.
- [x] **4. The bridge.** The methods in `bridge-protocol`, the features in the extension, and limits for a status read, which waits on nobody.
- [x] **5. Delivery.** `send` through `tx::Submitter`, the `cosign` round, and `return` with the watch that follows it.
- [x] **6. Later resolutions.** A signature from the offline device completes one. A proposal made for one is followed until it runs. A Squads multisig, waiting or not, creates a new account at an ephemeral signer's address, so no key has to be kept for it. Left: a proposal is followed only until the app restarts.
- [x] **7. For apps.** The test page in `examples/intents`, one row for each method, with the comparison of a returned transaction with the one sent and the check of a memo. Left: those two as a library of their own.
- [x] **8. Tests.** Unit tests beside each part, a flow test for a mint made through notes, and `scripts/drive-intents-e2e.mjs`, which runs the page against the app and a local node for a hot key, a multisig carried alone, a multisig that waits, an offline key and a programmatic signer. Left: a Vector wallet, a smart account, an authority kept elsewhere, an offline key on its durable nonce, and the extension itself in a browser.
- [ ] **9. Proposing it.** An sRFC on the Solana forum and a pull request to `anza-xyz/wallet-standard`, once the prototype runs.

Each phase leaves `cargo fmt --all --check`, CI's Clippy step and the touched crates' tests green, and adds its line to `CHANGELOG.md` when people will notice it.

## Open

- **The endpoint.** What a wallet sends an app's endpoint and gets back, and which addresses it may call at all. A wallet that calls an address a site gave it tells that address when the person acts, so at the least it is `https`, on the site's own origin, and shown. Kora's `signTransaction` is the obvious shape for a sponsor. Nothing is decided, and phase 5 can ship without it.
- **Mobile Wallet Adapter.** Airlock's Android app answers the six methods as extra JSON-RPC methods in an MWA session (`airlock_*`), by extending Solana Mobile's walletlib. MWA lets a wallet answer only what the page asks, so a page asks again for a resolution that finishes later, in a new session. Solana Mobile's JS client drops a JSON-RPC error's `data`, so the test page patches it. A standard would want MWA to name such extensions, and its client to keep `data`. Tried only on an emulator, not on a real phone, where a browser or app behind another may be cut off.
- **A wallet that must change an instruction.** A Vector account cannot be the source of a System transfer, so Airlock turns it into `Withdraw`. Rule 2 forbids that. Either such an account says in its profile what it cannot run, or the standard names the substitutions it allows.
- **Packing.** An app splits what does not fit one transaction, and a carrier leaves less room than the app counted on. Whether a wallet may split one transaction of a `serial` request further is not decided.
- **The names.** `resolve` and the rest are placeholders.
- **Offline wallets and sites.** The prototype lets an offline wallet be connected to a site, which it could not be before: without that its profile could never be asked for. Whether that stays is Tiago's to decide.
- **A memo's proof on a version 1 transaction.** A wallet without a key sends its memo in a version 1 transaction, and an app reads the proof through `getTransaction`. Surfpool returns one when asked with `maxSupportedTransactionVersion: 1`. Whether the public clusters' nodes do was not tested.
- **An ephemeral signer's address moves with the proposal's number.** It is derived when the request is prepared. A proposal stored in between changes it, and the wallet prepares again. An app that showed the address before the resolution was `done` showed one that may change. Whether `replaced` should be withheld until the proposal has landed is not decided.
- **A smart account that runs one transaction of a request at once.** Airlock counts it as a stored proposal when it numbers the ones after it, so a later transaction of the same request with a new account derives its address from a number one too high and fails. Not fixed.
- **A memo as it is, or marked.** The memo is the message, byte for byte. A prefix would tell a message signed on purpose from a memo sent for another reason.

## Sources

- Wallet Standard, Solana features: <https://github.com/anza-xyz/wallet-standard/tree/master/packages/core/features/src>
- sRFC 19, Get Ephemeral Signers: <https://forum.solana.com/t/srfc-00019-wallet-standard-get-ephemeral-signers-feature/340>
- sRFC 30, Account Abstraction Interfaces: <https://forum.solana.com/t/srfc-30-account-abstraction-interfaces/1860>
- EIP-5792, Wallet Call API: <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-5792.md>
- Kora, JSON-RPC API: <https://solana.com/docs/tools/kora/json-rpc-api>
