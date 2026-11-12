# IDLs

How Airlock uses IDLs to read and build instructions, and the rules that keep what the user reads tied to what they sign. The dynamic client itself, and the gaps of its Anchor converter, are in `crates/codama/README.md`. What an IDL read from the chain may and may not unlock at signing time is in [signing.md](signing.md).

## Rules

- **One IDL per program, not per transaction.** A transaction is a list of instructions, each with its own program id, so one message has a set of IDLs. Lookup, checksum, display and QR pins are all per program id. One instruction failing to match an IDL does not blank the others.
- **IDLs are kept per network.** A program can run at different addresses on mainnet and devnet, and the same address can run different versions on each. The catalog keeps the files of each network apart (`IdlNetwork`: mainnet, devnet, testnet, localnet) and shows the network of the cluster in use. A custom RPC reads the IDLs of the public cluster its genesis matched; one that matched none reads localnet's.
- **IDL bytes never travel in a QR code.** A zipped Token-2022 IDL is about 34 KiB, a device may need dozens of programs, and the QRS1 payload cap is 32 KiB (`MAX_PAYLOAD` in `qr-protocol`). IDLs move by file import on each device. QRS1 carries at most a 32-byte SHA-256 pin per program and a few small account blobs (the context note, kind 3).
- **The signing device explains bytes with the IDL stored on that device.** A pin from the other device is a comparison, not a document to adopt. Display never changes the signed message.
- **IDLs are read from the chain only.** `wallet_flows::idl` reads the program-metadata account first, then the legacy Anchor IDL account. Nothing fetches an IDL over HTTP.
- **A check never overwrites.** Checking a program online hashes the bytes the chain publishes and records whether they match the stored file, or the document the file was made from. "Differs" does not switch the explanation to the remote document; the user may import the remote bytes on purpose, which replaces the stored one. With airgap set, nothing is fetched and the status stays "not checked".
- **An IDL can leave an instruction unread, and nothing worse.** Any program's author can publish one, and it is read as soon as a transaction names the program. A file that nests too deep is refused (`MAX_IDL_DEPTH`). A type that links to itself, expands too far, or holds too many empty items fails to decode, so that instruction reads raw, and the rest of the transaction and of the IDL are not affected. The limits are in `crates/codama/README.md`, and `crates/wallet-core/tests/hostile_idl.rs` runs each case on a stack the size of a main thread's.
- **The sentence is text; tooltips are a span list beside it.** `interpolated_intent` stays a plain string. Hover tooltips come from `IntentSpan`s produced by the same walk. Public names (`toly.sol`) never go into the signed sentence or the QR object.

## Where it lives

| Piece | Where |
| --- | --- |
| Catalog: imported files, checksums, the index | `crates/wallet-core/src/idl/catalog` |
| Core IDLs, compiled in | `crates/wallet-core/idls/`, listed in `BUILTIN_IDLS` (`idl/builtin.rs`) |
| Third-party IDLs, the IDL pack | `resources/idls/`, listed in `resources/idls/manifest.toml`; packed by `crates/app-ui-kit/build.rs` with the `idl-pack` crate, read by `wallet_core::idls_from_pack` and `app-ui-kit/src/platform/idl_pack.rs` |
| Networks | `crates/wallet-core/src/idl/network.rs` |
| Building an instruction from an IDL | `crates/wallet-core/src/idl/build`, `idl/draft.rs`, the Build screen's tabs |
| Explaining an instruction | `crates/wallet-core/src/review/explain` |
| Reading an IDL from the chain | `crates/wallet-flows/src/idl.rs` |
| The context note (kind 3) | `crates/qr-protocol/src/object.rs` |

Every review row and every Build form comes from an IDL. The hand-written decoders and forms are gone. The one byte-level rule left is the runtime's: a leading System `AdvanceNonceAccount` makes a durable-nonce transaction. Compute budget and the nonce advance are under Build's Advanced section.

Build types an amount in the unit Review shows it in, from the same `amountNumberDisplayNode`: a fixed number of decimals and a unit (`transferSol.amount`, 9 and SOL; `createAccount.space`, bytes), or the decimals another argument of the instruction holds (`transferChecked.amount`, from `decimals`, which the form asks for first). The text becomes the integer digit by digit, never through a float, and a value with more decimal places than the unit has, or one the field's integer cannot hold, is refused. A scale that needs an account's data (`mintTo.amount`, from its mint) is not read on Build: that field is typed in base units and says so. Opening an instruction on Build (`edit::idl_draft_for`) writes its amounts in the same unit, so building it again gives the same bytes. The accounts the IDL derives and the arguments left to their default fold under "Filled in for you", at the end of the instruction.

