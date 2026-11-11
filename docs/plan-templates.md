# Plan: templates on Build

Status: phases 1 to 6 are **built** (2026-11-11). Phases 5 to 7 were added the same day, after Tiago used the first four: a crash on import, two extensions the size functions left out, and a browser for templates that reads and writes their text. What is left after those is under [Open](#open). Each phase below says what it adds; a phase is done when its tests pass and its line is ticked in [Phases](#phases).

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

The format, the built-in names, the functions and chain facts are in [templates.md](templates.md), which is the reference from phase 3 on.

## Phases

- [x] **1. Build's fields.** Scaled amounts and folded fields on the Build form.
- [x] **2. The core.** The format, checking, expansion, functions and facts in `wallet-core` and `wallet-flows`; the shipped templates; their tests against the real programs.
- [x] **3. The form.** The picker and the template form on Build, "Show as blocks", compiling to Review and Sign.
- [x] **4. Your own.** Save as template, storage, import and export.
- [x] **5. Loose ends.** The crash on importing a name that exists; an enum behind a defined type as a choice on Build; default account state and confidential transfer in the size table.
- [x] **6. The browser.** The Templates dialog as a list and a reader, with each template's questions, blocks and text.
- [ ] **7. Templates as text.** Writing, editing and importing a template as text in the same dialog, checked as it is typed.

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

As built: the module is public and its parts are reached as `wallet_core::template::{check, expand, Answers, Env, Facts, Fact, …}`; only `Template`, `TemplateError`, `BuiltinTemplate`, `BUILTIN_TEMPLATES` and `builtin_template` are also at the root (`Fact` is taken there). `check` takes the network's catalog as `&dyn IdlCatalog` and returns the inputs (with their kind and scale) and options; the rows the template fixes and works out come from `expand`, per block, with their resolved text. `Answers` tells an input with no answer (it takes its default) from one cleared on purpose (it stays blank). A `token-program-of` block whose mint is not known yet is laid out as Token's, with no program. `template_facts` asks rent one size at a time (the node has no call for several) and every mint in one request. The size table holds the extensions a test lands against the VM's Token-2022 (11.0.0): the six shipped ones and interest-bearing, transfer hook, group pointer, group member pointer, pausable and scaled UI amount. Phase 5 added default account state and confidential transfers with their fee.

The shipped templates:

| File | Asks | Options |
| --- | --- | --- |
| `create-token.toml` | decimals, mint authority, freeze authority (optional) | Mint a first supply (to, amount) |
| `create-token-2022.toml` | the same | Name and symbol (metadata pointer to the mint itself, then token metadata: name, symbol, URI), Transfer fee (fee, maximum fee), Close authority, Permanent delegate, Non-transferable, Mint a first supply |
| `mint-tokens.toml` | mint, to, amount | none. Uses `token-program-of` and `decimals-of`, creates the recipient's associated token account if missing (`createAssociatedTokenIdempotent`), then `mintToChecked`, with the wallet as mint authority |

An option ships only if its test passes against the program the test VM carries. All of them did, and all six of `create-token-2022` land together: the VM's Token-2022 accepts a transfer fee on a non-transferable mint.

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

As built: Build's form holds `template: Signal<Option<OpenTemplate>>` (the source, the parsed template, the answers, facts typed and facts read, the address of each `new-keypair` key, the inputs marked missing) and `fact_read: Signal<FactRead>`. `TemplateSource` has one variant, `Builtin(id)`, for phase 4 to add a saved one beside. The keys go to `generated`, where Sign finds Build's own. `compile` in `actions/build.rs` and the template's Review and Sign share `compile_instructions`. The core gained `Template::default_text` (what an input shows before it is answered) and `Expansion::uses` (every fact the blocks rest on, known or not, so a fact field does not vanish once filled). The facts become fields offline and also when the node refuses. The disclosures number blocks by their place in the transaction, as Review and Sign does, not by their place in the file. Airlock Offline compiles a template only on a durable nonce, as it does Build's blocks.

### 4. Your own

- **Save as template** on Build, when it holds at least one block and all are IDL blocks: a dialog lists each block's fields with a tick "Ask each time". Ticked fields become inputs named after the field; ticked fields holding the same value become one input. An address that is a keypair made on Build becomes a `new-keypair` value everywhere it appears. The rest are literals. Name and summary are typed.
- **Storage.** The TOML text, one file each: `templates/<slug>.toml` in the data folder on desktop and Android, `airlock.template.<slug>` in a browser, following how IDL bodies are stored. Saved checked, as anything only the user can recreate.
- **Import and export**, from the picker: one `.toml` file. Importing shows what the template fixes before it is kept, and keeps nothing that fails `parse`. A saved name that exists asks before replacing. Built-in templates cannot be removed or replaced, and can be exported.

As built: the tick became a choice per field (keep, ask each time, the wallet in use, the fee payer), because a field that holds the wallet or the fee payer should follow whoever opens the template rather than be fixed or asked. A blank field is asked or left out, and one the program needs is always asked. Fields asked that hold the same value and take the same widget are one question. `wallet_core::template::SavePlan` makes the template, which reads back from its file to itself and checks against the IDLs; opened with the blocks' values as its answers it writes the same blocks, tested on transfers, a scaled token transfer, a mint with a key made here and an overridden account, and the blocks of every shipped template. Expanding now marks only a derived account as overridden (`IdlDraft::manual`), as Build's "Change" does; it marked an account the IDL cannot fill too, which Build never does. `Storage` in `app-core` gained `list` and `remove` for the store of templates. `TemplateSource` has `Yours` and `Broken` beside `Builtin`; the list is read into `BuildForm::kept` the first time Build is shown. The import preview is read from the file, not from an expansion, so it shows blocks under an option and a template that does not fit this network. A name a shipped template has, or its id, is refused for one of yours, saved or imported. The phone offers no import or export, as it offers no IDL import. [templates.md](templates.md#your-own-templates) has the whole of it.

### 5. Loose ends

- **The import crash.** Importing a template whose name is already taken panics with an `unwrap` on `None`. No code of ours on that path unwraps, so it is reproduced first, in a debug build with a backtrace, with a name one of the user's has and with a name a shipped one has, through the file dialog's debug seam and with the reasoning for the real dialog. The cause is fixed where it is, and a test holds it where a test can reach it. Phase 7 replaces the import dialog, which is no reason to skip this: the cause may be a pattern used elsewhere.
- **An enum behind a defined type.** `classify` in `idl/build/form.rs` reads a `definedTypeLinkNode` as a field nobody can fill, so `initializeDefaultAccountState.state` and Token's `setAuthority.authorityType` cannot be built at all. It follows the link to the program's own defined type, to a bounded depth: an enum whose variants hold nothing becomes `IdlArgKind::Enum`, a choice. Anything else stays as it is. Building, opening for edit and building again gives the same bytes.
- **The size table** gains `initializeDefaultAccountState` and `initializeConfidentialTransferMint`, each landed alone against the VM's Token-2022 as the others were. A confidential mint with no auditor needs no ElGamal key; with one, the key is 32 bytes the IDL types as an address. `initializeConfidentialTransferFee` joins them if it lands beside a transfer fee, and is left out with the reason if not.
- The shipped templates do not change. A mint whose accounts start frozen cannot take a first supply, and a template has no way to say two options do not go together.

As built: the crash came from the file dialog, not from the name. The click handler opened rfd's blocking dialog, whose message loop runs on the window's thread while dioxus holds the window's elements borrowed for the click (desktop delivers each event through a webview request, `__events`, handled where it arrives). The dioxus waker fired inside that loop and rendered the window; the first node the render made panicked ("RefCell already borrowed"), the handler then panicked on a scope that was gone (the `unwrap` on `None`), and the process aborted. It took a change on screen before the dialog: Import clears the note of the import before it, so a second import crashed and a first did not, whatever the names. Through the debug folder no dialog opens, which is why phase 4's runs never saw it; it was reproduced by driving the real dialog with UI Automation, three runs out of three, and is gone in the same runs since. Import and export now await rfd's async dialog in a task (`import_from`, `export_template`), so the click returns before the dialog opens, and a kit test holds that the preview comes from the task. Programs opens its IDL dialogs the blocking way too (`actions/programs.rs`: `import_one`, `import_folder`, `export_folder`); it changes nothing on screen before them, so it crashes only if something else renders while its dialog is open, and it is left for its own change. A link is followed to the program's own defined types at most two deep (`classify_in`); an enum whose variants hold nothing is a choice, anything else stays raw, and seeds and scales still do not follow links. 21 arguments of the shipped IDLs became choices, among them `setAuthority.authorityType` in Token and Token-2022, `initializeDefaultAccountState.state` and `updateDefaultAccountState.state`, the stake authorizations' `stakeAuthorize` and program-metadata's `format`, `encoding`, `compression` and `dataSource`; a test builds each variant of each, opens it on Build and builds the same bytes, on every network. The lengths the VM took are default account state 1, confidential transfer mint 65 and confidential transfer fee 129. Neither ElGamal key is read when it is set up: an auditor key or a withdraw key of 32 bytes of `0xff`, which is no point, lands as well as the Ristretto basepoint, so the tests use the basepoint and say so. The confidential transfer fee lands only beside both a transfer fee and confidential transfers, and the program refuses those two together without it; a frozen default needs a freeze authority, and `uninitialized` is refused.

### 6. The browser

The Templates dialog becomes what Programs is for IDLs: a list on one side, the template picked on the other. On a phone it is one at a time, with a way back, as Programs is.

- **The list.** Every template, shipped and the user's, with a filter over name and summary, the chip, and the greyed state of one that does not fit this network. A file of the user's that is not a template is listed too.
- **The reader.** The name, the summary, where it comes from, and for one that does not fit this network every problem. Then tabs, as an IDL has: **Questions · n** (each question with its hint, what it holds until answered, and the options with the questions asked under each), **Blocks · n** (every block in order: program, address, instruction, the option it runs under, and every field it sets with where the value comes from: asked, fixed, worked out and how), and **Text**, the file as it is kept, with Copy.
- **Actions** on the template picked: **Use this template** (asks first when Build holds blocks, as today), Export, and Remove for one of the user's. The rows of the list carry no buttons.
- **Kept as written.** One of the user's is kept as the text it was saved from, so Text, Copy and Export give back the same bytes, comments included. Save as template still writes `to_toml`. `TemplateSource::Yours` and `Broken` carry the text.
- Build's empty state keeps its short list: a row there opens the template at once.

As built: the reader is drawn from `TemplateView` (`app-ui-kit`, `actions/template_browser.rs`), which `template_view` reads from a template and its text and nothing else, so phase 7 can give it a template parsed from text that is not kept: `ReaderQuestions` lists the questions as the form asks them, with the widget each takes in words from `check`'s form (none when the template does not fit), and `ReaderBlock` every field each block sets, accounts apart from arguments, in the IDL's order, as `FieldOrigin::Asked`, `Fixed` or `Worked`. A kit test holds that every field of every shipped template is on its Blocks tab once, with the origin the file gives it. The import preview reads with the same tabs, in place of its own lists and the Fixed and Worked out disclosures, and once kept the dialog shows the template in its list. `TemplateSource::Yours` carries the text kept and `Broken` the text when it was text; `keep_import` takes the file's text and parses it again, so the bytes kept are the bytes read, while Save as template still writes `to_toml`. `Template::parse` refuses control characters other than tab, line feed and carriage return, and Unicode bidirectional controls, in the text by line and, written as escapes in a string, by key (`TemplateError::Hidden`). Build's short list leaves out a file that is not a template, which the dialog lists, and gains **Browse templates**. On a wide window the dialog opens on the first template that can be used; on a phone on the list. Two leftovers of phase 5 are fixed here. A required choice with no answer showed its first variant while it held nothing, so picking that variant changed nothing; Build's shared `argument_input` now puts "Choose one" first while the text is not one of its variants, which a block of Build's own never is, since its draft starts on the first. And the file dialogs, awaited since phase 5 and so not modal to the window, could be opened twice: a flag on the desk (`TemplateDesk::dialog`) is held while one is open, disables Import and Export, and is let go when the dialog returns, from a task spawned in the scope that owns the flag, so closing the Templates dialog meanwhile does not drop it. Checked with the real Windows dialogs, import and export twice each, answered through UI Automation. The phone's one-pane layout comes only from the mobile shell (`PhoneLayout`), so it was checked in the Android app on an emulator, not in a desktop window.

### 7. Templates as text

The same dialog writes templates. Rule 3 holds: whatever is about to be kept, the dialog shows what it does beside the text, read from the text as it stands.

- **Ways in.** **New template** (a short starter that parses), **Edit** on one of the user's, **Edit a copy** on a shipped one, **Import a file** (the file's text, not kept yet), and pasting. A broken file of the user's opens here to be mended. The import dialog of phase 4 goes: an imported file is a text not yet saved.
- **The editor.** The text in a plain monospace field, and beside it (under it when the dialog is narrow) the Questions and Blocks of phase 6 for the text as typed. Checked as it is typed: the reader's error, with TOML's own line and excerpt; then every problem `check` finds on this network; then the name (a shipped one's is refused, another of the user's says saving replaces it).
- **Saving.** Allowed when the text parses and the name is free or replaces one of the user's knowingly. Problems on this network do not stop it: it is listed greyed, as an import was. Editing one of the user's and changing its name renames it: the old file goes once the new one is kept. Leaving with changes not saved asks first.
- **On a phone** there is still no file picker, and now no need: Copy on Text and paste into New template carry a template between devices.

## Tests

- `wallet-core`: parsing (every refusal), `to_toml` round trip, checking against IDLs, expansion (blank answers, options on and off, each function), the scale and fold rules of phase 1, and every built-in template on every network.
- `flow-tests`, against the real Token, Token-2022 and associated-token programs in the test VM: each shipped template with no option, with each option alone, and with every option that the program allows together. Each run expands, compiles, signs with the wallet and the generated mint key, lands, and reads the mint back (decimals, authorities, each extension, supply). This is what makes the size table trustworthy.
- `app-ui-kit`: Build's form state through a template and back to blocks, and save as template from blocks.

## Open

- Token metadata's size depends on its text. If the program's check of the funded size disagrees with ours by a few bytes, the function is fixed against the VM, never padded.
- Whether a saved template should record the checksum of each IDL it was made against, to say "this program's IDL changed since" on open. Not in v1: a name that no longer matches already refuses.
- Files on a phone. Android has no file picker the app can open; phase 7 carries a template as text instead. The system share sheet or a QR would be the next step.
- Hints and defaults for the questions of a saved template. Save writes each question with its field's name in words and no hint or default; Edit, from phase 7, is the way to add them.
- Token-2022's token group and group member, written onto the mint after it is initialized as metadata is, are not sized.
