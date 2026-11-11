# Templates

A template is a short TOML file that asks a few questions and writes Build's blocks from the answers. The blocks are compiled and read on Review and Sign exactly as blocks made by hand are, so a template saves typing and nothing else: it cannot reach anything Build cannot.

```
template (TOML) ─► check (form: inputs, options) ─► answers ─► expand ─► IDL blocks ─► compile ─► Review and Sign
```

How an instruction is built from an IDL is in [idls.md](idls.md); how the screen is drawn, in [design-system.md](design-system.md). The plan the feature was built from, with what is still to come (templates of your own, import and export), is [plan-templates.md](plan-templates.md).

## Rules

They are what keeps a template from being a way around Review and Sign.

1. **A template writes IDL blocks, never bytes.** It names a program, an instruction and that instruction's fields. A program with no IDL on the network in use, an instruction the IDL lacks or a field it does not name fails the template by name, before anything is built.
2. **Review and Sign reads the transaction, not the template.** A template's name appears only in the source note ("Compiled on Build from the template “Create a token”"). Its labels, hints and summary never appear on Review and Sign.
3. **Nothing a template writes is hidden.** The form lists every value the template fixes and every value it works out, beside the fields it asks for.
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
| `[[input]]` | A question: `name`, `label`, optional `hint`, `optional`, `default` (a literal, a built-in name or a program alias), `option` (asked only while that option is on) |
| `[values]` | Derived values, one function call each: `name = "function arg arg"`. An argument is a reference or a literal. Calls do not nest; a value names another value instead |
| `[[block]]` | One instruction, in the order written: `program`, `instruction`, `accounts`, `arguments`, optional `when` (an option name: the block exists only while it is on) |

An input or value that nothing uses refuses the file, and so does one larger than 64 KiB or with more than 32 blocks, inputs or values, 16 options or 16 program aliases.

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

The two size functions read the template's own enabled blocks, so the checkboxes and the size cannot disagree. They count each enabled block whose program is a Token-2022 alias and whose `mint` is bound to the same reference as the function's argument, each extension once. A length that would equal a multisig's (355) is two bytes more, as Token-2022 makes it. An `initialize…` instruction on that mint the table does not know refuses the template when it is checked. A Token-2022 mint is created at the size without metadata and funded for the size with it, because `initializeTokenMetadata` grows the account itself. The extension sizes are a table in Rust, checked by running every shipped option against the real program ([Adding a shipped template](#adding-a-shipped-template)).

## Chain facts

Some values only the node knows: the rent of an account of a given size, a mint's decimals, the program that owns a mint. Expanding a template returns the ones it still needs as `Fact`s (`Expansion::needs`), and every fact its blocks rest on, known or not (`Expansion::uses`). A fact whose argument is not known yet (a mint not typed in) is not needed yet. The caller answers them in `Facts` and expands again:

- **Online**, `wallet_flows::template_facts(rpc, needs)` reads them: rent one size at a time, as the node answers it, and every mint in one request. A mint that does not exist, or is not a mint of Token or Token-2022, refuses the lot and names the address.
- **With no node**, each fact is a field of the form, labelled as what it is (its `Display`: "Rent for 234 bytes, in lamports"). The person types it as the chain has it.

## The code

| Part | Where |
| --- | --- |
| The format, checking, expansion, functions, sizes, the shipped templates | `wallet_core::template` (`crates/wallet-core/src/template/`), files in `crates/wallet-core/templates/` |
| Reading chain facts from a node | `wallet_flows::template_facts` (`crates/wallet-flows/src/template.rs`) |
| The open template on Build's form | `OpenTemplate`, `TemplateSource`, `FactRead` in `app-ui-kit` (`state/forms/build.rs`) |
| Opening, answering, reading facts, Show as blocks, compiling | `app-ui-kit` (`actions/template.rs`) |
| The picker and the form | `app-ui-build` (`ui/build/template.rs`); the widgets are Build's own (`ui/build/fields.rs`) |

The functions a caller uses:

