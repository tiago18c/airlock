# Codama dynamic client

Runtime Rust port of the TypeScript `@codama/dynamic-*` packages. Given a Codama IDL, these crates encode and decode instructions, derive PDAs, parse accounts and events, and render a human-readable display. Nothing here generates Rust source.

`wallet-core` builds and explains every instruction through `codama-dynamic-client`, and converts Anchor IDLs with `codama-nodes-from-anchor`. How the app uses IDLs is in [docs/idls.md](../../docs/idls.md).

`vendor/codama-rs` is an older local fork. It is not a dependency of these crates, and it is the wrong tree to diff when updating this port. `codama-program-cli` stays in that vendor tree.

## Crates

| Crate | TypeScript package | Role |
| --- | --- | --- |
| `codama-visitors-core` | `@codama/visitors-core` | Visitor dispatch, `NodePath`, `LinkableDictionary`, `DynamicError` |
| `codama-dynamic-codecs` | `@codama/dynamic-codecs` | `Value` and encode/decode of Codama type nodes |
| `codama-dynamic-resolution` | `@codama/dynamic-address-resolution` | Account addresses and PDAs |
| `codama-dynamic-instructions` | `@codama/dynamic-instructions` | `InstructionBuilder`, plus display and intent |
| `codama-dynamic-parsers` | `@codama/dynamic-parsers` | Identify and decode account, event, and instruction bytes |
| `codama-dynamic-client` | `@codama/dynamic-client` | `ProgramClient` facade. Start here |
| `codama-nodes-from-anchor` | `@codama/nodes-from-anchor` | Anchor IDL JSON (legacy and 0.30+) into a `RootNode` |

Dependency direction: parsers and resolution depend on codecs; instructions depend on parsers, resolution, and codecs; the client depends on all of them. `nodes-from-anchor` depends only on `codama-nodes` at build time. Its dev-dependency on the client is for fixture tests.

Node types and the error enum come from crates.io, not from this tree:

- `codama-nodes` 0.13.2
- `codama-errors` 0.13.2
- `codama-nodes-derive` 0.13.2
- `codama-syn-helpers` 0.13.2

There is no nested Cargo workspace under `crates/codama`. Members are listed in the root `Cargo.toml`.

## Snapshot

Recorded 8 October 2026. Diff later changes against these revisions, not against `vendor/codama-rs`.

### TypeScript behavior

Repository: <https://github.com/codama-idl/codama>

Branch **`1.x`**, commit **`759dd1f81054ef8d70778147e2dd3724aa85c2b9`** (6 October 2026, `build(deps-dev): bump turbo from 2.11.6 to 2.11.7`).

Packages this port tracks:

- `packages/dynamic-client`
- `packages/dynamic-codecs`
- `packages/dynamic-instructions` (including `src/display`)
- `packages/dynamic-parsers`
- `packages/dynamic-address-resolution`
- `packages/visitors-core`
- `packages/nodes-from-anchor`

`main` is Codama spec 2.0 and is not the baseline. Do not diff `main` into this tree until the "Future work" note on spec 2.0 is in scope.

```bash
git diff 759dd1f81054ef8d70778147e2dd3724aa85c2b9..origin/1.x -- \
  packages/dynamic-client \
  packages/dynamic-codecs \
  packages/dynamic-instructions \
  packages/dynamic-parsers \
  packages/dynamic-address-resolution \
  packages/visitors-core \
  packages/nodes-from-anchor
```

### Rust node schema

Repository: <https://github.com/codama-idl/codama-rs>

crates.io **0.13.2**. `CODAMA_VERSION` in that crate is `"1.8.0"` (`codama-nodes` `src/generated/codama_version.rs`). Checksums in the root `Cargo.lock`:

| Package | Checksum |
| --- | --- |
| `codama-nodes` | `23b0d12fde4b749f4e28ef83a9b83e4aa00697a7847dfb0aa6d6e633a3f6421e` |
| `codama-errors` | `95bea6c7b0724cfabade2ec9aab60687f7bf2b8b039233054d70fc68d4fa3c7c` |
| `codama-nodes-derive` | `0bc8c5044a8de1157b61996276736ea1f6eabd83d55ceb5f6b810e4e3ac873c1` |

`codama-rs` publishes the IDL node schema and the visitors that *generate* IDLs from Rust programs. It does not execute an IDL. Execution lives in this directory.

Unpublished commits on `codama-rs` after the 0.13.2 tag did not change the 1.8 node schema at the time of this port. A newer crates.io release is the signal to revisit `codama-nodes`, not a random commit on that repo.

## How to use

Codama IDL JSON (a document whose root is `kind: "rootNode"`):

