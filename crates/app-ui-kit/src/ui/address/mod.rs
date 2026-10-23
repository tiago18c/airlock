//! Every address the app shows, in one of three densities, with a card that
//! says what is known about it. What to call an address comes from
//! [`wallet_core::Directory`]: a wallet's name wins over the book's, and an
//! address nothing names shows only its short form.
//!
//! - [`Address`]: inline, in a sentence or a row of chips.
//! - [`AddressRow`]: a line of a list or a picker.
//! - [`AddressHeader`]: the head of a detail, with the whole address.

use crate::state::session::{Session, Tab};
use crate::ui::{format_sol, format_token_amount, CopyButton, Glyph, Icon};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::{short_pubkey, Category, Described, Fact, Origin, TokenFlag, WalletSort};

mod card;
mod header;
mod inline;
pub use inline::use_card_beside;
mod phrase;
mod row;
mod select;

pub use card::AddressCard;
pub use header::AddressHeader;
pub use inline::Address;
pub use phrase::PhraseView;
pub use row::AddressRow;

pub use select::AddressSelect;

/// The SOL each account would gain or lose, as the last simulation said,
/// before and after. A view that provides it gets the change drawn beside
/// each address in it.
#[derive(Clone, Copy)]
pub struct BalanceDeltas(pub Memo<std::collections::HashMap<Pubkey, (u64, u64)>>);

/// The change [`BalanceDeltas`] knows for `pubkey`, if any is provided.
#[component]
fn BalanceDelta(pubkey: Pubkey) -> Element {
    let deltas = try_use_context::<BalanceDeltas>();
    let Some((before, after)) = deltas.and_then(|deltas| deltas.0.read().get(&pubkey).copied())
    else {
        return rsx! {};
    };
    let tip = format!("{} → {}", format_sol(before), format_sol(after));
    if after >= before {
        rsx! { span { class: "delta up", title: "{tip}", "+{format_sol(after - before)}" } }
    } else {
        rsx! { span { class: "delta down", title: "{tip}", "−{format_sol(before - after)}" } }
    }
}

/// The icon of a category.
pub fn category_glyph(category: Category) -> Glyph {
    match category {
        Category::Wallet(WalletSort::Hot) => Glyph::Flame,
        Category::Wallet(WalletSort::Cold) => Glyph::Snowflake,
        Category::Wallet(WalletSort::Hardware) => Glyph::Device,
        Category::Wallet(WalletSort::SmartContract) => Glyph::Safe,
        Category::Nonce => Glyph::Hourglass,
        Category::Program => Glyph::Code,
        Category::Validator => Glyph::Server,
        Category::Token => Glyph::Coin,
        Category::Contact => Glyph::Person,
        Category::Unknown => Glyph::Unknown,
    }
}

/// The class that colours a category's icon.
pub fn category_class(category: Category) -> &'static str {
    match category {
        Category::Wallet(WalletSort::Hot) => "cat cat-hot",
        Category::Wallet(_) => "cat cat-wallet",
        Category::Nonce => "cat cat-nonce",
        Category::Program => "cat cat-program",
        Category::Validator => "cat cat-validator",
        Category::Token => "cat cat-token",
        Category::Contact => "cat cat-contact",
        Category::Unknown => "cat cat-unknown",
    }
}

/// The category's icon, coloured.
#[component]
pub fn CategoryIcon(category: Category) -> Element {
    rsx! {
        span { class: category_class(category), title: category.name(),
            Icon { glyph: category_glyph(category) }
        }
    }
}

/// What is known about `pubkey`, following the wallets, the book and the
/// holdings as they change.
pub fn use_described(pubkey: Pubkey) -> Described {
    let session = use_context::<Session>();
    use_memo(use_reactive!(
        |(pubkey)| (session.directory)().describe(&pubkey)
    ))()
}

/// The name of an address for running text: its label, else its short form.
pub fn name_or_short(session: &Session, pubkey: &Pubkey) -> String {
    session
        .label_of(pubkey)
        .unwrap_or_else(|| short_pubkey(pubkey))
}

/// An address is shown in amber when nothing names it, or when its name is
/// also another address's.
pub fn needs_care(described: &Described) -> bool {
    described.category == Category::Unknown || !described.collides_with.is_empty()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every screen draws an address through this module, so a short form
    /// made anywhere else is a screen that went its own way. The screens live
    /// in the other `app-ui*` crates, beside this one.
    #[test]
    fn only_this_module_shortens_an_address() {
        let kit = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(kit.parent().unwrap()).unwrap() {
            let crate_dir = entry.unwrap().path();
            if !crate_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("app-ui")
            {
                continue;
            }
            visit(&crate_dir.join("src").join("ui"), &mut |path, text| {
                let inside = path.components().any(|part| part.as_os_str() == "address");
                if !inside && text.contains("short_pubkey") {
                    offenders.push(path.display().to_string());
                }
            });
        }
        assert!(offenders.is_empty(), "short_pubkey used in {offenders:?}");
    }

    fn visit(dir: &Path, found: &mut impl FnMut(&Path, &str)) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found(&path, &std::fs::read_to_string(&path).unwrap());
            }
        }
    }
}
