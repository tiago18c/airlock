# Vector program fork

How Airlock pins the Vector program, what we changed in the fork, how to rebase it, and the decisions and rules behind Vector wallets. How the app signs for Vector wallets is in [signing.md](signing.md).

## Pin

| What | Value |
| --- | --- |
| Upstream | [blueshift-gg/vector](https://github.com/blueshift-gg/vector) (pull request 17 / `feat/single-program`) |
| Fork | [tiago18c/vector](https://github.com/tiago18c/vector), branch `feat/single-program` |
| Revision | `e0566fc4f15686b5f49c419d7eb08bb4496bf9cc` (upstream PR 17 head `9cf6bf9` plus our patches) |
| Dependency | `vector-core = { git = "https://github.com/tiago18c/vector", rev = "e0566fc…" }` in `crates/wallet-core/Cargo.toml` |
| Program ID | `74mXFW7BSAAwc6zQC2uYKuDcWnz17HHXMWqLnEeg9BJv` (devnet) |
| Binary pin | SHA-256 in `scripts/fetch-programs.mjs` (667,936 bytes on chain) |

`deny.toml` allows that fork and the three `blueshift-gg` git sources `vector-core` pulls in (`solana-ml-dsa`, `solana-winternitz`, `solana-shake`).

Addresses under our program ID are not the addresses the same keys will have under an upstream deployment. A wallet made here does not move there.

## Patches on the fork

All four sit in **one** commit (`e0566fc`) on top of upstream `9cf6bf9`. Split them before a rebase or before offering them upstream:

1. **`declare_id!`** in `programs/vector/src/lib.rs` → our program ID. Owners, PDAs and instruction scans use `crate::ID`; `vector-core`'s `PROGRAM_ID` comes from the same place.
2. **`ed25519-dalek`** in `tests/Cargo.toml`: `"=2.1.1"` → `"2.2"`, matching this repo.
3. **`verify_signature(scheme, identity, digest, signature)`** in `crates/core/src/verify.rs`, including XMSS and Winternitz through `solana-winternitz`. Identity for those two is the key the account holds now. It does not record spent leaves or keys.
4. **`idl/vector.json`**, a Codama IDL for the five instructions, the account header and the program's errors. Vendored unchanged as `crates/wallet-core/idls/vector.json`.

## Deployment (devnet)

Checked 2026-10-14, for `fd71dda`. Redeployed from `e0566fc` before 2026-10-18; the new binary's SHA-256 is pinned in `scripts/fetch-programs.mjs` and the flow tests pass against it. The rows below were not checked again for the redeploy.

| Claim | Result |
| --- | --- |
| Cluster | Devnet, upgradeable loader, last deployed in slot 510797325 |
| Upgrade authority | `3zFrUC2W8ziNmDjhMJ9qdfcVJjGUCg94BrvYXtKA8cqM` |
| IDL | Program-metadata account `138wxQsuz9i7n9rQWJUSXhXb4Q6nJPxi3p7PUNrUwuCv`, byte-for-byte the fork's `idl/vector.json` |
| Reproducible build | **No.** `cargo build-sbf` of `fd71dda` here yields 667,904 bytes with a different hash from byte 41 on. Redeploy from a build anyone can repeat before mainnet. |

Mainnet is in scope later ([D1](#decisions)): reproducible bytes, an audit of the deployed revision, and an upgrade authority that is not one hot key.

## What PR 17's head changed for the app

Moving from `094fa5a` to `9cf6bf9` (2026-10-18):

- **`Advance`** carries, per account, the key it signs with next (Winternitz and XMSS, 41 bytes) before its signature. Each digest is `sha256(message ‖ nonce ‖ address ‖ next key)`. A Winternitz account names a new key every time; an XMSS account names its current key to keep it.
- **`Passthrough`** no longer depends on the scheme: `[1, n, as-system flag × n, count, …]` with accounts `[instructions sysvar, vector × n, …]`. It opens on the Sign screen with nothing known about its accounts. A signer may sign as its account's system PDA (`[vector address]` under the program).
- **`Expand`** is gone: `Initialize` with no key after the discriminator grows an ML-DSA-44 account, three calls in one transaction.
- **The header's byte 66** is the system PDA's bump, not a version.
- The program has its own error codes (`NotTopLevel`, `NotAdvanced`, `KeyReused`, `LeafUsed`, `TreeExhausted`).

## Decisions

Taken on 2026-10-14 unless noted.

- **Target pull request 17 as it stands**, in our fork, deployed by us. Only `vector` is forked (D2); `deny.toml` allows that fork and the three `blueshift-gg` repositories its manifest pins.
- **D1: the maintainer deploys the program** and holds its keys. Mainnet is in scope, later. Before it: a build anyone can repeat to the deployed bytes, an audit of the program at the revision deployed, and an upgrade authority that is not one hot key.
- **Link `vector-core` by git**, pinned by revision, behind one module boundary (below).
- **One wallet kind for both protocols**, `Programmatic { protocol, authority, fee_payer }`. One `Advance` per transaction, any number of Vector signers in it, and one authorization protocol per transaction.
- **Vector is turned on by a run-time setting.** Every build holds every scheme; there are no cargo features for them. ML-DSA-44 is the post-quantum default on every build.
- **D3: `falcon-rust` may hold a key**, desktop and airgap only, labelled experimental. It verifies against the on-chain verifier (header rewritten `0x59`→`0x39`). It is unaudited, uses floating point, and makes no constant-time claim.
- **D4: names.** "Programmatic signer" stays the Ed25519 signer program. The new kind is "Vector wallet · Ed25519", "Vector wallet · ML-DSA-44" and so on, both under "Smart contract wallet".
- **D5: one Ed25519 key may back both a programmatic signer and a Vector wallet.** Both rows are shown.
- **D6: Winternitz is on, with a known risk** (on again 2026-10-25; turned off 2026-10-21; first offered 2026-10-14). A key signs once, and a transaction runs whole or not at all. When the `Passthrough` fails on chain (slippage, an account that moved), the `Advance` rolls back with it, so the account still holds the key that already signed. Anything else it does needs a second signature from that key, which weakens it, so the app refuses: the account is stuck until a program lets the key be used up whether or not the action succeeds. The row says so. A key device that never reads the chain learns that a transaction landed from the account state (kind 7) the online device shows, then moves to its next key and shows the one after it (kind 4).
- **No backwards compatibility** while the app has no released users: a stored format carries a version number and is refused whole when it is not the build's.

## Rules for the code

- **Nothing outside `wallet-core/src/wallets/vector` and `wallets/relay/vector*` names `vector_core`.** The pull request may still change before it merges; one module boundary keeps that cheap.
- **Bytes and rules live in `wallet-core`.** UI code never hashes, derives an address, parses an account or decides whether something may be signed. A form previews an address by calling `vector::address`.
- **Anything that talks to a node is a function in `wallet-flows`** taking `&dyn SolanaRpc`, tested against the fake node.
- **Closed enums and exhaustive matches for protocols and schemes**, never `_ =>` arms on `WalletKind`, `RouteStep`, `AskedWallet`, `AdaptedStep` or `SchemeId`, so adding a variant lists every site. Nothing outside `wallets/vector` matches on `SchemeId` except the add form's picker and `summary`.
- **Signing status types stay `Copy` and small.** Slots carry addresses; keys stay in records.
- **Secrets are `SecretBytes`** and never appear in `Debug`.

## Stateful keys (XMSS and Winternitz)

Both schemes sign with `solana-winternitz` `hazmat`. The chain sees what has landed; it does not see two signatures made off chain before either lands. Shared defences: desktop only (browser storage can be cleared or synced back), sign last, salt retry for the same digest, no export of the secret, and a next public key held ahead of a rotating wrap.

- **The secret is a 32-byte seed**, expanded by a ChaCha20 stream into the hazmat key, so the key is rebuilt on unlock and the vault holds the seed and a counter rather than the full key. This construction is ours, not upstream's, and is pinned by a known-answer test: changing it changes every key. Signature salts come from the OS, never from the seed.
- **XMSS seeds come from a recovery phrase**, along Winterwallet's derivation path, so a lost device's keys can be made again and the account's current key found by scanning. See [signing.md](signing.md#recovery-phrases-xmss).
- **Reserve before signing.** What is about to be signed (an XMSS leaf, or the Winternitz digest and salt) is written to `vector-leaves.json`, outside the vault, and synced before the signature is made: the file is replaced in one step, and the replacing itself is flushed, on Windows too. It keeps no older copy, since an older copy read in its place would lower a mark. One left by an earlier build is removed by the next save, and blocks signing if the file itself is gone. A save that fails means no signature. A `vector-leaves.json` that cannot be read is left as it is and blocks every hash-based signature until it reads again, rather than starting empty.
- **A retry does not spend a leaf.** The same digest signs again with the same salt.
- **Sign last.** When several accounts sign one `Advance`, a stateful one signs after every other signature is in and verified.
- **XMSS (256 leaves):** the signing leaf is the largest of the record's counter, the high-water mark and the chain's `next_leaf`. A floor that only a context note gave is not taken when it is above this device's own count, so one lying note cannot spend the leaves below it: the device refuses, and scanning the account state takes that floor on purpose. When the mark is missing while the record says leaves were used, 16 leaves are skipped. Warn at 80% of leaves used; past 95%, only rotating transactions are built, and the device that holds the key signs nothing else (`LeafFor`): the last 12 leaves are kept for the rotation, at least as many as the nonces it may sign at. The device that holds the key signs a rotation by itself, at every nonce its own signatures may have left the account at, and installs the next key once a nonce shows the rotation landed (see [signing.md](signing.md#rotating-an-xmss-key)).
- **Winternitz (one leaf):** every `Passthrough` carries the next key, and any digest other than the recorded one is refused until the account has moved to its next key.
- **What cannot be defended:** a restored copy of the whole data directory, or a cloned virtual machine, signing again before the first signature lands. The row says a stateful key must live on one device and never be restored from a backup.

## Risks

- **The pull request changes before it merges, or does not merge.** The fork pins what we deployed, so nothing breaks on its own. A later upstream change means a new deployment and new addresses; wallets made before it stay on the old program.
- **An unaudited program that we operate.** Whoever holds the upgrade authority can replace the code behind every wallet. The setting stays off by default, and the record pins the program ID.
- **No expiry.** A signed authorization stays valid until the nonce moves, so a relay can hold it back. The same is true of the Ed25519 programmatic signer. The Sign screen says so. A deadline instruction (with revocation and ordered pre-signing) is not built.
- **A digest that is wrong by one flag fails only on chain.** That is why demotion cases are refused instead of modelled, and why the flow tests in LiteSVM are the proof.
- **Transaction size with ML-DSA.** One signature is 2,420 of 4,096 bytes. Two ML-DSA signers do not fit in one transaction, and a Squads proposal behind one often will not.
- **An account asked to pay inside another program fails.** The `Withdraw` rewrite covers a plain transfer; the simulation before wrapping tells the user before they sign.
- **One level less of CPI depth** for what runs inside a `Passthrough`.

## Origins and credits

Most of what Airlock does with Vector accounts is built on Blueshift's work ([blueshift-gg](https://github.com/blueshift-gg)):

- **The Vector program** ([blueshift-gg/vector](https://github.com/blueshift-gg/vector), pull request 17), which we fork and deploy, and its client crate `vector-core`. Its README's guidance on rotation (generate and persist a fresh signer, retain and rebroadcast a signed authorization, never sign twice with one leaf) is what the rotation flow follows.
- **`solana-winternitz`**, `solana-ml-dsa` and `solana-shake`, which sign and verify the hash-based and ML-DSA keys.
- **Winterwallet** ([blueshift-gg/winterwallet](https://github.com/blueshift-gg/winterwallet/tree/672fc6789b1532ee680f24842d235e0be8737b61), at `672fc67`), whose design the recovery phrase follows: a BIP-39 phrase, a master node of `HMAC-SHA512(magic, seed)`, hardened-only SLIP-0010 style children along `wallet' / parent' / child'`, and recovery by deriving keys until one matches the key on chain. Its pattern of persisting the next signing position before anything is sent is the one `vector-leaves.json` follows. Our derivation reproduces its published test vector step for step; only the magic string and what a node becomes differ (see [signing.md](signing.md#recovery-phrases-xmss)).

## Rebase

When upstream moves (PR 17 merges or the branch advances):

1. Fetch upstream into the fork and rebase the four patches as separate commits (or cherry-pick after splitting the combined commit).
2. Rebuild `vector.so`, redeploy if the program ID or account layout changed, and update the SHA in `scripts/fetch-programs.mjs`.
3. Bump the `rev` in `crates/wallet-core/Cargo.toml` and refresh `Cargo.lock`.
4. Re-run `cargo test -p flow-tests` (after `node scripts/fetch-programs.mjs`) and the Vector unit tests in `wallet-core`.
5. If the program ID or Initialize layout changed, existing wallets on the old deployment stay there; they do not migrate.

Do not point `vector-core` at crates.io: that name is an unrelated crate.