- `Template::parse(&str)` reads a file; `Template::to_toml()` writes one `parse` reads back to the same template.
- `check(&template, idls)` checks it against the IDL catalog of the network in use and returns its form: each input with the widget its fields call for (`InputKind`: an address, or an argument with its kind and scale), and the options. A `Problem` says what is wrong, by block and field.
- `expand(&template, &answers, &env, &facts, idls)` writes the blocks as `IdlDraft`s, in order, with the rows each block fixes and works out, what is still missing (`Missing`: an input, the wallet, the fee payer) and the facts still needed. It never fails on a blank answer: the field stays blank.
- `Answers` holds each input's text as typed (in the field's unit) and the options that are on. An input with no entry takes its default; one with an empty entry was cleared and stays blank. `Template::default_text` gives what an input shows before it is answered.
- `Env` holds `$wallet` (the address the selected wallet acts as: a multisig's vault, not its member), `$fee-payer` (Build's fee payer field) and the address of the keypair made for each `new-keypair` value. The core never sees a secret.
- `BUILTIN_TEMPLATES` and `builtin_template(id)` give the shipped ones.

## The form

"Templates", in Build's head, lists every template: its name, its summary and a chip ("built-in"). One that fails `check` on the network in use is listed greyed, with the first problem in words. With no block on Build and no template open, the same list sits under the instruction rail. Opening a template while Build holds blocks asks before discarding them.

An open template takes the place of the rail and the block editor. Fee payer stays above it and Advanced options under it. The form shows:

- the template's name and summary, and **Close template**, which leaves Build empty;
- each input not under an option, with its label, hint and the widget Build uses for its kind: the address picker, a select for an enum, a checkbox for a boolean, a text field for the rest, with an amount's unit in its label and its decimal places in the placeholder. An input with a default shows it (for `$wallet`, the wallet's address) until it is changed, and can be cleared;
- each option as a checkbox with its hint; turning it on shows its inputs under it;
- the chain facts, as described below;
- **Fixed by this template · n** and **Worked out · n**, closed: for each block in the transaction, numbered as Review and Sign numbers it, every field the template sets to a literal and every field it works out, with its value (an address drawn as the app draws addresses, a value not known yet saying so).

A `new-keypair` value gets a key when the template opens. It goes into Build's list of keys made on this device (`BuildForm::generated`), where Review and Sign finds it, and stays the same while the template is open. Clear and Close template leave it there, as Build's own keys stay.

Online, the facts are read once the answers have been still for 400 ms, with a quiet "Reading from the node…" line, away from the window; a reply for an older question, or for a template closed since, is dropped. If the node refuses, the error names the address and the facts become fields. Offline (Airlock Offline, a phone in offline mode: anywhere `Session::rpc` returns an error), they are fields from the start, under the inputs. Each field stays while the blocks rest on its fact, so it does not vanish once filled.

**Show as blocks** writes the expansion into Build's blocks and leaves the template, with a note saying where they came from. It is one way: the blocks are ordinary blocks, and changing them does not change the template.

**Review and Sign** expands again. A required input left blank stops it: the error names each one and its field is marked. Otherwise the blocks compile through the same path as Build's own button (`compile_instructions` in `actions/build.rs`): the same fee payer and Advanced options, the same simulation and blockhash, errors numbered by instruction. Review and Sign's note reads "Compiled on Build from the template “<name>”", plus whatever compiling adds.

## Adding a shipped template

1. Write `crates/wallet-core/templates/<id>.toml`. The id is the file's stem and stays the same across versions.
2. Add it to `BUILTIN_TEMPLATES` in `crates/wallet-core/src/template/builtin.rs`, in the order the list shows them.
3. Run `cargo test -p wallet-core template`. It parses and checks every shipped template against the embedded IDLs of every network its programs run on, and expands and builds each with every option on.
4. Add a run to `crates/flow-tests/tests/integration/templates.rs`: with no option, with each option alone, and with every option the program allows together. Each run lands the transaction against the real programs in the test VM and reads the result back. An option ships only if its run passes.
5. A Token-2022 extension the size table (`crates/wallet-core/src/template/size.rs`) does not know refuses the template. Add its size there, with a flow test that lands it.
6. Add a line to `CHANGELOG.md`.
