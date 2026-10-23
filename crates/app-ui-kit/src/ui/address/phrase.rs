//! A note from the core, with its addresses drawn as addresses.

use super::*;
use wallet_core::{Part, Phrase};

/// Text as text, each address as [`Address`]. Resolved when drawn, so a
/// note shows the names of now.
#[component]
pub fn PhraseView(phrase: Phrase) -> Element {
    rsx! {
        for (i, part) in phrase.0.into_iter().enumerate() {
            match part {
                Part::Text(text) => rsx! { span { key: "{i}", "{text}" } },
                Part::Address(pubkey) => rsx! { Address { key: "{i}", pubkey } },
            }
        }
    }
}
