# Vector program fork

How Airlock pins the Vector program, what we changed in the fork, how to rebase it, and the decisions and rules behind Vector wallets. How the app signs for Vector wallets is in [signing.md](signing.md).

## Pin

| What | Value |
| --- | --- |
| Upstream | [blueshift-gg/vector](https://github.com/blueshift-gg/vector) `master`, Vector 2.0 (`f806f6c`, pull requests 17, 18, 20, 22 and 24) |
| Fork | [tiago18c/vector](https://github.com/tiago18c/vector), branch `airlock/v2` (its `master` follows upstream's) |
| Revision | `6d893b5b7f056b4daf7155637c8574cbb2c7f37e` (upstream `f806f6c` plus our three patches) |
| Dependency | `vector-core = { git = "https://github.com/tiago18c/vector", rev = "6d893b5…" }` in `crates/wallet-core/Cargo.toml` |
| Program ID | `74mXFW7BSAAwc6zQC2uYKuDcWnz17HHXMWqLnEeg9BJv` (devnet) |
| Binary pin | SHA-256 in `scripts/fetch-programs.mjs` (667,936 bytes of program data on chain: the 577,368-byte build, then zeros left from the first deployment) |

`deny.toml` allows that fork as the only git source. The verifiers `vector-core` links (`solana-ml-dsa`, `solana-winternitz`, `solana-xmss`, `solana-shake`, `solana-falcon512`) come from crates.io since 2.0.

Addresses under our program ID are not the addresses the same keys will have under an upstream deployment. A wallet made here does not move there. Upstream's own ID, `vectorcLBXJ2TuoKuUygkEi6FWqvBnbHDEDWoYamfjV`, was deployed on neither devnet nor mainnet on 2026-11-14.

## Patches on the fork

Three commits on top of upstream `f806f6c`, each one thing, so they rebase and can be offered upstream on their own:

1. **Our program ID** (`ece6665`): `declare_id!` in `programs/vector/src/lib.rs`, the TS SDK's `PROGRAM_ID`, the validator script and the Readme. Owners, PDAs and instruction scans use `crate::ID`; `vector-core`'s `PROGRAM_ID` comes from the same place. Addresses derive from the ID, so the PDAs, bumps and digests the Rust and TS tests pin, and `fixtures/vectors.json` (regenerated with the ignored `regenerate_vectors` test), change with it.
2. **`verify_signature(scheme, identity, digest, signature)`** in `crates/core/src/verify.rs` (`8013153`), for every scheme, with the program's own scheme code: the key signs `"vector:" ‖ digest`. Upstream's `verify_advance` rebuilds the message from every signer's key, which a device that signs for one account seldom has; the app computes the message from the transaction instead and checks one signature over its digest. Identity for Winternitz and XMSS is the key the account holds now. It does not record spent leaves or keys. The same commit takes `ed25519-dalek` `"2.2"` instead of `"=2.1.1"`, matching this repo.
3. **`idl/vector.json`** (`6d893b5`), a Codama IDL for the six instructions, the 76-byte header, the nonce account and errors 0 to 9. Vendored as `crates/wallet-core/idls/vector.json` with display nodes added.

## Deployment (devnet)

Upgraded to `6d893b5` on 2026-11-14 in slot 522350558. Every account made under the program before it is left unreadable by it (the header grew), which is accepted: it was devnet only, with no funds at stake.

| Claim | Result |
| --- | --- |
| Cluster | Devnet, upgradeable loader, program data `4QGrtQkj1ofFfNGZzFSsMXUZBXeXWpQepgx4AANhCLQS` |
| Upgrade authority | `3zFrUC2W8ziNmDjhMJ9qdfcVJjGUCg94BrvYXtKA8cqM` |
| Build | `cargo build-sbf --arch v0 --manifest-path programs/vector/Cargo.toml` with the Agave 4.4.1 release (`cargo-build-sbf` 4.5.0, platform-tools v1.57), upstream's CI toolchain. 577,368 bytes, SHA-256 `2495582244d26767afa3abc38002b6fcccc84efe2bfc7745076aeccd85aca317`; the deployed bytes match it (`solana program dump`). `get-program-hash` (trailing zeros trimmed): `7abfe93a…` |
| IDL | Program-metadata account `138wxQsuz9i7n9rQWJUSXhXb4Q6nJPxi3p7PUNrUwuCv`, written with `@solana-program/program-metadata` 0.11.0, byte-for-byte the fork's `idl/vector.json` (SHA-256 `45c3f486…`) |
| Reproducible build | Not checked on a second machine, nor in a container. Redeploy from a build anyone can repeat before mainnet. |

Mainnet is in scope later ([D1](#decisions)): reproducible bytes, an audit of the deployed revision, and an upgrade authority that is not one hot key.

## What Vector 2.0 changed for the app

Moving from PR 17's head `9cf6bf9` to upstream `f806f6c` (2026-11-14):

- **A key signs `"vector:" ‖ digest`**, never the digest bare. Ed25519, Falcon-512 and ML-DSA-44 sign those 39 bytes; secp256k1, Winternitz and XMSS sign their SHA-256; EIP-191 is `personal_sign` of the 39 bytes (`\x19Ethereum Signed Message:\n39`), and the program also takes a `v` of 27 or 28. The site-message guard refuses anything that starts with `vector:`, and no longer refuses a 32-byte message.
- **`Advance` expires.** Its data is `[0, valid_until (u64 LE), {next key, signature}…]`. The message keeps the discriminator and `valid_until` and cuts the rest, so every signature covers the one expiry, and the program fails with `Expired` (9) past it. The Sign screen names that slot in the `Advance`'s sentence. The app sets it ([signing.md](signing.md#rebuilding-for-a-wallet-without-a-key)): about an hour after the slot the wrap was made at, or never when a Winternitz or XMSS account signs. A rotation never expires.
- **The header is 76 bytes**: `created` (the slot the account was made in, u64) after the seed, so the scheme byte is at 72 and every account is 8 bytes longer. `Close` refuses an account in the slot it was made in (`FreshVector`).
- **`Initialize` refuses keys that cannot sign**: an Ed25519 key of small order or in a second encoding (y ≥ p, which anyone could sign for), and a secp256k1 key whose x is not on the curve. The app refuses the same keys when one is pasted (`KeyRefused`).
- **Winternitz and XMSS accounts never close**: `Close`, and a `Withdraw` of every lamport, fail with `KeyRotates` (6). Migrate already keeps rent in an account with data.
- **Nonce accounts** (`InitializeNonce`, 5): extra nonces of a vector, listed after it in an `Advance`, so its transactions need not land in order. The app makes none, and an `Advance` that lists one reads wrong on Sign (its accounts are all read as signers).
- **XMSS comes from `solana-xmss`**, Winternitz from `solana-winternitz` 0.3; the `hazmat` keys the app rebuilds from a seed are at `solana_xmss::hazmat` and `solana_winternitz::hazmat`, and make the same keys as before. The recovery salt's known answer changed with the message it is tested over.
- New errors: `ForeignNonce` (5), `KeyRotates` (6), `FreshNonce` (7), `FreshVector` (8), `Expired` (9).

## Decisions

Taken on 2026-10-14 unless noted.

- **Target upstream's release**, in our fork, deployed by us: pull request 17 as it stood until 2026-11-14, Vector 2.0 (`master`) since. Only `vector` is forked (D2); `deny.toml` allows that fork and no other git source.
- **D1: the maintainer deploys the program** and holds its keys. Mainnet is in scope, later. Before it: a build anyone can repeat to the deployed bytes, an audit of the program at the revision deployed, and an upgrade authority that is not one hot key.
- **Link `vector-core` by git**, pinned by revision, behind one module boundary (below).
- **One wallet kind for both protocols**, `Programmatic { protocol, authority, fee_payer }`. One `Advance` per transaction, any number of Vector signers in it, and one authorization protocol per transaction.
- **Vector is turned on by a run-time setting.** Every build holds every scheme; there are no cargo features for them. ML-DSA-44 is the post-quantum default on every build.
- **D3: `falcon-rust` may hold a key**, desktop and airgap only, labelled experimental. It verifies against the on-chain verifier (header rewritten `0x59`→`0x39`). It is unaudited, uses floating point, and makes no constant-time claim.
- **D4: names.** "Programmatic signer" stays the Ed25519 signer program. The new kind is "Vector wallet · Ed25519", "Vector wallet · ML-DSA-44" and so on, both under "Smart contract wallet".
- **D5: one Ed25519 key may back both a programmatic signer and a Vector wallet.** Both rows are shown.
- **D6: Winternitz is on, with a known risk** (on again 2026-10-25; turned off 2026-10-21; first offered 2026-10-14). A key signs once, and a transaction runs whole or not at all. When the `Passthrough` fails on chain (slippage, an account that moved), the `Advance` rolls back with it, so the account still holds the key that already signed. Anything else it does needs a second signature from that key, which weakens it, so the app refuses: the account is stuck until a program lets the key be used up whether or not the action succeeds. The row says so. A key device that never reads the chain learns that a transaction landed from the account state (kind 7) the online device shows, then moves to its next key and shows the one after it (kind 4).
- **No backwards compatibility** while the app has no released users: a stored format carries a version number and is refused whole when it is not the build's.
- **Moving to Vector 2.0 in place** (2026-11-14): the devnet program was upgraded at the same ID, and accounts made before it are left behind, since they were devnet only and held nothing of value.
- **D7: every authorization the app wraps expires** about an hour after it is made (`vector::AUTHORIZATION_SLOTS`, 9,000 slots), except when a Winternitz or XMSS account signs, which never expires (2026-11-14). An hour carries it to a key on another device and back, and outlives a blockhash refresh, which its signature does not cover.

## Rules for the code

- **Nothing outside `wallet-core/src/wallets/vector` and `wallets/relay/vector*` names `vector_core`.** The pull request may still change before it merges; one module boundary keeps that cheap.
- **Bytes and rules live in `wallet-core`.** UI code never hashes, derives an address, parses an account or decides whether something may be signed. A form previews an address by calling `vector::address`.
- **Anything that talks to a node is a function in `wallet-flows`** taking `&dyn SolanaRpc`, tested against the fake node.
- **Closed enums and exhaustive matches for protocols and schemes**, never `_ =>` arms on `WalletKind`, `RouteStep`, `AskedWallet`, `AdaptedStep` or `SchemeId`, so adding a variant lists every site. Nothing outside `wallets/vector` matches on `SchemeId` except the add form's picker and `summary`.
- **Signing status types stay `Copy` and small.** Slots carry addresses; keys stay in records.
- **Secrets are `SecretBytes`** and never appear in `Debug`.

## Stateful keys (XMSS and Winternitz)

Both schemes sign through `hazmat`: `solana-winternitz`'s for Winternitz, `solana-xmss`'s for XMSS, each over the SHA-256 of `"vector:" ‖ digest`. The chain sees what has landed; it does not see two signatures made off chain before either lands. Shared defences: desktop only (browser storage can be cleared or synced back), sign last, salt retry for the same digest, no export of the secret, and a next public key held ahead of a rotating wrap.

- **The secret is a 32-byte seed**, expanded by a ChaCha20 stream into the hazmat key, so the key is rebuilt on unlock and the vault holds the seed and a counter rather than the full key. This construction is ours, not upstream's, and is pinned by a known-answer test: changing it changes every key. Signature salts come from the OS, never from the seed, with one exception: a wallet restored from its recovery phrase makes the salt of its rotation from the seed, the leaf and the digest, so that restoring twice signs the same rotation once (also pinned by a known-answer test).
- **XMSS seeds come from a recovery phrase**, along Winterwallet's derivation path, so a lost device's keys can be made again and the account's current key found by scanning. See [signing.md](signing.md#recovery-phrases-xmss).
- **Reserve before signing.** What is about to be signed (an XMSS leaf, or the Winternitz digest and salt) is written to `vector-leaves.json`, outside the vault, and synced before the signature is made: the file is replaced in one step, and the replacing itself is flushed, on Windows too. It keeps no older copy, since an older copy read in its place would lower a mark. One left by an earlier build is removed by the next save, and blocks signing if the file itself is gone. A save that fails means no signature. A `vector-leaves.json` that cannot be read is left as it is and blocks every hash-based signature until it reads again, rather than starting empty.
- **A retry does not spend a leaf.** The same digest signs again with the same salt.
- **Sign last.** When several accounts sign one `Advance`, a stateful one signs after every other signature is in and verified.
- **XMSS (256 leaves):** the signing leaf is the largest of the record's counter, the high-water mark and the chain's `next_leaf`. A floor that only a context note gave is not taken when it is above this device's own count, so one lying note cannot spend the leaves below it: the device refuses, and scanning the account state takes that floor on purpose. When the mark is missing while the record says leaves were used, 16 leaves are skipped. Warn at 80% of leaves used; past 95%, only rotating transactions are built, and the device that holds the key signs nothing else (`LeafFor`): leaves 244 to 251 are kept for the rotation, as many as the nonces it may sign at. The last four, 252 to 255, are signed with only by a wallet restored from its recovery phrase, one for each attempt at its rotation, from the last one down ([signing.md](signing.md#recovery-phrases-xmss)). The device that holds the key signs a rotation by itself, at every nonce its own signatures may have left the account at, and installs the next key once a nonce shows the rotation landed (see [signing.md](signing.md#rotating-an-xmss-key)).
- **Winternitz (one leaf):** every `Passthrough` carries the next key, and any digest other than the recorded one is refused until the account has moved to its next key.
- **What cannot be defended:** a restored copy of the whole data directory, or a cloned virtual machine, signing again before the first signature lands. The row says a stateful key must live on one device and never be restored from a backup.

## Risks

- **Upstream keeps changing.** Pull requests 25 (Winternitz and XMSS accounts may close again) and 26 (the client crate becomes `vector-signer`, which the program depends on) were open on 2026-11-14. The fork pins what we deployed, so nothing breaks on its own. A later upstream change to the layout or the digest means a new deployment and new addresses.
- **An unaudited program that we operate.** Whoever holds the upgrade authority can replace the code behind every wallet. The setting stays off by default, and the record pins the program ID.
- **A Winternitz or XMSS authorization never expires.** It stays valid until the nonce moves, so a relay can hold it back; one of any other scheme lapses at its `valid_until` (D7). The Ed25519 programmatic signer's authorizations have no expiry either.
- **A digest that is wrong by one flag fails only on chain.** That is why demotion cases are refused instead of modelled, and why the flow tests in LiteSVM are the proof.
- **Transaction size with ML-DSA.** One signature is 2,420 of 4,096 bytes. Two ML-DSA signers do not fit in one transaction, and a Squads proposal behind one often will not.
- **An account asked to pay inside another program fails.** The `Withdraw` rewrite covers a plain transfer; the simulation before wrapping tells the user before they sign.
- **One level less of CPI depth** for what runs inside a `Passthrough`.

## Origins and credits

Most of what Airlock does with Vector accounts is built on Blueshift's work ([blueshift-gg](https://github.com/blueshift-gg)):

- **The Vector program** ([blueshift-gg/vector](https://github.com/blueshift-gg/vector), Vector 2.0), which we fork and deploy, and its client crate `vector-core`. Its README's guidance on rotation (generate and persist a fresh signer, retain and rebroadcast a signed authorization, never sign twice with one leaf) is what the rotation flow follows.
- **`solana-winternitz`**, `solana-xmss`, `solana-ml-dsa`, `solana-falcon512` and `solana-shake`, which sign and verify the hash-based and post-quantum keys.
- **Winterwallet** ([blueshift-gg/winterwallet](https://github.com/blueshift-gg/winterwallet/tree/672fc6789b1532ee680f24842d235e0be8737b61), at `672fc67`), whose design the recovery phrase follows: a BIP-39 phrase, a master node of `HMAC-SHA512(magic, seed)`, hardened-only SLIP-0010 style children along `wallet' / parent' / child'`, and recovery by deriving keys until one matches the key on chain. Its pattern of persisting the next signing position before anything is sent is the one `vector-leaves.json` follows. Our derivation reproduces its published test vector step for step; only the magic string and what a node becomes differ (see [signing.md](signing.md#recovery-phrases-xmss)).

## Rebase

When upstream's `master` moves:

1. Fast-forward the fork's `master` to it, branch from it, and cherry-pick the three patches. The program ID patch changes the pinned PDAs, bumps and digests in the Rust and TS tests: update them to what the failing tests print, and run `cargo test -p vector-tests -- --ignored regenerate_vectors` for `fixtures/vectors.json`.
2. Build with `cargo build-sbf --arch v0 --manifest-path programs/vector/Cargo.toml` (upstream's CI uses Agave 4.4.1) and run `cargo test --workspace` in the fork.
3. Upgrade the devnet program with the upgrade authority (`solana program deploy --program-id 74mX… target/deploy/vector.so`), check the dump against the build, and write the IDL with `program-metadata write idl 74mX… idl/vector.json`.
4. Update the SHA in `scripts/fetch-programs.mjs`, the vector entry in `resources/program-builds.toml` and the on-chain IDL's SHA-256 in `wallet-core/src/idl/builtin.rs`.
5. Bump the `rev` in `crates/wallet-core/Cargo.toml` and refresh `Cargo.lock`.
6. Re-run `cargo test -p flow-tests` (after `node scripts/fetch-programs.mjs`) and the Vector unit tests in `wallet-core`.
7. If the account layout changed, accounts made before the upgrade are unreadable by the program; decide whether that is acceptable before upgrading in place.

Do not point `vector-core` at crates.io: that name is an unrelated crate.
