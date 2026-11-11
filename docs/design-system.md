# Design system

One set of tokens and components styles the desktop app, the web app and the extension popup.

| File | Holds |
| --- | --- |
| `crates/app-ui-kit/assets/tokens.css` | Every color, size, radius, shadow and duration, for light and dark |
| `crates/app-ui-kit/assets/main.css` | The app's components, written only against tokens |
| `extension/src/popup.css` | The popup's components. `build.mjs` copies `tokens.css` beside it |
| `crates/app-ui-kit/src/ui/icons.rs` | `Icon { glyph }`, the stroke icon set |
| `crates/app-ui-kit/src/ui/theme.rs`, `src/state/theme.rs` | `ThemeToggle`, and the stored theme choice |
| `docs/design-system.html` | A gallery of every token and component, rendered from the real stylesheets |

Open `docs/design-system.html` in a browser to see the system. Add `#light` or `#dark` to the URL to pin a theme.

## Rules

1. **Components never name a color.** They read a token. A new theme, or a new brand color, is an edit to `tokens.css` alone.
2. **No webfonts or remote assets.** The app runs air-gapped, so type comes from the system font stack.
3. **Addresses, hashes and encoded payloads are mono.** Similar characters must stay distinguishable.
4. **A QR is dark on white in both themes.** A camera has to read it. `--qr-bg` does not change with the theme.
5. **Color is never the only signal.** Every status chip carries a word.

## Themes

The OS setting applies by default. `<html data-theme="light">` or `data-theme="dark"` pins one. `ThemeToggle` sets that attribute and stores the choice in `localStorage` under `airlock.theme`.

The dark values appear twice in `tokens.css`: once for the OS setting and once for the pinned attribute. Change both together.

## Color

| Role | Tokens | Use |
| --- | --- | --- |
| Surfaces | `--bg`, `--surface`, `--surface-2`, `--surface-3` | Page, card, sunken or hover, pressed |
| Lines | `--border`, `--border-control`, `--border-strong` | Dividers, field and button outlines, hover outlines |
| Text | `--text`, `--text-2`, `--text-3` | Primary, secondary, hints and placeholders |
| Brand | `--brand`, `--brand-hover`, `--brand-active`, `--brand-text`, `--brand-soft`, `--brand-line` | Primary action, selection, focus |
| Status | `--ok-*`, `--warn-*`, `--danger-*`, `--info-*` | Each has `-text`, `-soft`, `-line`, `-solid` |

Status colors have fixed meanings:

- **Green**: verified or local. A valid signature, a local signer, an IDL that matches.
- **Amber**: check this. An imported address, an unknown account, a signature still missing.
- **Red**: wrong or flagged. An invalid signature, a red-flagged token, an IDL that differs.
- **Blue**: a neutral result message.

Brand color never carries a status, so "primary action" and "signed" cannot be confused.

Contrast, checked for both themes: body and secondary text at least 7:1, hint text and all status text at least 4.5:1 on their backgrounds, field outlines and status dots at least 3:1.

## Type, space, shape

- Type scale: 11, 12, 13, 14 (body), 16, 18, 22, 28 px. Weights 400, 500, 600.
- Space: a 4px grid, `--space-1` (4px) to `--space-12` (48px).
- Radius: 6 (small controls), 8 (buttons, fields), 12 (cards, rows), 16 (dialogs), full (chips).
- Controls: 28, 36 and 44px high. On touch screens buttons and fields are 44px and fields use 16px text.

## Components

