# Templates

A template is a short TOML file that asks a few questions and writes Build's blocks from the answers. The blocks are compiled and read on Review and Sign exactly as blocks made by hand are, so a template saves typing and nothing else: it cannot reach anything Build cannot.

```
template (TOML) ─► check (form: inputs, options) ─► answers ─► expand ─► IDL blocks ─► compile ─► Review and Sign
```

How an instruction is built from an IDL is in [idls.md](idls.md); how the screen is drawn, in [design-system.md](design-system.md). The plan the feature was built from is [plan-templates.md](plan-templates.md).

## Rules

They are what keeps a template from being a way around Review and Sign.

1. **A template writes IDL blocks, never bytes.** It names a program, an instruction and that instruction's fields. A program with no IDL on the network in use, an instruction the IDL lacks or a field it does not name fails the template by name, before anything is built.
2. **Review and Sign reads the transaction, not the template.** A template's name appears only in the source note ("Compiled on Build from the template “Create a token”"). Its labels, hints and summary never appear on Review and Sign.
3. **Nothing a template writes is hidden.** The form lists every value the template fixes and every value it works out, beside the fields it asks for.
4. **Templates are never fetched.** Built-in ones are compiled into the binary; the rest are written here, saved from Build or imported as files.
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
| `name`, `summary` | Shown in the list, the reader and at the head of the form. Never on Review and Sign |
| `[programs]` | Alias to address. A block's `program` is an alias, or a value whose function returns a program |
| `[[option]]` | A checkbox: `name`, `label`, optional `hint`, optional `default = true`. No nesting, no else |
| `[[input]]` | A question: `name`, `label`, optional `hint`, `optional`, `default` (a literal, a built-in name or a program alias), `option` (asked only while that option is on) |
| `[values]` | Derived values, one function call each: `name = "function arg arg"`. An argument is a reference or a literal. Calls do not nest; a value names another value instead |
| `[[block]]` | One instruction, in the order written: `program`, `instruction`, `accounts`, `arguments`, optional `when` (an option name: the block exists only while it is on) |

An input or value that nothing uses refuses the file, and so does one larger than 64 KiB or with more than 32 blocks, inputs or values, 16 options or 16 program aliases.

A file is shown and kept as it is written, so nothing in it may read as something other than what it is. A control character other than tab, line feed and carriage return, or a Unicode bidirectional control (U+202A to U+202E, U+2066 to U+2069, U+200E, U+200F, U+061C), anywhere in the text, comments included, refuses the file and names its line ("line 3: holds U+202E, a bidirectional control…"). One written as an escape inside a string (a backslash, `u` and the code) is refused too, naming its key.

Order is the order of the `[[block]]` entries, so an option's block sits where it must run (most extensions before `initializeMint2`, metadata after). TOML does not keep the order between two different arrays, which is why an option does not hold its own blocks.

**An input has no declared type.** It takes the widget of the field it is bound to: an address picker for an account or a public key argument, a select for an enum, a checkbox for a boolean, a scaled amount where the IDL declares a scale (phase 1). Bound to several fields, they must agree, or the file is refused. An input used only as a function's argument takes that parameter's type. An optional input left blank leaves its fields blank, which the IDL form reads as it does on Build (`None`, the IDL default, or an optional account left out); a required field bound to an optional input refuses the file.

**An argument made of fields** (a struct, directly or through a type the IDL defines) is one field per member, named `argument.member`, as Build shows it: `arguments = { "params.commit" = "$commit" }`. The key is quoted, since TOML reads a bare dotted key as a table. **A list** of text, numbers, addresses or booleans is typed as a JSON array, and blank is the empty list, so it may be bound to an optional input.

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
| `program-data-of <program>` | The program data account the upgradeable loader keeps the program's code in | nothing |
| `idl-metadata-of <program>` | The program's canonical IDL account on program-metadata (seed `idl`) | nothing |