```rust
use codama_dynamic_client::{get_instruction_display, ProgramClient, Value};
use std::collections::HashMap;

let client = ProgramClient::from_json(idl_json)?;

let instruction = client
    .method("transfer")?
    .account("source", source)
    .account("destination", destination)
    .account("authority", authority)
    .signer("authority")
    .argument("amount", 400_000u64)
    .build()?;

let parsed = client
    .parse_instruction(&instruction)?
    .expect("instruction came from this IDL");

// `accounts` is address -> encoded account bytes. Empty means nothing was fetched.
// `names` replaces those addresses in the field list only, not in the sentence.
let display = get_instruction_display(client.root(), &parsed, &HashMap::new(), &HashMap::new())
    .expect("parsed path ends in an instruction");
```

`display.intent` is the short label. `display.interpolated_intent` is the filled sentence, or `None` when a placeholder cannot be filled. `display.fields` is the fallback list.

Before rendering, ask which accounts the sentence needs and fill a dictionary:

```rust
use codama_dynamic_client::{
    get_instruction_display_from_dictionary, required_accounts_for_display, DisplayDictionary,
    DictionaryAccount,
};

let needed = required_accounts_for_display(client.root(), &parsed);
let dictionary = DisplayDictionary {
    accounts: needed
        .into_iter()
        .map(|pk| DictionaryAccount {
            address: pk.to_string(),
            data: fetch_account_bytes(&pk),
        })
        .collect(),
    names: Vec::new(),
};
let display = get_instruction_display_from_dictionary(client.root(), &parsed, &dictionary);
```

Anchor IDL JSON is not a Codama root. Convert it first. Legacy IDLs often have no program address, so pass one:

```rust
use codama_nodes_from_anchor::root_node_from_anchor;

let root = root_node_from_anchor(anchor_json)?;
let client = ProgramClient::from_root_with_program_id(root, program_id)?;
```

`ProgramClient::from_root` fails when `program.publicKey` is missing or not a pubkey. `with_program_id` overrides an address that is present.

Standalone PDAs:

```rust
let mut seeds = HashMap::new();
seeds.insert("authority".into(), Value::PublicKey(authority.to_bytes()));
let (address, bump) = client.derive_pda("myPda", seeds)?;
```

Everything is synchronous. TypeScript's async `fetch` and async resolvers are a preloaded map or a `HashMap` you pass in.

Tests that lock the wire format live in `codama-dynamic-client/tests/spl_fixtures.rs`. They rebuild SPL Token and Token-2022 instructions and compare them to vectors produced by the TypeScript client. The Token-2022 fixture is the upstream `idl.json` and includes display nodes. In debug that test takes about two minutes.

```bash
cargo test -p codama-dynamic-client -p codama-dynamic-instructions -p codama-nodes-from-anchor
```

## What this port does not include

Left on the TypeScript side on purpose:

- Codama spec 2.0 (the `main` branch node schema).
- `generate-client-types` and the other code-generation CLIs.
- Superstruct validators. Callers pass a typed `Value`.
- Extra `visitors-core` helpers: byte-size visitors, debug strings, node selectors.
- The `nodes-from-anchor` `defaultVisitor` pipeline (PDA extraction, fixed account sizes, argument flattening). The raw tree is enough to encode, decode, and build.
- Decoded-node formatters that were still on unmerged TypeScript branches when 1.x was snapshotted.

Display never changes signed bytes. Codecs walk the type tree and ignore display nodes. `skip: always` removes a member from the field list only.

## Future work

- Spec 2.0, once an IDL we need is `version: "2.0.0"` and `codama-nodes` on crates.io speaks that schema. Until then, keep diffing `1.x`.
- Anchor v00 `kind: "programId"` seeds. The runtime encoder writes a `programIdValueNode` as the raw 32-byte derivation program id. `codama-nodes-from-anchor`'s v00 converter still drops the whole PDA when it sees that seed, so a converted legacy IDL will not derive it.
- `accountFieldValueNode` as an *argument default* while building a transaction. Display can decode a preloaded account. The encoder still returns an error (`account field requires fetched account data`) and will not put fetched data into the signed instruction.
- Anchor v01 PDA seeds whose `path` contains a dot (`a.b`). The converter skips that account's default instead of emitting a nested read.
- Name substitution inside the interpolated sentence. Names from `DisplayDictionary` apply to the field list. The sentence stays base58, matching the TypeScript interpolator.

## Gotchas

**A missing amount scale drops the whole sentence.** If an amount's decimals or unit cannot be resolved, the field text is `"<integer> (raw)"` and `interpolated_intent` is `None`. Fall back to `fields`. Omitted decimals means scale 0, not "unknown": Token-2022 `transfer` has unit `"base units"` and no decimals, so `400000` renders as `400000 base units`.

