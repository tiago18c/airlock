# Plan: templates on Build

Status: **in progress**. Decided with Tiago on 2026-11-11. Each phase below says what it adds; a phase is done when its tests pass and its line is ticked in [Phases](#phases).

Build asks for every account and argument an IDL names, in the program's base units. That is the right tool for an instruction nobody planned for, and too raw for the things people do often. A template is a short text file that asks a few questions and writes the Build blocks from the answers. The blocks are compiled and read on Review and Sign exactly as blocks made by hand are.

```
template (TOML) ─► form (inputs, options) ─► answers ─► IDL blocks ─► compile ─► Review and Sign
```

Background: [idls.md](idls.md) for how an instruction is built from an IDL, [architecture.md](architecture.md) for where code goes, [design-system.md](design-system.md) for how a screen is drawn.

## What this adds

- **Build's own fields read better** (phase 1, useful with or without templates): an amount is typed in the unit its IDL declares (SOL, percent), and the accounts the IDL fills in fold away.
- **Templates that ship with the app**: Create a token, Create a Token-2022 token (with extensions as options), Mint tokens.
- **Templates of your own**: save what is on Build as a template, ticking the fields to ask for each time. Export one to a file and import it on another device.
- **A way back to blocks**: "Show as blocks" turns a filled template into ordinary Build blocks, to change what the template does not ask.

Left out, and why:

| Left out | Why |
| --- | --- |
| Conditions other than an option being on, comparisons, arithmetic, loops | They make the file a program to audit. A repeat is written by copying a block. What needs logic stays a flow in Rust, as Stake and Wallets are |
| Conflicts between options | The program refuses what it does not allow, and the simulation says so |
| Fetching a template over the network | Same rule as IDLs: files ship with the app, are written here, or are imported by hand |
| Swaps and anything that needs a quote or a route computed off chain | A template only binds answers to fields |
| Raw and carried blocks in a saved template | v1 templates hold IDL blocks only. Save refuses and says which block |
| Advanced options (compute budget, nonce) in a template | They stay Build's, and apply to a template's transaction as to any other |

## Rules

These hold for every phase. They are what keeps a template from being a way around Review and Sign.

1. **A template writes IDL blocks, never bytes.** It names a program, an instruction and that instruction's fields. A program with no IDL on the network in use, an instruction the IDL lacks or a field it does not name fails the template by name, before anything is built.
2. **Review and Sign reads the transaction, not the template.** A template's name appears only in the source note ("Compiled on Build from the template “Create a token”"). Its labels, hints and summary never appear on Review and Sign.
3. **Nothing a template writes is hidden.** The form lists every value the template fixes and every value it derives, beside the fields it asks for.
4. **Templates are never fetched.** Built-in ones are compiled into the binary; the rest are saved or imported as files.
5. **Money and sizes are never guessed.** Rent comes from the node. With no node it is a field to fill.
6. **The format is closed.** A file names functions from a fixed list in Rust. An unknown key, function or reference refuses the whole file.

## The format

TOML, version 1. One namespace: inputs, values, program aliases, options and the built-in names share it, and a file that uses a name twice is refused. `$name` is a reference; `$$` is a literal dollar. Any other string is a literal, written as it would be typed into the Build field it goes to (a base58 address, a number in the field's unit, an enum variant, `true`).

```toml
format = 1
name = "Create a Token-2022 token"
summary = "A new mint, with the extensions you choose."

[programs]
system = "11111111111111111111111111111111"
token = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"

[[option]]
name = "transfer_fee"
label = "Transfer fee"
hint = "Keeps a share of every transfer in the receiving account."

[[input]]
name = "decimals"
label = "Decimals"
default = "9"

[[input]]
name = "authority"
label = "Mint authority"
default = "$wallet"

[[input]]
name = "freeze_authority"
label = "Freeze authority"
optional = true

[[input]]
name = "fee"
label = "Fee"
option = "transfer_fee"

[[input]]
name = "max_fee"
label = "Maximum fee"
hint = "The most one transfer can be charged, in the token's base units."
option = "transfer_fee"

[values]
mint = "new-keypair"
space = "token-2022-mint-space $mint"
funded = "token-2022-mint-funded-space $mint"
lamports = "rent-for $funded"

[[block]]
program = "$system"
instruction = "createAccount"
accounts = { payer = "$fee-payer", newAccount = "$mint" }
arguments = { lamports = "$lamports", space = "$space", programAddress = "$token" }

[[block]]
when = "transfer_fee"
program = "$token"
instruction = "initializeTransferFeeConfig"
accounts = { mint = "$mint" }
arguments = { transferFeeBasisPoints = "$fee", maximumFee = "$max_fee", transferFeeConfigAuthority = "$authority", withdrawWithheldAuthority = "$authority" }

[[block]]
program = "$token"
instruction = "initializeMint2"
accounts = { mint = "$mint" }
arguments = { decimals = "$decimals", mintAuthority = "$authority", freezeAuthority = "$freeze_authority" }
```

| Key | Meaning |
| --- | --- |
| `format` | `1`. Any other number refuses the file |
| `name`, `summary` | Shown in the picker and at the head of the form. Never on Review and Sign |
| `[programs]` | Alias to address. A block's `program` is an alias, or a value whose function returns a program |
| `[[option]]` | A checkbox: `name`, `label`, optional `hint`, optional `default = true`. No nesting, no else |
| `[[input]]` | A question: `name`, `label`, optional `hint`, `optional`, `default` (a literal or a reference), `option` (asked only while that option is on) |
| `[values]` | Derived values, one function call each: `name = "function arg arg"`. An argument is a reference or a literal. Calls do not nest; a value names another value instead |
| `[[block]]` | One instruction, in the order written: `program`, `instruction`, `accounts`, `arguments`, optional `when` (an option name: the block exists only while it is on) |

Order is the order of the `[[block]]` entries, so an option's block sits where it must run (most extensions before `initializeMint2`, metadata after). TOML does not keep the order between two different arrays, which is why an option does not hold its own blocks.

**An input has no declared type.** It takes the widget of the field it is bound to: an address picker for an account or a public key argument, a select for an enum, a checkbox for a boolean, a scaled amount where the IDL declares a scale (phase 1). Bound to several fields, they must agree, or the file is refused. An input used only as a function's argument takes that parameter's type. An optional input left blank leaves its fields blank, which the IDL form reads as it does on Build (`None`, the IDL default, or an optional account left out); a required field bound to an optional input refuses the file.

**A field the template does not mention** is left to the IDL: a derived account is derived, an argument with a default takes it. A required field with no binding and no default refuses the file when it is checked against the IDL.

### Built-in names

| Name | Value |
| --- | --- |
| `$wallet` | The address the selected wallet acts as: what its transactions move funds from (a multisig's vault, not its member) |
| `$fee-payer` | Build's fee payer field |

### Functions

The whole list. A function is pure, or names the chain fact it needs.

| Function | Returns | Needs |
| --- | --- | --- |
| `new-keypair` | The address of a keypair made on this device for this form, kept as Build's "New keypair" keeps one. Stable while the form is open | nothing |
| `associated-token-account <owner> <mint> <token program>` | The associated token address | nothing |
| `token-2022-mint-space <mint>` | The size to create a Token-2022 mint at: the base mint and every extension an enabled block of this template initializes on `<mint>`, without token metadata | nothing |
| `token-2022-mint-funded-space <mint>` | The size to fund: the above, plus the token metadata an enabled `initializeTokenMetadata` block of this template writes to `<mint>`, from the lengths of the values bound to it | nothing |
| `rent-for <bytes>` | Lamports | the node's rent for that size |
| `decimals-of <mint>` | The mint's decimals | the mint account |
| `token-program-of <mint>` | Token or Token-2022, whichever owns the mint | the mint account |

The two size functions read the template's own enabled blocks, so the checkboxes and the size cannot disagree. A Token-2022 mint is created at the size without metadata and funded for the size with it, because `initializeTokenMetadata` grows the account itself. The extension sizes are a table in Rust, checked by running every shipped option against the real program ([Tests](#tests)).

**Chain facts.** Expanding a template returns the facts it still needs. Online, the app reads them from the node (`wallet-flows`, over `SolanaRpc`) and expands again. With no node (Airlock Offline, or offline mode) each missing fact is a field of the form, labelled as what it is ("Rent for 234 bytes, in lamports"), under the template's own inputs. A block whose program is `token-program-of` is checked against both programs' IDLs.

## Phases

- [ ] **1. Build's fields.** Scaled amounts and folded fields on the Build form.
- [ ] **2. The core.** The format, checking, expansion, functions and facts in `wallet-core` and `wallet-flows`; the shipped templates; their tests against the real programs.
- [ ] **3. The form.** The picker and the template form on Build, "Show as blocks", compiling to Review and Sign.
- [ ] **4. Your own.** Save as template, storage, import and export.

Each phase is one commit series on `t3/build-pane-templates`, in order. Each leaves `cargo fmt --all --check`, CI's Clippy step and the touched crates' tests green, and adds its line to `CHANGELOG.md` when people will notice it.

### 1. Build's fields

*Scaled amounts.* The embedded IDLs already say how an amount reads: `amountNumberDisplayNode` on a number type gives decimals and a unit (`transferSol.amount`: 9, SOL; `initializeTransferFeeConfig.transferFeeBasisPoints`: 2, %; `createAccount.space`: no decimals, bytes), and an `injectedValueNode` names a key the instruction `provides` (`transferChecked.amount` takes `decimals` from the instruction's own `decimals` argument; `mintTo.amount` from the mint account). Review uses this; Build ignores it and says "Amounts are in the program's base units".

- `IdlArgField` gains a scale: fixed (decimals and an optional unit), from another argument of the same instruction (by name), or none. A scale that needs an account's data is none in this phase: that field stays in base units and says so.
- `IdlDraft` holds an argument as typed. For a scaled field that is the display unit ("1.5"), and `parse_argument` turns it into the integer exactly: no floats, more decimal places than the scale refuses the value, and a scale from another argument reads that argument's text and asks for it first when it is blank.
- `edit::idl_draft_for` writes the same text back, so a transaction opened for edit shows "1.5", and building it again gives the same bytes. A test builds, opens and rebuilds each scaled field of the core IDLs.
- The field's label carries the unit ("amount · SOL"). A numeric field with no scale reads "base units". The blanket hint goes.

*Folded fields.* Accounts the IDL fills (`IdlAccountSource::Derived` and not overridden) and arguments left to their default (`blank_uses_default`, blank) move under one closed disclosure at the end of the block, "Filled in for you · n", using the kit's `Accordion`. Each row keeps what it has today, "Change" included. A derived account that could not be filled, or one the user overrode, stays in the main list.

Touches `wallet-core` (`idl/build`, `idl/draft.rs`, `edit/idl.rs`), `app-ui-build` (`fields.rs`, `editor.rs`), `app-ui-kit` CSS if the disclosure needs any.

### 2. The core

A new module `wallet_core::template`, placed in `layers.rs` above `idl` and `spl`. No Dioxus, no network.

- `Template::parse(&str)`: structure, the namespace, references, function names and arity. Errors name the key and say what was expected.
- `Template::to_toml()`: writes a file `parse` reads back to the same template (phase 4 saves with it).
- `check(&Template, idls)`: every program has an IDL on this network, every instruction and field exists, every input's bindings agree on a kind, every required field is bound or left to an IDL default. Returns the form: inputs with their kinds, options, and the rows the template fixes.
- `expand(&Template, answers, env, facts, idls)`: returns the blocks as `IdlDraft`s in order, the rows it fixed and derived with their resolved values, what is still missing (an unanswered required input), and the chain facts it needs. Expansion never fails on a blank answer: the field is left blank, so "Show as blocks" works on a half-filled form.
- `answers`: input text by name and the set of options that are on. `env`: `$wallet`, `$fee-payer`, and the address of each `new-keypair` by value name (the caller makes and keeps the keys; the core never sees a secret).
- `BUILTIN_TEMPLATES`, from `crates/wallet-core/templates/*.toml` with `include_str!`. A test parses and checks each against the embedded IDLs of every network its programs run on, as the pinned programs are checked today.
- `wallet_flows::template_facts(rpc, needs)`: reads rent, mint decimals and mint owners, one request each kind where the node allows.
- `toml` is already in the lock file (`idl-pack`, `program-watch`); `wallet-core` takes the same version. It must build for wasm.

The shipped templates:

| File | Asks | Options |
| --- | --- | --- |
| `create-token.toml` | decimals, mint authority, freeze authority (optional) | Mint a first supply (to, amount) |
| `create-token-2022.toml` | the same | Name and symbol (metadata pointer to the mint itself, then token metadata: name, symbol, URI), Transfer fee (fee, maximum fee), Close authority, Permanent delegate, Non-transferable, Mint a first supply |
| `mint-tokens.toml` | mint, to, amount | none. Uses `token-program-of` and `decimals-of`, creates the recipient's associated token account if missing (`createAssociatedTokenIdempotent`), then `mintToChecked` |

An option ships only if its test passes against the program the test VM carries.

### 3. The form

- **Picker.** "Templates" in Build's page head opens a list: name, summary and a chip ("built-in", "yours"). A template that fails `check` on this network is listed disabled, with the reason. With no blocks on Build, the empty state offers the same list.
- **Template form**, in place of the rail and the block editor while a template is open: its name and summary, the inputs, each option as a checkbox that reveals its inputs, then "Fixed by this template · n" and "Worked out · n" as closed disclosures listing every constant and derived value with its block and field. Fee payer and Advanced options stay where they are.
- **Facts.** Online, missing facts are read as the answers that decide them settle (debounced, off the window as other reads are), with a quiet "Reading from the node…" line. Offline they are fields.
- **Show as blocks** writes the expansion into Build's blocks and leaves the template, with a note saying so. It is one-way; Clear starts over.
- **Review and Sign** expands, compiles through the path Build's own button uses, and adds the template's name to the source note. An unanswered required input is named in the error and marked on its field.
- **Keys.** A `new-keypair` value makes a `GeneratedKey` in Build's form state, where Sign already looks for them.
- Works on a phone and in Airlock Offline, which share these crates.
- `docs/templates.md`: the format, the rules, and how to add a shipped template. `idls.md` and `architecture.md` point to it.

Touches `app-ui-kit` (Build's form state and actions), `app-ui-build`, `app-ui-kit` CSS.

### 4. Your own

- **Save as template** on Build, when it holds at least one block and all are IDL blocks: a dialog lists each block's fields with a tick "Ask each time". Ticked fields become inputs named after the field; ticked fields holding the same value become one input. An address that is a keypair made on Build becomes a `new-keypair` value everywhere it appears. The rest are literals. Name and summary are typed.
- **Storage.** The TOML text, one file each: `templates/<slug>.toml` in the data folder on desktop and Android, `airlock.template.<slug>` in a browser, following how IDL bodies are stored. Saved checked, as anything only the user can recreate.
- **Import and export**, from the picker: one `.toml` file. Importing shows what the template fixes before it is kept, and keeps nothing that fails `parse`. A saved name that exists asks before replacing. Built-in templates cannot be removed or replaced, and can be exported.

## Tests

- `wallet-core`: parsing (every refusal), `to_toml` round trip, checking against IDLs, expansion (blank answers, options on and off, each function), the scale and fold rules of phase 1, and every built-in template on every network.
- `flow-tests`, against the real Token, Token-2022 and associated-token programs in the test VM: each shipped template with no option, with each option alone, and with every option that the program allows together. Each run expands, compiles, signs with the wallet and the generated mint key, lands, and reads the mint back (decimals, authorities, each extension, supply). This is what makes the size table trustworthy.
- `app-ui-kit`: Build's form state through a template and back to blocks, and save as template from blocks.

## Open

- Token metadata's size depends on its text. If the program's check of the funded size disagrees with ours by a few bytes, the function is fixed against the VM, never padded.
- Whether a saved template should record the checksum of each IDL it was made against, to say "this program's IDL changed since" on open. Not in v1: a name that no longer matches already refuses.
