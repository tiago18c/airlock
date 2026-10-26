# IDLs

How Airlock uses IDLs to read and build instructions, and the rules that keep what the user reads tied to what they sign. The dynamic client itself, and the gaps of its Anchor converter, are in `crates/codama/README.md`. What an IDL read from the chain may and may not unlock at signing time is in [signing.md](signing.md).

## Rules

- **One IDL per program, not per transaction.** A transaction is a list of instructions, each with its own program id, so one message has a set of IDLs. Lookup, checksum, display and QR pins are all per program id. One instruction failing to match an IDL does not blank the others.
- **IDLs are kept per network.** A program can run at different addresses on mainnet and devnet, and the same address can run different versions on each. The catalog keeps the files of each network apart (`IdlNetwork`: mainnet, devnet, testnet, localnet) and shows the network of the cluster in use. A custom RPC reads the IDLs of the public cluster its genesis matched; one that matched none reads localnet's.
- **IDL bytes never travel in a QR code.** A zipped Token-2022 IDL is about 34 KiB, a device may need dozens of programs, and the QRS1 payload cap is 32 KiB (`MAX_PAYLOAD` in `qr-protocol`). IDLs move by file import on each device. QRS1 carries at most a 32-byte SHA-256 pin per program and a few small account blobs (the context note, kind 3).
- **The signing device explains bytes with the IDL stored on that device.** A pin from the other device is a comparison, not a document to adopt. Display never changes the signed message.
- **IDLs are read from the chain only.** `wallet_flows::idl` reads the program-metadata account first, then the legacy Anchor IDL account. Nothing fetches an IDL over HTTP.
- **A check never overwrites.** Checking a program online hashes the bytes the chain publishes and records whether they match the stored file, or the document the file was made from. "Differs" does not switch the explanation to the remote document; the user may import the remote bytes on purpose, which replaces the stored one. With airgap set, nothing is fetched and the status stays "not checked".
- **The sentence is text; tooltips are a span list beside it.** `interpolated_intent` stays a plain string. Hover tooltips come from `IntentSpan`s produced by the same walk. Public names (`toly.sol`) never go into the signed sentence or the QR object.

## Where it lives

| Piece | Where |
| --- | --- |
| Catalog: imported files, checksums, the index | `crates/wallet-core/src/idl/catalog` |
| Embedded IDLs, their networks and sources | `crates/wallet-core/idls/`, listed in `BUILTIN_IDLS` (`idl/builtin.rs`) |
| Networks | `crates/wallet-core/src/idl/network.rs` |
| Building an instruction from an IDL | `crates/wallet-core/src/idl/build`, `idl/draft.rs`, the Build screen's tabs |
| Explaining an instruction | `crates/wallet-core/src/review/explain` |
| Reading an IDL from the chain | `crates/wallet-flows/src/idl.rs` |
| The context note (kind 3) | `crates/qr-protocol/src/object.rs` |

Every review row and every Build form comes from an IDL. The hand-written decoders and forms are gone. The one byte-level rule left is the runtime's: a leading System `AdvanceNonceAccount` makes a durable-nonce transaction. Compute budget and the nonce advance are under Build's Advanced section.

## Embedded IDLs

`crates/wallet-core/idls/` holds the IDL of each program the app needs without an import (System, Stake, Token, Token-2022, compute budget, the Ed25519 programmatic signer, the two Squads programs, Vector, the Raydium CLMM and CP swap on devnet and mainnet, the devnet Raydium router, Orca Whirlpool), unchanged from upstream apart from display nodes. The Squads and Raydium files are Anchor IDLs converted to Codama; the Raydium ones also declare the remaining accounts their instructions read (the CLMM's tick arrays and swap route, the CP swap config's new owner), which an Anchor IDL cannot, and show their token amounts in base units because they cannot read a mint's decimals. The Raydium router publishes no IDL, so `raydium-router.json` is written by hand from transactions the program executed (its docs page lists account orders the chain does not use). Its swaps name the owner and the accounts at fixed positions; the hops, whose accounts depend on each pool program, are one hidden remaining-accounts group. `SwapBaseOut` (tag 9) is left out until a transaction shows its layout. The Orca Whirlpool file is the IDL its program-metadata account publishes on mainnet: the program has the same address and version on devnet, where its IDL accounts are missing or stale. Its V2 instructions declare the transfer hook and supplemental tick array accounts that `remainingAccountsInfo` slices out. To update one, replace the file and its source checksum; the `builtin.rs` tests check its program id, its name, that no two files claim a program on the same network, and the pinned default instruction on every network.

They are compiled into the binary and never written to the index. Importing a file for the same program on the same network replaces the embedded one, and removing that import brings it back. An embedded IDL cannot be removed and is left out of an export. `.gitattributes` keeps the files LF on every checkout: `include_str!` embeds the checked-out bytes, and their checksum is compared across devices.

### Networks and sources

Each `BUILTIN_IDLS` entry lists the networks it is read on, its source (`IdlSource`) and what was changed in it:

| File | Networks | Source |
| --- | --- | --- |
| System, Stake, Token, Token-2022, compute budget | all | the program's repository; display nodes added |
| Ed25519 signer, Vector | devnet | the program-metadata IDL devnet publishes |
| Squads v4 | mainnet, devnet | the 2.1.0 Anchor IDL devnet publishes (mainnet still publishes 2.0.0) |
| Squads smart account | mainnet, devnet | the repository's Anchor IDL, newer than either network's |
| Raydium CLMM, CP swap (`DRay…`) | devnet | the program-metadata IDL devnet publishes |
| Raydium CLMM, CP swap (`CAMM…`, `CPMM…`) | mainnet | the program-metadata IDL mainnet publishes |
| Raydium router | devnet | written by hand |
| Orca Whirlpool | mainnet, devnet | the program-metadata IDL mainnet publishes |

A program at different addresses on two networks has a file for each: the file declares its address, and often accounts that differ by network too (Raydium's admin and fee receiver). The mainnet Raydium files are the devnet ones with those addresses swapped, which is all the two networks' published IDLs differ in, and a test checks that.

A file made from what a program publishes on chain keeps the SHA-256 of those bytes and the account they were read from (`IdlChainSource`). Our file's checksum C1 is never the chain's C2: it was converted, given display nodes, or given accounts the source cannot declare. A check reads the account the file was made from (`IdlRecord::compared_account`) and records both comparisons (`IdlCheck::same`, `IdlCheck::source`). When the chain publishes C2, the status is "matches its source": the file is current and nothing is fetched to replace it. When it publishes something else, the status is "differs", and the source is where to start an update. The app reads a program's IDL from the chain only when the network in use has none stored, so a download never replaces an embedded file.

### Stored files

The index lists each imported file with its network. Bodies are `idls/<network>/<program id>.json` on desktop and `airlock.idl.<network>.<program id>` in a browser. Only the network in use is read into memory; switching cluster reads the other's files. An index from before networks has no `network` on its records: the files are moved under the network in use when the app opens. An export holds every network's files as `<network>.<program id>.json`, and an older bundle with `<program id>.json` files is imported for the network in use.

### Additional programs

A Codama file can carry other programs in `additionalPrograms`. The catalog gives each one a record of its own (`IdlRecord::host` names the file it is read from), so build and review use it like any other program. Its checksum, origin and online check are the host file's. Such a record is never stored or exported, cannot be removed on its own, and goes when its host file goes.

- A file of the program's own, embedded or imported, wins over an entry carried by another file.
- When several files carry the same program, the fullest entry wins: Codama nodes plus non-empty docs lines (`companions::completeness`). On a tie, the host listed first in `BUILTIN_IDLS` wins, then the lower host address.

The embedded files carry three: the associated-token program in `token` and `token-2022`, and the message executor and its nonce program in `ed25519-programmatic-signer`. The associated-token entries are fuller than that program's own `idl.json` (the payer, the associated account and the token program have defaults, so the form derives the address), so that file is no longer embedded. On the ATA tab, "Change" on the token program row switches it to Token-2022, and the address follows. The program's own file also has `createWithArgs`, which neither entry has; a transaction using it reads raw until that file is imported.

## The context note (kind 3)

An object of its own, matched to a transaction by its message hash. Share sends it in the same payload as the transaction, after it, or alone for a device that already has the transaction. Written with wincode (`TxContext` in `qr-protocol`):

```text
message         [u8; 32]  SHA-256 of the serialized versioned message
pins            u16 count, 0..=64, each:
  program_id    [u8; 32]
  idl_sha256    [u8; 32]
authorizations  u16 count, 0..=16, each:
  account       [u8; 32]
  scheme        u8
  nonce         [u8; 32]
  leaf_floor    u32 LE
accounts        u16 count, 0..=32, each:
  pubkey        [u8; 32]
  data          u16 length, then the bytes
```

[signing.md](signing.md) lists the object kinds.

- Pins are one per distinct program id. Pins and authorizations are kept: more than a note holds, and it is not built.
- The receiver decodes a note only when a transaction is loaded and the message hash matches. Otherwise it says the note is for a different transaction and keeps the transaction. A second note for the same hash replaces the first.
- For each instruction whose program has a pin, the pin is compared with the local checksum. Unequal or missing locally: that program's line says so, and the note's account bytes are not fed into that instruction's display, because a different IDL must not decode a mint.
- The note is ignored for signing. Signatures cover the versioned message only. Review, build and spans all work with no note.

## Known gaps

- An account whose signer status the IDL leaves open (`either`, as on token authorities) is always signed for. There is no multisig signer list.
- An IDL names accounts by position. When an instruction carries more accounts than its IDL describes, and the IDL allows no extras, review says so on that row (`IdlExplanation::unnamed_accounts`), because the names may be on the wrong accounts. The embedded stake IDL describes the layout without sysvar accounts, so a stake instruction in the older layout, which most clients still send, is flagged this way.
- Out of scope: Codama spec 2.0, and the `nodes-from-anchor` gaps listed in the Codama readme (the builder surfaces those as a missing default, not as a silent address).