## Embedded IDLs

The app ships the IDL of each program it needs without an import, in two sets that read the same once in the catalog (`EmbeddedIdl`, origin `Builtin`):

- **The core**, compiled into the binary: `crates/wallet-core/idls/`, listed in `BUILTIN_IDLS`. These are the programs the wallet's own code builds instructions for, so they cannot fail to load: System, Stake, Token, Token-2022, compute budget, the Ed25519 programmatic signer, the two Squads programs, the Squads clear signing program, Vector.
- **The IDL pack**, for third-party programs: `resources/idls/`, listed in `manifest.toml` (the Raydium CLMM and CP swap on devnet and mainnet, the devnet Raydium router, Orca Whirlpool, Jupiter v6, the three memo programs, the SPL Governance program Realms runs, program-metadata, the upgradeable BPF loader). This is where new programs go.

Both are unchanged from upstream apart from what each entry's `changes` says. The Squads and Raydium files are Anchor IDLs converted to Codama; the Raydium ones also declare the remaining accounts their instructions read (the CLMM's tick arrays and swap route, the CP swap config's new owner), which an Anchor IDL cannot, and show their token amounts in base units because they cannot read a mint's decimals. The Raydium router publishes no IDL, so `raydium-router.json` is written by hand from transactions the program executed (its docs page lists account orders the chain does not use). Its swaps name the owner and the accounts at fixed positions; the hops, whose accounts depend on each pool program, are one hidden remaining-accounts group. `SwapBaseOut` (tag 9) is left out until a transaction shows its layout. The Orca Whirlpool file is the IDL its program-metadata account publishes on mainnet: the program has the same address and version on devnet, where its IDL accounts are missing or stale. Its V2 instructions declare the transfer hook and supplemental tick array accounts that `remainingAccountsInfo` slices out. The Jupiter file is the Anchor IDL its program publishes on mainnet, the only network it runs on (on devnet its address is a system account). Its swaps read as a sentence: who swaps how much of which mint for a quoted amount of which other mint, and the slippage allowed, in percent. Each amount is scaled by the decimals of its own mint, read from the mint account (the file carries the Token program's `mint` layout for that), so the sentence waits for the node's answer; `route` and `routeWithTokenLedger` name no input mint, so their input stays in base units. The program enforces a floor of the quote less the slippage (an exact-out swap, a ceiling of the quote plus it), so the sentence names the quote and the slippage, not a minimum the instruction does not carry. The platform fee and an output account other than the owner's are fields. Each step of the route plan reads its AMM's accounts after the named ones, so they are one hidden remaining-accounts group, and the route plan itself is hidden. The v01 Anchor conversion leaves `optionalAccountStrategy` unset, which this port reads as omitted; Anchor 0.30 passes the program's own address for an optional account left out, so the file sets `programId` on each instruction, and review leaves such an account out of the fields rather than show the program's address. The memo files are the IDL in the memo repository, which is v4's: v3 (`MemoSq…`) and v1 (`Memo1…`) take the same data, so their files are it with their own address and version, and a test checks that. v1 does not check the signers passed to it. The Realms governance program publishes no IDL on chain; `realms-governance.json` is the Shank IDL of Mythic's governance SDK, converted to Codama and checked against spl-governance v3.1.3, which the program on both networks runs. The source declares tags 29 and 30 as the required-signatory instructions, where the program has its transaction buffer instructions, so those two are left out with the deprecated instructions the program rejects, and those tags read raw. Accounts that a realm's config or plugins add or leave out (in `createRealm`, `setRealmConfig`, `createProposal` and `castVote`) are remaining-accounts groups, because naming by position would put the wrong names on them. Review opens an `insertTransaction` the way it opens a Squads stored transaction (`review/explain/realms.rs`, `InnerKind::ProposalTransaction`), reading the instructions from the data's own bytes, so the IDL hides them as a field. Realms is not a wallet: nothing routes a transaction through a governance, and Edit does not rewrite what a proposal stores. The program-metadata file, for the program the app reads IDLs from, is the IDL that program publishes for itself, the same on mainnet and devnet. The loader-v3 file, for the upgradeable BPF loader that deploys, upgrades and closes most programs, is the Shank IDL in the loader-v3 repository with the two transforms its `codama.mjs` applies before generating clients: the program is `loaderV3` at `BPFLoaderUpgradeab1e…` (the Shank file declares the address of its Core BPF port), and `write`'s bytes take a u64 length prefix, as bincode writes a `Vec<u8>`. The loader takes an optional account by leaving it out, so optional accounts are omitted. A sentence cannot name an account the instruction leaves out, so `setAuthority` says in its sentence that leaving the new authority out is final, and names the new authority, when there is one, as a field. The bytes a `write` carries are not a field. The repository's IDL has no accounts; the file adds the four states of `UpgradeableLoaderState` (uninitialized, buffer, program, program data), each told by its u32 tag. A buffer's and a program data account's authority takes 33 bytes even when there is none, and the bytes after a none tag may still hold the old authority; the program bytes that follow are not read, so the inspector decodes the state and not the executable, and shows the executable's hash beside it (see [Builds an IDL was reviewed for](#builds-an-idl-was-reviewed-for)). `Migrate` (8) and `ExtendProgramChecked` (9), which `solana-loader-v3-interface` dropped in 7.0, and the optional trailing flags its 8.x adds to `deployWithMaxDataLen`, `upgrade` and `close`, are left out: such an instruction reads raw, and the trailing flag is not shown. The Squads clear signing file, in the core because a Squads approval made on an offline device builds its instructions, is the repository's Anchor IDL converted to Codama. `verifyProposal` carries the whole content of the vault transaction a proposal stores and aborts on chain unless the chain stores exactly that; its sentence names the proposal, the transaction index, the multisig, the member and the vote, and the instructions it carries are hidden as a field, because review opens them from the data's own bytes (`wallets/squads/clear_sign.rs`, `InnerKind::SquadsCheck`) on any network, with or without this IDL. The address lookup tables it reads after its named accounts are a remaining-accounts group. `verifyProposalCpi` is the same check for a member that signs through a program (a programmatic signer, or a Vector account or its system PDA), run inside that program's `Submit` or `Passthrough`: the same arguments under its own tag, the member as a signer after the instructions sysvar, hidden because the sentence names it, and a sentence that says the member signs through a program; review opens it the same way. `verifyBufferHash` and `assertProgramRevision` show the hash in full, to compare with what `solana-verify get-buffer-hash` and `get-program-hash` print. Devnet's program-metadata account for it publishes the IDL of an earlier build, which lacks `assertAccountHash`, `assertProgramRevision` and `verifyProposalCpi`, so the source is the repository. To add or update one of the pack's, put the file in `resources/idls/` and its entry in `manifest.toml` with its source checksum; a JSON file the manifest does not list fails the build. The tests in `idl/pack.rs` and `idl/builtin.rs` check each file's program id and name, that no two files (core or pack) claim a program on the same network, every instruction's form on every network, and the pinned default instructions.

Embedded IDLs are never written to the index. Importing a file for the same program on the same network replaces the embedded one, and removing that import brings it back. An embedded IDL cannot be removed and is left out of an export. `.gitattributes` keeps both folders LF on every checkout: the bytes shipped are the checked-out bytes, and their checksum is compared across devices.

### The IDL pack

`crates/app-ui-kit/build.rs` packs `resources/idls` into `crates/app-ui-kit/assets/idls.pack` (ignored by git) and pins the pack's SHA-256 in the build (`AIRLOCK_IDL_PACK_SHA256`). The pack is zlib over the manifest, as JSON, and each listed file (format in the `idl-pack` crate); it is about a twentieth of the JSON it holds. `asset!` ships it beside the app: in the bundle's assets on desktop, as one hashed, cacheable file of the page on the web.

- **Desktop** reads it while the catalog opens, so the catalog is whole from the first frame. Run without a bundle (`cargo run`, tests), the asset resolves to the file in the source tree.
- **Web** fetches it once after the page loads. Until it arrives, the catalog holds the core only, and Review and Programs say "Loading the IDLs that ship with the app…".
- **A pack that does not match the pin is refused whole**, before anything in it is read: a file changed after install, or a server sent something else. So is one that claims a program on a network the core already covers. Its programs then read raw, and Review and Programs say why.

The pin makes the pack as trusted as the binary that carries it: an IDL decides what the signer shows before signing.

### Networks and sources

Each embedded IDL, core or pack, lists the networks it is read on, its source (`IdlSource`) and what was changed in it:

| File | Networks | Source |
| --- | --- | --- |
| System, Stake, Token, Token-2022, compute budget | all | the program's repository; display nodes added |
| Ed25519 signer | devnet | the program-metadata IDL devnet publishes, brought to the repository's IDL at `2b2cc7f`: devnet's predates the transition signer the executor and nonce program take since 2026-11-09 |
| Vector | devnet | the program-metadata IDL devnet publishes |
| Squads v4 | mainnet, devnet | the 2.1.0 Anchor IDL devnet publishes (mainnet still publishes 2.0.0) |
| Squads clear signing | devnet | the repository's Anchor IDL at `0922251`: devnet's program-metadata account publishes an earlier build's, without `assertAccountHash`, `assertProgramRevision` and `verifyProposalCpi` |
| Squads smart account | mainnet, devnet | the repository's Anchor IDL, newer than either network's: they lack `executeTransactionSyncV2` and `closeEmptyPolicyTransaction` and name older accounts, while the mainnet program accepts every instruction the repository's lists (checked 2026-11-09) |
| Raydium CLMM, CP swap (`DRay…`) | devnet | the program-metadata IDL devnet publishes |
| Raydium CLMM, CP swap (`CAMM…`, `CPMM…`) | mainnet | the program-metadata IDL mainnet publishes |
| Raydium router | devnet | written by hand |
| Orca Whirlpool | mainnet, devnet | the program-metadata IDL mainnet publishes |
| Jupiter v6 | mainnet | the Anchor IDL mainnet publishes |
| Memo v4, v3, v1 | mainnet, devnet, testnet | the memo repository's IDL, v4's |
| Realms governance | mainnet, devnet | the Shank IDL in Mythic's governance SDK repository |
| Program metadata | mainnet, devnet | the program-metadata IDL mainnet publishes |
| Upgradeable BPF loader (loader-v3) | all | the loader-v3 repository's Shank IDL at `b9c8ab3`, with its `codama.mjs` transforms; account states added by hand |

A program at different addresses on two networks has a file for each: the file declares its address, and often accounts that differ by network too (Raydium's admin and fee receiver). The mainnet Raydium files are the devnet ones with those addresses swapped, which is all the two networks' published IDLs differ in, and a test checks that.

A file made from what a program publishes on chain keeps the SHA-256 of those bytes and the account they were read from (`IdlChainSource`). Our file's checksum C1 is never the chain's C2: it was converted, given display nodes, or given accounts the source cannot declare. A check reads the account the file was made from (`IdlRecord::compared_account`) and records both comparisons (`IdlCheck::same`, `IdlCheck::source`). When the chain publishes C2, the status is "matches its source": the file is current and nothing is fetched to replace it. When it publishes something else, the status is "differs", and the source is where to start an update. The app reads a program's IDL from the chain only when the network in use has none stored, so a download never replaces an embedded file.

### Builds an IDL was reviewed for

Nothing on chain ties an IDL to a program build. The program-metadata and Anchor IDL accounts are written by whoever controls the program, at any time, and say nothing about the code they describe. A verified build ties a program's executable to a repository commit, not to an IDL. The tie is our own: `resources/program-builds.toml` has one entry per program and network, stating that someone read the embedded IDL (its SHA-256) against the program running one executable (its hash, as `solana-verify get-program-hash` prints it). The entry also records the upgrade authority, the source commit when known, and the uploader of the verify PDA whose build OtterSec reproduced.

`program-watch` (`crates/program-watch`, never shipped) reads every embedded IDL, core and pack, and for each network it runs on reports:

- a program redeployed (its executable hash changed) or a changed upgrade authority,
- an embedded file changed after its review,
- the on-chain IDL a file was made from changed or gone,
- OtterSec starting or stopping verifying the running build (mainnet only: OtterSec does not cover devnet),
- a program with no entry, with an entry filled in from the chain to paste once reviewed.

`.github/workflows/program-watch.yml` runs it daily and keeps one issue labelled `program-watch` open while anything needs action. The executable hash comes from the program-data account over RPC (`wallet_core::program_code`). Only the verification status comes from OtterSec's API, and only in CI.

The account inspector shows the same hash, so a user can compare a program with a build they trust. An account of the upgradeable loader shows, above its decoded state: for a program, its program-data account (which opens in the inspector), the program hash, the upgrade authority or that nobody can upgrade it, and the slot it was last deployed in, read from that program-data account through `wallet_flows::loader::program_data` (a closed program, or an address that is not a program-data account of the loader, says so instead); for a program-data account opened directly, the same rows; for a buffer, the buffer hash, the digest `solana-verify get-buffer-hash` prints (SHA-256 of the bytes after its 37-byte header, trailing zeros cut, `wallet_core::program_code::buffer_hash`), and its authority. Each hash is in full, in lowercase hex, with a copy button and the `solana-verify` command that prints it. The hashing runs off the window, since a program-data account holds up to 10 MiB. Programs of the other loaders (the non-upgradeable BPF loaders, loader v4) show no hash there.

### Stored files

The index lists each imported file with its network. Bodies are `idls/<network>/<program id>.json` on desktop and `airlock.idl.<network>.<program id>` in a browser. Only the network in use is read into memory; switching cluster reads the other's files. An index from before networks has no `network` on its records: the files are moved under the network in use when the app opens. An export holds every network's files as `<network>.<program id>.json`, and an older bundle with `<program id>.json` files is imported for the network in use.

### What is read from the chain

A site can name any program, and the IDL it publishes is read and kept, so what is read from the chain is a cache: at most 64 IDLs or 32 MiB for each network (`IdlStore::keep_within_limits`). The oldest read goes first, and is read again when something needs it. A file imported by hand, and what ships with the app, is not counted and is never let go.

### Additional programs

A Codama file can carry other programs in `additionalPrograms`. The catalog gives each one a record of its own (`IdlRecord::host` names the file it is read from), so build and review use it like any other program. Its checksum, origin and online check are the host file's. Such a record is never stored or exported, cannot be removed on its own, and goes when its host file goes.

- A file of the program's own, embedded or imported, wins over an entry carried by another file.
- When several files carry the same program, the fullest entry wins: Codama nodes plus non-empty docs lines (`companions::completeness`). On a tie, the host embedded first wins (the core in the order of `BUILTIN_IDLS`, then the pack in the order of its manifest), then the lower host address.

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

`accounts` carries what a node said is stored behind what the transaction approves, as the raw account, for a device that cannot ask. See [What a transaction does not hold](signing.md#what-a-transaction-does-not-hold).

[signing.md](signing.md) lists the object kinds.

- Pins are one per distinct program id. Pins and authorizations are kept: more than a note holds, and it is not built.
- The receiver decodes a note only when a transaction is loaded and the message hash matches. Otherwise it says the note is for a different transaction and keeps the transaction. A second note for the same hash replaces the first.
- For each instruction whose program has a pin, the pin is compared with the local checksum. Unequal or missing locally: that program's line says so, and the note's account bytes are not fed into that instruction's display, because a different IDL must not decode a mint.
- An online device reads the accounts a display spec needs (`IdlExplanation::accounts_needed`, such as a mint for its decimals) from its own node, and reads the transaction again with them. It keeps an account when the program whose instruction reads it owns it, since bytes anyone could write to an account of their own must not set a mint's decimals. It also keeps an initialized mint of the Token or Token-2022 program for any reader: only those programs write a mint they own, so its decimals are true whoever reads them, as a swap does for the mints it trades (`display_accounts`, which tells a mint from a token account or multisig by its length and, for Token-2022, its account type byte). It takes what it keeps over the note's. An amount scaled from a note's account says **unverified scale**; one scaled from the node's does not.
- The note is ignored for signing. Signatures cover the versioned message only. Review, build and spans all work with no note.

## Known gaps

- An account whose signer status the IDL leaves open (`either`, as on token authorities) is always signed for. There is no multisig signer list.
- An IDL names accounts by position. When an instruction carries more accounts than its IDL describes, and the IDL allows no extras, review says so on that row (`IdlExplanation::unnamed_accounts`), because the names may be on the wrong accounts. The embedded stake IDL describes the layout without sysvar accounts, so a stake instruction in the older layout, which most clients still send, is flagged this way.
- Out of scope: Codama spec 2.0, and the `nodes-from-anchor` gaps listed in the Codama readme (the builder surfaces those as a missing default, not as a silent address).