| Component | Classes | Notes |
| --- | --- | --- |
| Button | bare `button`, `.primary`, `.ghost`, `.danger`, `.danger.solid`, `.warn.solid` | A bare button is the secondary. One primary per view: on a page of cards, such as Settings, each card's action is a bare button unless it is the one thing the page is for. A state is never a button that cannot be pressed: it is a chip in the button's place, such as "in use" beside the RPC field. `.ghost` is for Cancel, Clear, Dismiss. `.danger.solid` only confirms a destructive dialog. `.warn.solid` goes ahead with something the app could not check, such as Sign on a request it could not put in words. It sits beside the primary, which stays on the safer way forward. A banner's action, such as Rebuild in Review and Sign, is a bare button; where it is the only way forward, the bar's primary disabled until it is done, it is the `.warn.solid`, never a second brand-filled button. A row's actions in a list are `.sm` and bare; only the one opened row, such as an opened proposal, fills its own primary |
| Button size | `.sm`, `.icon-btn` | An icon-only button needs `title` and `aria_label` |
| Link button | `button.link-btn` | A button inside a line of text that reads as a link, such as Inspect under Summary's instructions |
| Field | `label` wrapping `input`, `select` or `textarea` | `label.check` for a checkbox row |
| Tabs | `.tabs > .tab` | Views of one area |
| Segmented | `.segmented > .tab` | Two or three exclusive options |
| Filter pill | `.tab` in a `.chip-row` | Narrows a list |
| Chip | `.chip` with `.kind`, `.selected`, `.ok`, `.warn`, `.danger`, `.info` | Read-only label. Its text is lower case ("signed", "succeeded", "current", "built-in"), except proper names and acronyms ("Squads v4", "USDC"). `.selected` (`SelectedChip`) is the active one of several, such as the wallet the app acts as ("current") or the RPC in use ("in use"): outlined in the brand's colour, with a check before its word. `.kind` says what a thing is ("Squads v4 · 2 of 3"), filled in the brand's colours, so a filled brand chip is always a kind. The status colours keep their meanings: green is for verified, local or done ("signed", "succeeded"), never for chosen |
| Steps | `.sign-steps > .sign-step` with `.done`, `.current`, `.running`, `.blocked`, `.failed`, `.unanswered`, `.skipped` | Review and Sign. A mark, a name and a word for its state; on a phone only the current step keeps its name. A step that cannot happen is `.skipped` with a word that says why, never "next": an offline app's Share to submit, for a transaction no other device takes, reads "cannot be shared" |
| Network status | `.status` with `.online` (answered), `.offline` (unreachable), `.probing` | An online app: the foot of the sidebar on a desktop (its dot alone in the icon rail); on a phone, above the tab bar and only while something is wrong |
| Offline marker | `OfflineMarker { short }`: `.status.offline-mark` | An app with no network (Airlock Offline, the phone's offline mode), in the network status's place: the sidebar's foot on a desktop, "Offline · never connects"; on a phone the top bar, in the bell's place, "Offline" alone (`short`). A hollow mark, not a status colour: nothing is wrong. Its lock screen says so in one line under the heading. Such an app never offers what needs a network, not even disabled: no Submit, no Check online, no Balance column, and no words that send the person to a button it does not have. Wherever it explains itself it uses one phrase, "this app never connects" ("Not simulated: this app never connects."), never "this device is offline" or "never reads the chain", which the online app keeps for a node it cannot reach |
| Lock | `.lock-toggle` in the top bar | Unlocked, the usual state, a quiet padlock icon. Locked, the state to notice: `.locked`, amber with the word "Locked", the same on a desktop and on a phone. Its tooltip and accessible name say what pressing it does. Every unlock card, the lock screen's and the prompt's, has one quiet line under its title: what this unlock is for, or else `UNLOCK_LINE` ("Unlock the vault to sign with its keys and change its wallets."); in an app with no network the offline line follows it. While locked, a pane that says so uses `LockedNote`, one sentence on a desktop and a phone |
| Message | `p.note`, `p.warn`, `p.error` | Result, caution, failure |
| Empty state | `Empty { glyph, text, compact, children }`: `.empty` | When a list has nothing to show: an icon, one sentence saying what will appear there ("The stake accounts your wallets control show here."), and an optional action as its children. A dashed card with the three centred; `compact` makes it a row no taller than one of the list's entries, for a group among others (the Wallets pane's groups, the notifications, a multisig's proposals). Plain muted text is not an empty state. An empty list that is itself the way to add the first entry, such as Build's instruction tile, stays a button and says so in words beside its plus |
| Card | `.card`, `.card.auth` | Groups one topic on a page |
| Stats | `.stats > .stat` | Transaction summary |
| Row | `.book-row`, `.wallet-row` | List entries. Row actions appear on hover or focus, always on touch |
| Table row | `.ab-table` holding `.ab-columns` and `ul.book-list.ab-list` of `li.ab-row` | The Address Book: one entry a row on shared columns (kind, name and tags, address, copy, balance, menu), under a row of column labels and, with every kind listed, a `li.ab-kind-head` per kind. A row's actions are in a `PaneMenu` at its end, in sight at rest |
| Dialog | `.modal-backdrop > .modal` | A bottom sheet under 640px. Its last row of buttons reads the same on every screen size: what dismisses first (Cancel or Close, `.ghost`), then other secondary actions (Clear), then the one primary last, at the trailing edge. A destructive confirmation is Cancel, then the `.danger.solid`. The order in the markup is the order on screen, so the tab order agrees: never `row-reverse`. A footer with a button at its other end keeps it there; the order applies to the trailing group. A dialog with a button that saves or sends is left by **Cancel**; **Close** is only for one with nothing to keep, such as Details or a share dialog. No line above the last row, on any dialog or sheet. The lock screen's card ends the same way |
| Phone sheet | `PhoneSheet { label, title, on_close }`: `.phone-scrim` and `.phone-sheet`, with a grab handle and `.phone-sheet-title` | The phone's top bar and tab bar menus: the wallet picker, the notifications and More. Over a scrim that closes it. A sheet from the top bar runs to the bottom of the screen, over the tab bar; More stays docked on the tab bar, which stays undimmed |
| Accordion | `Accordion { title, open }` | Optional detail |
| Pane menu | `PaneMenu { label }` holding `button.pane-menu-item` with `role: "menuitem"` | A pane's less used actions, behind a three-line button at the end of its header. It closes on a choice, a press outside or Escape. `glyph: Glyph::More, small: true` makes it the three-dot menu at the end of a row's buttons, as on a wallet card. On a phone a pane's menu and a row's three dots open a bottom sheet instead (`ActionSheet`, in the kit's `ui/phone.rs`): a title, the same rows with icon and label, and Close right under them. A wallet card's sheet is titled with the wallet's name and also lists Details and Settings (`WalletRowMenu`); its three dots, drawn without a box, end the card's name line, and the balance shares its line with the card's buttons (`WalletFoot`) |
| Settings and details of a row | `button.icon-btn.sm` with `Glyph::Gear` and `Glyph::Info`, each opening a `.modal`; on a wallet card, `WalletRowMenu { wallet, dialogs }`, which a phone lists in the card's sheet of actions | What can be changed about a row's subject goes behind the wheel, in a dialog that keeps the choices until **Save** and says "Not saved yet" meanwhile. What can only be read goes behind the "i", in a dialog with **Close** and a button to the settings. The row itself keeps only what identifies its subject and what needs attention. An action that starts a transaction acts at once, wherever it is |
| Copy | `CopyButton { text }` | An icon. Its tooltip and accessible name say "Copy to clipboard"; it shows a check for a moment after copying |

## Addresses

Every address goes through `ui/address` in `app-ui-kit`. What to call it comes from `wallet_core::Directory`: a wallet's name wins over the address book's, and nothing makes a name up from the address itself. A test fails if `short_pubkey` is used anywhere else in the `app-ui*` crates.

| Category | Icon | Token |
| --- | --- | --- |
| Wallet · hot | flame | `--cat-hot` |
| Wallet · cold | snowflake | `--cat-wallet` |
| Wallet · hardware | device | `--cat-wallet` |
| Wallet · smart contract (Squads vault and multisig, programmatic signer, Vector account) | safe | `--cat-wallet` |
| Nonce | hourglass | `--cat-nonce` |
| Program | `</>` | `--cat-program` |
| Validator | server | `--cat-validator` |
| Token (mint or token account) | coin | `--cat-token` |
| Contact | person | `--cat-contact` |
| Unknown | circled `?` | amber |

| Density | Component | Shows | Where |
| --- | --- | --- | --- |
| Inline | `Address { pubkey }` | icon · label · short | Sentences and chip rows: Sign, the transaction view and its intent sentences, the titles of History's rows (no taller than their line), Proposals, browser requests, fact rows, stake authorities |
| Row | `AddressRow { pubkey }`, `AddressSelect` | icon · label · short · copy, then what the list adds | Pickers, checklists, lists. `full: true` shows the whole address: Inspect, and on Sign the accounts of an instruction no sentence explains (no IDL, or an IDL without a display form). `no_icon: true` leaves the kind icon out where every row has the same one (the Programs list); an address that needs care keeps it. `no_label: true` leaves the label out where the list shows the name in a column of its own |
| Copy capsule | `CopyAddress { pubkey }` | short · copy, one button | Where the address is the thing to press: an Address Book entry and a token account on a phone. It copies the whole address and shows a check for a moment |
| Full | `AddressHeader { pubkey }` | badge · label · what it is · whole address · copy | Wallet rows, address book entries, Programs detail, stake accounts. `no_address: true` leaves the address to the caller, as the Programs detail does to draw it under its pinned name. `short: true` shortens the address to one line: a wallet card on a phone |

- Without a label, an address shows its icon and short form only.
- Amber means nothing names the address, or its name is also another address's. A shared name adds an alert glyph, so colour is never the only sign.
- Hover, focus or a click on an inline or row address opens `AddressCard`: the whole address with copy, what it is, and what is known (balance, owner, multisig, fee payer, source). An unknown address offers to add it to the book.
- A note from the core that names addresses is a `wallet_core::Phrase`, drawn with `PhraseView`.

## Layout

`.app` is a sidebar and a main column. A page is `section.page` with a `.page-head` (title, lede, actions) and then cards or lists.

A page's head is its title, with a lede under it if it has one, then its actions on one row at the trailing edge, in one order: what filters the page (a select), then secondary actions (Refresh, always named so), then the one primary, then the pane's menu. On a phone every pane's head has one layout: the title on its own line, and under it one row in the same order, nothing stacked and nothing on the title's line. The filter takes the room that is left; with no filter the primary does (Add wallet, New stake), with the menu at the end. Every Refresh in the apps, a pane's or a dialog's, is `RefreshButton`: the refresh glyph, then the word on a desktop ("↻ Refresh"); on a phone the glyph alone, named "Refresh" for assistive tech and in its tooltip, except in a dialog's last row of buttons (`labelled`). The glyph turns while it runs, so the word never appears without it. On a phone it sits in that row on every pane that can refresh and never also in its menu; the pane says "Refreshing…" under the head while it runs.

| Width | Navigation |
| --- | --- |
| 960px and up | 232px sidebar with labels |
| 640 to 959px | 76px icon rail |
| under 640px | Bar along the bottom edge, sticky top bar |

A section has one name, the sidebar's, in buttons and running text alike: a button that takes a transaction to Review and Sign says "Review and Sign", and a hint says "in Review and Sign", never "in Sign" or "on Review". A tab's short label in a phone's tab bar ("Sign", "Contacts") is the only place a name is shortened. A phone's More sheet lists its sections in the sidebar's order.