**`whenInjected` hides a member only after the injection resolves.** `transferChecked` hides `decimals` because `provides` points that injection at the `decimals` argument, which is already in the instruction data. If the injection is an `accountField` and those bytes were not preloaded, the member stays visible and the amount is degraded. `required_accounts_for_display` lists the account either way.

**Only `accountField` triggers a fetch.** A plain account reference in the sentence is the address already on the instruction. It does not need account data. An `accountField` with no `path` is still listed (the account would be fetched) but it does not become a scalar, so the injection does not count as resolved.

**`programId` seeds are raw address bytes.** A constant PDA seed of type `programIdValueNode` is the 32-byte program id used for `find_program_address` (the PDA's program id when set, otherwise the instruction's program). The seed's declared type codec is not used. Literal seeds such as an ASCII prefix are encoded with that codec, as before.

**The v00 converter and the runtime disagree about `programId`.** See "Future work". Do not "fix" a converted IDL by assuming the PDA is merely missing a seed; the converter deleted the PDA.

**Optional accounts and signers are not booleans.** `is_optional` is `Option<bool>`. `IsSigner` is `True`, `False`, or `Either`. For remaining accounts, `Either` matches every meta. Non-final groups take metas whose signer flag matches; the last group takes whatever is left. Those metas are `ParsedInstruction.remaining_accounts`, the accounts after the named ones. Named pairing is by position.

**Skipped display members are still signed, and the sentence may still name them.** `initializeMint` hides the rent sysvar from the field list. The builder still puts that account on the instruction. A `${data.tag}` placeholder is filled even when that argument is `skip: always`.

**Injected values have two maps.** `InstructionBuilder::inject` / `AddressResolver::inject` supply values while *building*. Display `provides` on the instruction node supply values while *rendering*. A display injection does not change encoded bytes. Build-time lookup order is the inject map, then the node's fallback, then `InjectedValueMissing`.

**Account bumps are available only after account resolution.** The builder derives accounts before it encodes arguments, so an omitted `accountBump` argument can read the bump. `create_account_metas` does not take the inject map.

**Decoded `Value` is not the Kit shape.** Options are `Value::Option`, not `{ __option: "Some" }`. Enums are `Value::Enum { variant, fields }`. Variant names are pascal-cased by uppercasing the first character only (`closeAccount` becomes `CloseAccount`). A struct field named `__kind` is also accepted when formatting. Public keys are `[u8; 32]` and render as base58.

**`CamelCaseString` lowercases and camel-cases in its constructor.** `"my_value"` becomes `"myValue"`. Read it with `as_str()`. It derefs to `String`, so `Display` is the stored camelCase text, not the original spelling. IDL names that are already camelCase stay as they are.

**Account-field decoding needs a root-relative path.** The parsed instruction path must start at the `RootNode`. The account's `accountLink` is resolved with `LinkableDictionary` against that path, then `get_node_codec` decodes the preloaded bytes. A failed link or a failed decode is "no value", not a hard error, so display degrades instead of failing the render.

**Dates and durations do not use `chrono`.** Instants are UTC `YYYY-MM-DDTHH:MM:SS.mmmZ`. Years outside 0..=9999, a zero `ticksPerSecond`, or a non-integer fall back to the raw number and do not mark the sentence degraded. Durations are `HH:MM:SS` (hours may exceed 24). A negative duration falls back to the raw number. Amount scaling is integer-only; a fractional number, or more than 80 decimals, is degraded.

**String slices count chars, like JavaScript `String.slice`.** They are not byte offsets.

**Identification misses return `None`, not an error.** `parse_instruction` and `parse_*_data` return `Ok(None)` when no discriminator matches. `get_instruction_display` returns `None` when the path does not end in an instruction node. A program whose only candidate of the kinds asked for has no discriminators (memo v4's one instruction) is the match for any bytes, as in TypeScript; the bytes may still fail to decode.

**Large IDLs are slow in debug.** Parsing the Token-2022 fixture dominates `spl_fixtures`. Display metadata does not change the wire bytes those vectors check.

**`serde_json` errors are not `DynamicError` directly.** `ProgramClient::from_json` maps them through `codama_errors::CodamaError`, which `DynamicError` then wraps. `?` does not chain two `From` impls.

**Do not re-run a blind rename of `PdaValue`.** An earlier edit turned `N::PdaValue` into `N::PdaValuePda`. The node enum variant is `PdaValue`; `PdaValuePda` and `PdaValueProgramId` are the inner program-id and PDA choices.
