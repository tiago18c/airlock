//! Finding an address by what was typed: a label, a tag, or the address itself.

use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::collections::BTreeSet;
use std::str::FromStr;
use wallet_core::{AddressEntry, AddressPlace, BookRow, TokenFlag, TokenHolding};

#[derive(Clone)]
pub struct PickerChoice {
    pub pubkey: Pubkey,
    pub group_id: String,
    pub label: String,
    pub group_label: Option<String>,
    pub flag: Option<TokenFlag>,
}

pub fn picker_choices(
    query: &str,
    book: &wallet_core::AddressBook,
    holdings: &[TokenHolding],
    flag_of: impl Fn(&Pubkey) -> Option<TokenFlag>,
) -> Vec<PickerChoice> {
    let raw = query.trim();
    let mut choices = Vec::new();
    if let Ok(pk) = Pubkey::from_str(raw) {
        choices.push(choice_for_pubkey(pk, book.place(&pk), flag_of(&pk)));
    }
    for row in book.rows(raw, None) {
        if choices
            .iter()
            .any(|choice| choice.pubkey == row.entry.pubkey && choice.group_id == row.group_id)
        {
            continue;
        }
        choices.push(choice_from_row(&row, flag_of(&row.entry.pubkey)));
        if choices.len() == 12 {
            return choices;
        }
    }
    append_holding_choices(&mut choices, holdings, book);
    choices.truncate(12);
    choices
}

pub fn choice_for_pubkey(
    pubkey: Pubkey,
    place: Option<AddressPlace>,
    flag: Option<TokenFlag>,
) -> PickerChoice {
    match place {
        Some(place) => PickerChoice {
            pubkey,
            group_id: place.group_id,
            label: shown_label(&place.entry),
            group_label: place.group_label,
            flag,
        },
        None => PickerChoice {
            pubkey,
            group_id: String::new(),
            label: pubkey.to_string(),
            group_label: None,
            flag,
        },
    }
}

pub fn choice_from_row(row: &BookRow, flag: Option<TokenFlag>) -> PickerChoice {
    PickerChoice {
        pubkey: row.entry.pubkey,
        group_id: row.group_id.clone(),
        label: shown_label(&row.entry),
        group_label: if row.imported {
            Some(row.group_label.clone())
        } else {
            None
        },
        flag,
    }
}

pub fn shown_label(entry: &AddressEntry) -> String {
    if entry.label.trim().is_empty() {
        entry.pubkey.to_string()
    } else {
        entry.label.clone()
    }
}

pub fn append_holding_choices(
    choices: &mut Vec<PickerChoice>,
    holdings: &[TokenHolding],
    book: &wallet_core::AddressBook,
) {
    for holding in holdings {
        if choices.iter().any(|choice| choice.pubkey == holding.pubkey) {
            continue;
        }
        if book.get(&holding.pubkey).is_some() {
            continue;
        }
        let title = book
            .get(&holding.mint)
            .map(|entry| entry.label.clone())
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| holding.pubkey.to_string());
        choices.push(PickerChoice {
            pubkey: holding.pubkey,
            group_id: format!("holding:{}", holding.pubkey),
            label: title,
            group_label: None,
            flag: holding.flag,
        });
    }
}

#[allow(clippy::result_unit_err)]
pub fn resolve_address(raw: &str, book: &wallet_core::AddressBook) -> Result<Option<Pubkey>, ()> {
    if raw.is_empty() {
        return Ok(None);
    }
    if let Ok(pk) = Pubkey::from_str(raw) {
        return Ok(Some(pk));
    }
    if let Some(entry) = book
        .iter()
        .find(|entry| entry.label.eq_ignore_ascii_case(raw))
    {
        return Ok(Some(entry.pubkey));
    }
    Err(())
}

pub fn parse_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect()
}

