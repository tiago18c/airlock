# IDLs

How Airlock uses IDLs to read and build instructions, and the rules that keep what the user reads tied to what they sign. The dynamic client itself, and the gaps of its Anchor converter, are in `crates/codama/README.md`. What an IDL read from the chain may and may not unlock at signing time is in [signing.md](signing.md).

## Rules

- **One IDL per program, not per transaction.** A transaction is a list of instructions, each with its own program id, so one message has a set of IDLs. Lookup, checksum, display and QR pins are all per program id. One instruction failing to match an IDL does not blank the others.
- **IDL bytes never travel in a QR code.** A zipped Token-2022 IDL is about 34 KiB, a device may need dozens of programs, and the QRS1 payload cap is 32 KiB (`MAX_PAYLOAD` in `qr-protocol`). IDLs move by file import on each device. QRS1 carries at most a 32-byte SHA-256 pin per program and a few small account blobs (the context note, kind 3).
- **The signing device explains bytes with the IDL stored on that device.** A pin from the other device is a comparison, not a document to adopt. Display never changes the signed message.
- **IDLs are read from the chain only.** `wallet_flows::idl` reads the program-metadata account first, then the legacy Anchor IDL account. Nothing fetches an IDL over HTTP.
- **A check never overwrites.** Checking a program online hashes the bytes the chain publishes and records whether they match the stored file. "Differs" does not switch the explanation to the remote document; the user may import the remote bytes on purpose, which replaces the stored one. With airgap set, nothing is fetched and the status stays "not checked".
- **The sentence is text; tooltips are a span list beside it.** `interpolated_intent` stays a plain string. Hover tooltips come from `IntentSpan`s produced by the same walk. Public names (`toly.sol`) never go into the signed sentence or the QR object.

## Where it lives

| Piece | Where |
| --- | --- |
| Catalog: imported files, checksums, the index | `crates/wallet-core/src/idl/catalog` |
| Embedded IDLs | `crates/wallet-core/idls/`, listed in `BUILTIN_IDLS` (`idl/builtin.rs`) |
| Building an instruction from an IDL | `crates/wallet-core/src/idl/build`, `idl/draft.rs`, the Build screen's tabs |
| Explaining an instruction | `crates/wallet-core/src/review/explain` |
| Reading an IDL from the chain | `crates/wallet-flows/src/idl.rs` |
| The context note (kind 3) | `crates/qr-protocol/src/object.rs` |

Every review row and every Build form comes from an IDL. The hand-written decoders and forms are gone. The one byte-level rule left is the runtime's: a leading System `AdvanceNonceAccount` makes a durable-nonce transaction. Compute budget and the nonce advance are under Build's Advanced section.

## Embedded IDLs

`crates/wallet-core/idls/` holds the IDL of each program the app needs without an import (System, Stake, Token, Token-2022, compute budget, the Ed25519 programmatic signer, the two Squads programs, Vector, the devnet Raydium CLMM), unchanged from upstream apart from display nodes. The Squads and Raydium files are Anchor IDLs converted to Codama; the Raydium one also declares the remaining accounts its swap and liquidity instructions read, which an Anchor IDL cannot, and shows its token amounts in base units because it cannot read a mint's decimals. To update one, replace the file; the `builtin.rs` tests check its program id, its name and the pinned default instruction.

They are compiled into the binary and never written to the index. Importing a file for the same program replaces the embedded one, and removing that import brings it back. An embedded IDL cannot be removed and is left out of an export.

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
