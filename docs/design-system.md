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
| Button | bare `button`, `.primary`, `.ghost`, `.danger`, `.danger.solid`, `.warn.solid` | A bare button is the secondary. One primary per view. `.ghost` is for Cancel, Clear, Dismiss. `.danger.solid` only confirms a destructive dialog. `.warn.solid` goes ahead with something the app could not check, such as Sign on a request it could not put in words. It sits beside the primary, which stays on the safer way forward |
| Button size | `.sm`, `.icon-btn` | An icon-only button needs `title` and `aria_label` |
| Field | `label` wrapping `input`, `select` or `textarea` | `label.check` for a checkbox row |
| Tabs | `.tabs > .tab` | Views of one area |
| Segmented | `.segmented > .tab` | Two or three exclusive options |
| Filter pill | `.tab` in a `.chip-row` | Narrows a list |
| Chip | `.chip` with `.kind`, `.ok`, `.warn`, `.danger`, `.info` | Read-only label |
| Steps | `.sign-steps > .sign-step` with `.done`, `.current`, `.running`, `.blocked`, `.failed`, `.unanswered`, `.skipped` | Review and Sign. A mark, a name and a word for its state; on a phone only the current step keeps its name |
| Network status | `.status` with `.online` (answered), `.offline` (unreachable), `.probing` | Top bar, online build |
| Message | `p.note`, `p.warn`, `p.error` | Result, caution, failure |
| Empty state | `.empty` | When a list has nothing to show |
| Card | `.card`, `.card.auth` | Groups one topic on a page |
| Stats | `.stats > .stat` | Transaction summary |
| Row | `.book-row`, `.wallet-row` | List entries. Row actions appear on hover or focus, always on touch |
| Dialog | `.modal-backdrop > .modal` | A bottom sheet under 640px |
| Accordion | `Accordion { title, open }` | Optional detail |
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
| Inline | `Address { pubkey }` | icon · label · short | Sentences and chip rows: Sign, the transaction view and its intent sentences, Proposals, browser requests, fact rows, stake authorities |
| Row | `AddressRow { pubkey }`, `AddressSelect` | icon · label · short · copy, then what the list adds | Pickers, checklists, lists. `full: true` shows the whole address: Inspect, and on Sign the accounts of an instruction no sentence explains (no IDL, or an IDL without a display form) |
| Full | `AddressHeader { pubkey }` | badge · label · what it is · whole address · copy | Wallet rows, address book entries, Programs detail, stake accounts |

- Without a label, an address shows its icon and short form only.
- Amber means nothing names the address, or its name is also another address's. A shared name adds an alert glyph, so colour is never the only sign.
- Hover, focus or a click on an inline or row address opens `AddressCard`: the whole address with copy, what it is, and what is known (balance, owner, multisig, fee payer, source). An unknown address offers to add it to the book.
- A note from the core that names addresses is a `wallet_core::Phrase`, drawn with `PhraseView`.

## Layout

`.app` is a sidebar and a main column. A page is `section.page` with a `.page-head` (title, lede, actions) and then cards or lists.

| Width | Navigation |
| --- | --- |
| 960px and up | 232px sidebar with labels |
| 640 to 959px | 76px icon rail |
| under 640px | Bar along the bottom edge, sticky top bar |