pub fn join_tags(tags: &BTreeSet<String>) -> String {
    tags.iter().cloned().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_core::{AddressBook, DiscoveredTokenAccount, Holdings, Origin};

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn book() -> AddressBook {
        let mut book = AddressBook::default();
        book.insert_account(pk(1), "Treasury", Origin::Imported, None);
        book.insert_contact(pk(2), "Alice");
        book.insert_contact(pk(3), "");
        book
    }

    fn holding(account: u8, owner: u8, mint: u8) -> TokenHolding {
        let mut holdings = Holdings::default();
        holdings.replace_owners(
            &[pk(owner)],
            &[DiscoveredTokenAccount {
                pubkey: pk(account),
                owner: pk(owner),
                mint: pk(mint),
                amount: 5,
                decimals: 6,
            }],
        );
        holdings.page(&pk(owner), 0, 1).remove(0)
    }

    fn labels(choices: &[PickerChoice]) -> Vec<&str> {
        choices.iter().map(|choice| choice.label.as_str()).collect()
    }

    #[test]
    fn a_label_finds_its_entry_and_says_what_kind_of_address_it_is() {
        let found = picker_choices("ali", &book(), &[], |_| None);
        assert_eq!(labels(&found), ["Alice"]);
        assert_eq!(found[0].pubkey, pk(2));
        assert_eq!(found[0].group_label, None);
    }

    #[test]
    fn a_typed_address_comes_first_whether_or_not_the_book_knows_it() {
        let known = picker_choices(&pk(1).to_string(), &book(), &[], |_| None);
        assert_eq!(labels(&known), ["Treasury"]);

        let stranger = pk(9).to_string();
        let unknown = picker_choices(&stranger, &book(), &[], |_| None);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].label, stranger);
    }

    #[test]
    fn an_entry_without_a_label_is_shown_by_its_address() {
        let found = picker_choices(&pk(3).to_string(), &book(), &[], |_| None);
        assert_eq!(found[0].label, pk(3).to_string());
    }

    #[test]
    fn token_accounts_the_book_does_not_name_are_offered_under_their_mint() {
        let mut book = book();
        book.insert_contact(pk(20), "USDC");
        let named_mint = holding(10, 1, 20);
        let unnamed_mint = holding(11, 1, 21);
        // A token account the book already names is not offered twice.
        book.insert_contact(pk(12), "Savings");
        let in_book = holding(12, 1, 20);

        let found = picker_choices(
            "zzz no entry matches",
            &book,
            &[named_mint, unnamed_mint, in_book],
            |_| None,
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].label, "USDC");
        // Without a name, the account is matched by its address.
        assert_eq!(found[1].label, pk(11).to_string());
    }

    #[test]
    fn no_more_than_twelve_choices_are_offered() {
        let mut book = AddressBook::default();
        for n in 1..=20 {
            book.insert_contact(pk(n), format!("Friend {n}"));
        }
        assert_eq!(picker_choices("friend", &book, &[], |_| None).len(), 12);
    }

    #[test]
    fn a_choice_carries_the_flag_of_its_address() {
        let found = picker_choices("ali", &book(), &[], |address| {
            (*address == pk(2)).then_some(TokenFlag::Red)
        });
        assert_eq!(found[0].flag, Some(TokenFlag::Red));
    }

    #[test]
    fn what_was_typed_resolves_to_an_address_a_label_or_nothing() {
        let book = book();
        assert_eq!(resolve_address("", &book), Ok(None));
        assert_eq!(resolve_address(&pk(9).to_string(), &book), Ok(Some(pk(9))));
        assert_eq!(resolve_address("alice", &book), Ok(Some(pk(2))));
        // A label has to be typed whole.
        assert_eq!(resolve_address("ali", &book), Err(()));
    }

    #[test]
    fn tags_are_split_on_commas_and_joined_back_in_order() {
        let tags = parse_tags(" ops, ,cold ,ops");
        assert_eq!(tags, ["ops", "cold", "ops"]);
        let set: BTreeSet<String> = tags.into_iter().collect();
        assert_eq!(join_tags(&set), "cold, ops");
        assert!(parse_tags(" , ").is_empty());
    }
}
