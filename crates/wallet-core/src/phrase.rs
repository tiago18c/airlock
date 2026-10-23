//! A sentence for a screen that names addresses. The screen draws each
//! address the way it draws every other one, with the name it has when the
//! sentence is shown.

use crate::ids::short_pubkey;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Part {
    Text(String),
    Address(#[serde(with = "crate::ids::pubkey_string")] Pubkey),
}

/// Text and addresses, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phrase(pub Vec<Part>);

impl Phrase {
    pub fn text(text: impl Into<String>) -> Self {
        Self(vec![Part::Text(text.into())])
    }

    pub fn push_text(&mut self, text: impl Into<String>) -> &mut Self {
        let text = text.into();
        match self.0.last_mut() {
            Some(Part::Text(last)) => last.push_str(&text),
            _ => self.0.push(Part::Text(text)),
        }
        self
    }

    pub fn push_address(&mut self, pubkey: Pubkey) -> &mut Self {
        self.0.push(Part::Address(pubkey));
        self
    }

    /// Chain: `Phrase::default().t("Sends to ").a(pk).t(".")`.
    pub fn t(mut self, text: impl Into<String>) -> Self {
        self.push_text(text);
        self
    }

    pub fn a(mut self, pubkey: Pubkey) -> Self {
        self.push_address(pubkey);
        self
    }

    /// Append another phrase.
    pub fn and(mut self, other: Phrase) -> Self {
        for part in other.0 {
            match part {
                Part::Text(text) => {
                    self.push_text(text);
                }
                Part::Address(pubkey) => {
                    self.push_address(pubkey);
                }
            }
        }
        self
    }

    /// For logs, tests and anywhere without a screen: each address short.
    pub fn plain(&self) -> String {
        self.0
            .iter()
            .map(|part| match part {
                Part::Text(text) => text.clone(),
                Part::Address(pubkey) => short_pubkey(pubkey),
            })
            .collect()
    }
}

impl From<String> for Phrase {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

impl From<&str> for Phrase {
    fn from(text: &str) -> Self {
        Self::text(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_runs_join_and_addresses_read_short() {
        let pk = Pubkey::new_from_array([3; 32]);
        let phrase = Phrase::default().t("Sends ").t("to ").a(pk).t(".");
        assert_eq!(phrase.0.len(), 3);
        assert_eq!(phrase.plain(), format!("Sends to {}.", short_pubkey(&pk)));
    }
}