The two size functions read the template's own enabled blocks, so the checkboxes and the size cannot disagree. They count each enabled block whose program is a Token-2022 alias and whose `mint` is bound to the same reference as the function's argument, each extension once. A length that would equal a multisig's (355) is two bytes more, as Token-2022 makes it. An `initialize…` instruction on that mint the table does not know refuses the template when it is checked. A Token-2022 mint is created at the size without metadata and funded for the size with it, because `initializeTokenMetadata` grows the account itself. The extension sizes are a table in Rust, checked by running every shipped option against the real program ([Adding a shipped template](#adding-a-shipped-template)). Beside the shipped options it knows interest-bearing, transfer hook, group pointer, group member pointer, pausable, scaled UI amount, default account state and confidential transfers with their fee, each landed by a flow test. The program takes the confidential transfer fee only on a mint with a transfer fee and confidential transfers, and refuses those two together without it; a mint whose accounts start frozen needs a freeze authority. A template cannot say options go together, so the size functions do not check this: the simulation does.

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
| Build's short list and the form | `app-ui-build` (`ui/build/template.rs`); the widgets are Build's own (`ui/build/fields.rs`) |
| The Templates dialog: the list, the filter and the reader | `app-ui-kit` (`actions/template_browser.rs`: `TemplateView`, `ReaderQuestions`, `ReaderBlock`, `template_matches`); `app-ui-build` (`ui/build/browser.rs`) |
| The editor: what it writes, the text read as it is typed, saving it | `app-ui-kit` (`actions/template_text.rs`: `read_text`, `save_text`, `TextRead`, `NameCheck`; `TextEdit` and `EditTarget` in `state/forms/build.rs`); `app-ui-build` (`ui/build/text.rs`); the starter text is `STARTER_TEMPLATE`, from `crates/wallet-core/templates/starter/new-template.toml` |
| Making a template from Build's blocks, the slug a file is kept by | `wallet_core::template` (`save.rs`): `SavePlan`, `Choice`, `template_slug` |
| Keeping the user's templates on each platform | `app_core::storage` (`storage/templates.rs`) |
| Saving, reading, removing, importing, exporting | `app-ui-kit` (`actions/kept_templates.rs`); the file dialogs in `platform/template_files.rs` |
| The Save dialog, the remove confirmation | `app-ui-build` (`ui/build/kept.rs`) |

The functions a caller uses:

- `Template::parse(&str)` reads a file; `Template::to_toml()` writes one `parse` reads back to the same template. A refusal in the text says where it is (`TemplateError::place`: the line and the character), for a TOML error and a hidden character; a key that holds something the format does not allow is named instead.
- `check(&template, idls)` checks it against the IDL catalog of the network in use and returns its form: each input with the widget its fields call for (`InputKind`: an address, or an argument with its kind and scale), and the options. A `Problem` says what is wrong, by block and field.
- `expand(&template, &answers, &env, &facts, idls)` writes the blocks as `IdlDraft`s, in order, with the rows each block fixes and works out, what is still missing (`Missing`: an input, the wallet, the fee payer) and the facts still needed. It never fails on a blank answer: the field stays blank.
- `Answers` holds each input's text as typed (in the field's unit) and the options that are on. An input with no entry takes its default; one with an empty entry was cleared and stays blank. `Template::default_text` gives what an input shows before it is answered.
- `Env` holds `$wallet` (the address the selected wallet acts as: a multisig's vault, not its member), `$fee-payer` (Build's fee payer field) and the address of the keypair made for each `new-keypair` value. The core never sees a secret.
- `BUILTIN_TEMPLATES` and `builtin_template(id)` give the shipped ones.

## The Templates dialog

**Templates**, in Build's head, opens a dialog that is to templates what Programs is to IDLs: the list on the left, the template picked on the right. It is large, and each side scrolls on its own, so its head and its last row of buttons stay in place. On a wide window it opens on the first template that can be used on the network in use. On a phone it is one side at a time, as Programs is: it opens on the list, a row opens the template in its place, and **‹ Templates** goes back.

**The list** has every template: the shipped ones, then yours, by the file each is kept as. Each row shows the name, the summary cut to two lines and a chip, "built-in" or "yours". One that fails `check` on the network in use is greyed and says "Cannot be used on this network". A file of yours that is not a template is listed too, by its file name, and says "Not a template". Rows carry no buttons. The filter over the list reads names and summaries, whatever the case. Under the list stay the notes of what was not read: more than 64 files, or a store that could not be listed.

**The reader** shows the template picked:

- its name, summary and chip; for one of yours, the file it is kept as ("Kept on this device as `pay-a-friend.toml`."); for one that does not fit this network, "Cannot be used on this network." with every problem `check` gives;
- **Edit** (one of yours) and **Edit a copy** (every template), which open [the editor](#the-editor);
- **Questions · n**: every question in the order the form asks them, those asked always first. Each has its label, "(optional)", its hint, what it holds until answered ("Until answered: the wallet in use") and, when the template fits this network, the widget its fields give it in a few words ("an address", "a percentage", "a choice of uninitialized, initialized or frozen"). Then the options, each with its hint, "on at first" when it is, and the questions asked only with it. The count is the number of questions;
- **Blocks · n**: every block in file order, numbered as the file numbers them: the program as this app names it, its whole address with Copy (a block on `token-program-of` reads "The token program of the mint “Mint”"), the instruction, and "Only with “Transfer fee”" for one under an option. Then every field the block sets, accounts apart from arguments as Build's own block editor shows them, in the IDL's order, each with where its value comes from: **asked** (the question's label), **fixed** (the literal, an address drawn as the app draws addresses, a program alias with its `$name`, an amount with its unit) or **worked out** (in words, "a new keypair, made when it is opened", "the fee payer", "rent for $funded bytes, from the node", with its `$name`; in an app with no node, a chain fact reads "typed in as the chain has it", since the form asks for it). Nothing the template writes is left out of this tab: it is rule 3 for a template not opened yet;
- **Text**: the file as it is kept, with Copy. For a shipped template that is its file in `crates/wallet-core/templates/`; for one of yours, the bytes kept, comments and all.

The dialog's last row has **Close**; for the template picked, **Remove** (one of yours, asking first), **Export** (not on a phone) and **Use this template**, which closes the dialog and opens the template's form, asking first when Build holds blocks. A template that does not fit this network cannot be used, and the row says so. A file of yours that is not a template shows why, its text when it could be read, **Edit** to mend it, and **Remove**. **New template** and **Import a template** (not on a phone) sit in the dialog's head, beside the title. What the last save, export, import or removal did is said above the last row.

The reader is drawn from a template and its text (`TemplateView`), not from where it is kept, so a template that is not kept yet reads the same way: the editor shows its Questions and Blocks from the same parts.

## The editor

The Templates dialog writes templates too. While the editor is open it takes the place of the list and the reader, and its head says what is being done: "New template", "Editing “Pay a friend”", "A copy of “Create a token”", "Importing pay-a-friend.toml". The ways in:

- **New template**, in the dialog's head: a short starter text, a SOL transfer that asks "To" and "Amount", with comments that say what each part is. It reads and checks on every network.
- **Edit**, on one of yours: its text as it is kept. Saving writes over it. A file of yours that is not a template opens the same way, to be mended, when its text could be read.
- **Edit a copy**, on any template: the same text, tied to no file. Its name is then taken, and the editor says so until it is changed.
- **Import a template**: the file picked, read as text and not kept yet. A file that is not text, or is larger than a template can be, says why in the dialog and opens nothing.
- Pasting into the field is the same as typing.

Rule 3 holds here: nothing is kept before the person could see what it does. On a wide dialog the text is on the left and its reading on the right, each scrolling on its own; on a narrow dialog and a phone the reading is under the text, on one page that scrolls, and the last row of buttons stays in place. The reading is the reader's **Questions** and **Blocks**, one after the other, for the text as it stands.

The field is a plain text field in a monospace font, with no spell check, no capitals or corrections, and no wrapping, so a line in the field is a line of the file. The numbers of the lines run beside it and follow its scrolling. It is given its text once, when it opens, and never set back while it is typed in, so the caret stays where it is.

Above the reading, one status says what the text is, in this order:

1. Larger than 64 KiB: refused, and not read.
2. Not a template: the reader's words, TOML's or the format's ("block[1].accounts.destination: `$too` names nothing…"). When they point at a line, the line is shown with its number and the character pointed at marked, and **Show line 14 in the text** selects it in the field. A character that hides or turns text is shown by its code, `⟨U+202E⟩`. The reading then shows the last text that read as a template, dimmed, under "As of the last text that read as a template", or nothing if none did. **Save** is off.
3. Every problem `check` finds on the network in use, as a warning. They do not stop a save: the template is kept and listed greyed.
4. The name: a shipped template's name or id is refused ("“Create a token” is a built-in template. Give yours another name."); one of yours with the same slug says saving replaces it, and **Save** becomes **Replace**. The template being edited keeps its own name.

When all is well it says so, with what the template asks and writes: "Reads as a template: 2 questions, 1 block." Reading a text at the size limit takes about a millisecond in a release build, so it is read on every keystroke, with no delay.

**Save** keeps the text exactly as it is, comments, order and spacing included, through the same checked write as every other template, then shows the dialog's list with the template picked and a note. One of yours whose name changed is renamed: the new file is written and checked first, then the old one removed; if the old one cannot be removed, the note says it is still kept. A write that fails keeps the editor open with the text as it was, and says why.

**Cancel**, **Close**, a click outside the dialog, **New template** and **Import a template** ask "Discard your changes?" first when the text differs from what the editor opened with. The editor's text is held on Build's form (`BuildForm::editing`), so if something else takes the window, such as a site's request, the dialog opens on it again when Build is shown.

A template open on Build is not changed by its file: the form keeps the template it opened, and builds that, while its file is edited, renamed or removed in the dialog. The form then says its file has changed, or gone, since it opened it; closing it and opening the template again gives the new text.

## The form

With no block on Build and no template open, the templates that can be opened are listed under the instruction rail, under "Or start from a template", with **Browse templates** beside it to open the dialog. A row there opens its template at once; a greyed one says why it cannot be used. Opening a template while Build holds blocks asks before discarding them.

An open template takes the place of the rail and the block editor. Fee payer stays above it and Advanced options under it. The form shows:

- the template's name and summary, and **Close template**, which leaves Build empty;
- each input not under an option, with its label, hint and the widget Build uses for its kind: the address picker, a select for an enum, a checkbox for a boolean, a text field for the rest, with an amount's unit in its label and its decimal places in the placeholder. An input with a default shows it (for `$wallet`, the wallet's address) until it is changed, and can be cleared. A required choice with no default starts on "Choose one", so what is shown is what is held; an optional one starts on "None";
- each option as a checkbox with its hint; turning it on shows its inputs under it;
- the chain facts, as described below;
- **Fixed by this template · n** and **Worked out · n**, closed: for each block in the transaction, numbered as Review and Sign numbers it, every field the template sets to a literal and every field it works out, with its value (an address drawn as the app draws addresses, a value not known yet saying so).

A `new-keypair` value gets a key when the template opens. It goes into Build's list of keys made on this device (`BuildForm::generated`), where Review and Sign finds it, and stays the same while the template is open. Clear and Close template leave it there, as Build's own keys stay.

Online, the facts are read once the answers have been still for 400 ms, with a quiet "Reading from the node…" line, away from the window; a reply for an older question, or for a template closed since, is dropped. If the node refuses, the error names the address and the facts become fields. Offline (Airlock Offline, a phone in offline mode: anywhere `Session::rpc` returns an error), they are fields from the start, under the inputs. Each field stays while the blocks rest on its fact, so it does not vanish once filled.

**Show as blocks** writes the expansion into Build's blocks and leaves the template, with a note saying where they came from. It is one way: the blocks are ordinary blocks, and changing them does not change the template.

**Review and Sign** expands again. A required input left blank stops it: the error names each one and its field is marked. Otherwise the blocks compile through the same path as Build's own button (`compile_instructions` in `actions/build.rs`): the same fee payer and Advanced options, the same simulation and blockhash, errors numbered by instruction. Review and Sign names the template on the line under its title, "From the template “<name>” on Build", where it says where any transaction came from; the transaction's note reads "Compiled on Build from the template “<name>”", plus whatever compiling adds. Nothing else of the template reaches Review and Sign.

## Your own templates

### Saving

**Save as template** sits in Build's last row of buttons, after Review and Sign, while Build holds blocks and no template is open. Every block must be filled from an IDL: one written by hand or one that carries others is refused by its number ("Block 2 is written by hand, and a template holds only instructions filled from an IDL."), and so is one with no program or instruction yet.

The dialog asks for a name and an optional summary, then lists each block, numbered, with every account and argument its IDL form asks for, what it holds now (an address drawn as the app draws addresses) and what becomes of it (`Choice`):

| Choice | Writes | Offered |
| --- | --- | --- |
| Keep this value | the value as typed, a literal: in the field's unit for a scaled amount | always. On a blank field it reads "Leave it blank", and the field is left out |
| Ask each time | an input named after the field (made unique), labelled in words ("mintAuthority" reads "Mint authority") | always |
| The wallet in use | `$wallet` | on an address that is the wallet in use now |
| The fee payer | `$fee-payer` | on an address that is Build's fee payer now |

Some fields are not a choice. An address that is a keypair made on Build is a `new-keypair` value, one value wherever that address appears, so opening the template makes a new key ("A new keypair each time"). A blank field the program needs is asked each time. What the IDL fills in is not written at all: an account it derives that nobody overrode, and an argument left blank for its default. An account the person overrode ("Change") is written, and the template's form asks for it again the same way.

Fields asked that hold the same value and take the same widget are one question, and the dialog says which ("Asked once as “Source”, with 2 · authority"). An amount counted in another argument's units joins another only where that argument is bound the same way. Two questions with one label get their block's number after it.

Each program gets an alias from its IDL's name ("associated_token", "token2022"). The template written reads back from its file to itself and checks against the IDLs it was made from, or nothing is saved and the dialog says why. Opened with the values the blocks held as its answers, the same wallet, fee payer and key, it writes the same blocks, which build to the same bytes: the tests check this for transfers, a scaled token transfer, a mint with a key made here and an overridden account, and the blocks every shipped template shows.

The template's name may not be a shipped one's name or id. One of yours under the same slug asks first: the dialog says "You already have a template called “…”. Saving replaces it." and its main button becomes **Replace**. Saved, the dialog closes, Build's note says so and the blocks stay.

### Where they are kept

A template is kept as the text of its file, one file each, through `app_core::storage` as IDL bodies are. A template saved from the editor, typed, imported or edited, is kept as its text stood, byte for byte, comments and all; one saved from Build's blocks is kept as `Template::to_toml` writes it. `TemplateSource::Yours` holds that text beside the template read from it, so the Text tab, Copy and Export give back what is kept, and what was shown is what is kept: the template is the text read.

| App | Where |
| --- | --- |
| Airlock on desktop, the phone app | `templates/<slug>.toml` in the data folder. On Unix the folder is `0700` and the file `0600`, as every store's. The phone's online and offline modes each have their own data folder |
| Airlock Offline | the same, in its own data folder (`airlock-offline`) |
| A browser | the `localStorage` key `airlock.template.<slug>` |

The slug comes from the name (`template_slug`): lower case letters and digits joined by `-`, at most 48 characters, never empty, never a shipped template's id and never a name Windows keeps for a device (those get `-yours`). Nothing in a name can make it a path. Templates are not per network: `check` decides, per network, whether one can be opened.

A save is checked, as anything only the person can recreate: a write that fails is reported, and the list shows only what is kept. The list is read once, the first time Build is shown: at most 64 files, each at most 64 KiB (`MAX_TEMPLATE_BYTES`). A file that does not parse is listed as broken, with why and its text (`TemplateSource::Broken`), and can be removed; it does not stop the others.

### Import and export

**Export**, on the template picked in the dialog, writes its file as it is kept where the person picks (a save dialog on desktop, a download in a browser), named `<slug>.toml`, or `<id>.toml` for a shipped one. **Import a template**, in the dialog's head, picks one `.toml` file and opens its text in [the editor](#the-editor), not kept yet: nothing is kept before the person has seen what the file does, and a file from somewhere else can write nothing they did not see. The editor reads it as it reads anything typed, so a file that is not a template says why with its line, and can be mended there before it is saved; one that does not fit the network in use can be saved and is listed greyed.

**On a phone** the app has no file picker, so it offers neither, as Programs offers no IDL import there. A template goes between devices as text instead: **Copy** on its Text tab, then paste into **New template** on the other device. New template, Edit and Edit a copy are there as on a desktop, with the reading under the text and Save in the last row.

A debug build started with `AIRLOCK_DEBUG_FILE_DIR` set skips the file dialogs: import reads `import.toml` from that folder into the editor and export writes there, so a script driving the app over CDP can do both. A release build has no such way around them. The dialogs are awaited from a task, never opened inside the click that asks for them: a blocking dialog runs its own message loop on the window's thread, which renders the window while dioxus still holds it for the click, and that render panics. Driving the app through the debug folder does not open a dialog, so it cannot show this; a check of the dialogs needs the real one. An awaited dialog is not modal to the window, so while one is open **Import a template** and **Export** are disabled (`TemplateDesk::dialog`). The task runs in Build's scope, which owns that flag, so closing the Templates dialog while a file dialog is open still lets it finish.

## Adding a shipped template

1. Write `crates/wallet-core/templates/<id>.toml`. The id is the file's stem and stays the same across versions.
2. Add it to `BUILTIN_TEMPLATES` in `crates/wallet-core/src/template/builtin.rs`, in the order the list shows them.
3. Run `cargo test -p wallet-core template`. It parses and checks every shipped template against the embedded IDLs of every network its programs run on, and expands and builds each with every option on.
4. Add a run to `crates/flow-tests/tests/integration/templates.rs`: with no option, with each option alone, and with every option the program allows together. Each run lands the transaction against the real programs in the test VM and reads the result back. An option ships only if its run passes.
5. A Token-2022 extension the size table (`crates/wallet-core/src/template/size.rs`) does not know refuses the template. Add its size there, with a flow test that lands it.
6. Add a line to `CHANGELOG.md`.
