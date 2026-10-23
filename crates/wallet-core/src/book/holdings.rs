use crate::chain::DiscoveredTokenAccount;
use crate::error::VaultError;
use crate::ids::pubkey_string;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;
use wallet_vault::store::{check_saveable, load_json, LoadProblem, VaultStore};

/// A mark on a token account. Stored with the holding so the list can be filtered later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenFlag {
    White,
    Red,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenHolding {
    #[serde(with = "pubkey_string")]
    pub pubkey: Pubkey,
    #[serde(with = "pubkey_string")]
    pub mint: Pubkey,
    pub amount: u64,
    pub decimals: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<TokenFlag>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HoldingSummary {
    pub total: usize,
    pub red: usize,
    pub white: usize,
}

/// Token accounts keyed by owner. This is a cache, not part of the address book.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holdings {
    owners: BTreeMap<String, Vec<TokenHolding>>,
    /// Prices, asset groups and checks for the mints above.
    #[serde(default, skip_serializing_if = "is_empty_market")]
    pub market: crate::market::MarketCache,
}

fn is_empty_market(market: &crate::market::MarketCache) -> bool {
    *market == crate::market::MarketCache::default()
}

impl Holdings {
    pub fn summary(&self, owner: &Pubkey) -> HoldingSummary {
        let Some(list) = self.owners.get(&owner.to_string()) else {
            return HoldingSummary::default();
        };
        HoldingSummary {
            total: list.len(),
            red: list
                .iter()
                .filter(|h| h.flag == Some(TokenFlag::Red))
                .count(),
            white: list
                .iter()
                .filter(|h| h.flag == Some(TokenFlag::White))
                .count(),
        }
    }

    pub fn page(&self, owner: &Pubkey, offset: usize, limit: usize) -> Vec<TokenHolding> {
        self.owners
            .get(&owner.to_string())
            .map(|list| list.iter().skip(offset).take(limit).cloned().collect())
            .unwrap_or_default()
    }

    pub fn flag_of(&self, account: &Pubkey) -> Option<TokenFlag> {
        self.owners.values().find_map(|list| {
            list.iter()
                .find(|holding| holding.pubkey == *account)
                .and_then(|holding| holding.flag)
        })
    }

    /// A token account and the owner it is listed under.
    pub fn find(&self, token_account: &Pubkey) -> Option<(Pubkey, &TokenHolding)> {
        self.owners.iter().find_map(|(owner, list)| {
            let holding = list
                .iter()
                .find(|holding| holding.pubkey == *token_account)?;
            Some((owner.parse().ok()?, holding))
        })
    }

    pub fn decimals_of(&self, token_account: &Pubkey) -> Option<u8> {
        self.owners.values().find_map(|list| {
            list.iter()
                .find(|holding| holding.pubkey == *token_account)
                .map(|holding| holding.decimals)
        })
    }

    /// Match a mint or token-account address. An empty query matches nothing,
    /// so the picker does not list every holding.
    pub fn search(&self, query: &str, limit: usize) -> Vec<TokenHolding> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for list in self.owners.values() {
            for holding in list {
                let mint = holding.mint.to_string().to_ascii_lowercase();
                let account = holding.pubkey.to_string().to_ascii_lowercase();
                if !mint.contains(&needle) && !account.contains(&needle) {
                    continue;
                }
                out.push(holding.clone());
                if out.len() == limit {
                    return out;
                }
            }
        }
        out
    }

    /// Every mint held by `owners` with a balance, once each.
    pub fn mints_of(&self, owners: &[Pubkey]) -> Vec<Pubkey> {
        let mut mints: Vec<Pubkey> = Vec::new();
        for owner in owners {
            for held in self.owners.get(&owner.to_string()).into_iter().flatten() {
                if held.amount > 0 && !mints.contains(&held.mint) {
                    mints.push(held.mint);
                }
            }
        }
        mints
    }

    /// Replace each refreshed owner's token accounts. Flags already stored for
    /// the same token account are kept. Zero balances are dropped unless flagged.
    /// Pass only owners whose token list was actually fetched: an owner listed
    /// here with no tokens loses its holdings and their flags.
    pub fn replace_owners(&mut self, owners: &[Pubkey], tokens: &[DiscoveredTokenAccount]) {
        for owner in owners {
            let found: Vec<&DiscoveredTokenAccount> = tokens
                .iter()
                .filter(|token| token.owner == *owner)
                .collect();
            self.replace_one(*owner, &found);
        }
    }

    pub fn set_flag(&mut self, owner: &Pubkey, token: &Pubkey, flag: Option<TokenFlag>) -> bool {
        let Some(list) = self.owners.get_mut(&owner.to_string()) else {
            return false;
        };
        let Some(holding) = list.iter_mut().find(|holding| holding.pubkey == *token) else {
            return false;
        };
        holding.flag = flag;
        list.sort_by(holding_order);
        true
    }

    fn replace_one(&mut self, owner: Pubkey, tokens: &[&DiscoveredTokenAccount]) {
        let key = owner.to_string();
        let mut previous = BTreeMap::new();
        if let Some(list) = self.owners.get(&key) {
            for holding in list {
                if let Some(flag) = holding.flag {
                    previous.insert(holding.pubkey, flag);
                }
            }
        }
        let mut next = Vec::new();
        for token in tokens {
            let flag = previous.get(&token.pubkey).copied();
            if token.amount == 0 && flag.is_none() {
                continue;
            }
            next.push(TokenHolding {
                pubkey: token.pubkey,
                mint: token.mint,
                amount: token.amount,
                decimals: token.decimals,
                flag,
            });
        }
        next.sort_by(holding_order);
        if next.is_empty() {
            self.owners.remove(&key);
        } else {
            self.owners.insert(key, next);
        }
    }
}

fn holding_order(a: &TokenHolding, b: &TokenHolding) -> std::cmp::Ordering {
    flag_rank(a.flag)
        .cmp(&flag_rank(b.flag))
        .then(b.amount.cmp(&a.amount))
        .then(a.mint.cmp(&b.mint))
}

fn flag_rank(flag: Option<TokenFlag>) -> u8 {
    match flag {
        Some(TokenFlag::Red) => 0,
        Some(TokenFlag::White) => 1,
        None => 2,
    }
}

pub struct HoldingsStore<S: VaultStore> {
    store: S,
    data: Holdings,
    problem: Option<LoadProblem>,
}

impl<S: VaultStore> HoldingsStore<S> {
    pub fn open(store: S) -> Self {
        let (data, problem) = load_json(&store, "token holdings");
        Self {
            store,
            data,
            problem,
        }
    }

    /// Set when the stored holdings could not be read at startup.
    pub fn load_problem(&self) -> Option<&LoadProblem> {
        self.problem.as_ref()
    }

    pub fn data(&self) -> &Holdings {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Holdings {
        &mut self.data
    }

    pub fn save(&self) -> Result<(), VaultError> {
        check_saveable(&self.problem)?;
        let json =
            serde_json::to_vec_pretty(&self.data).map_err(|e| VaultError::Serde(e.to_string()))?;
        self.store.save(&json)
    }
    /// Change the data and save it. When the save does not happen the data
    /// in memory is what it was before.
    pub fn change(&mut self, f: impl FnOnce(&mut Holdings)) -> Result<(), VaultError> {
        let previous = self.data.clone();
        f(&mut self.data);
        let saved = self.save();
        if saved.is_err() {
            self.data = previous;
        }
        saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_vault::store::MemoryStore;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn token(account: u8, owner: u8, mint: u8, amount: u64) -> DiscoveredTokenAccount {
        DiscoveredTokenAccount {
            pubkey: pk(account),
            owner: pk(owner),
            mint: pk(mint),
            amount,
            decimals: 6,
        }
    }

    #[test]
    fn a_change_that_cannot_be_saved_is_not_kept_in_memory_either() {
        let store = std::sync::Arc::new(MemoryStore::new());
        let mut holdings = HoldingsStore::open(store.clone());
        let owner = pk(2);
        holdings
            .change(|data| data.replace_owners(&[owner], &[token(3, 2, 4, 10)]))
            .unwrap();

        store.refuse_saves(true);
        let refused = holdings.change(|data| data.replace_owners(&[owner], &[]));
        assert!(refused.is_err());
        assert_eq!(holdings.data().summary(&owner).total, 1);
    }

    #[test]
    fn refresh_keeps_flags_and_drops_empty_balances() {
        let mut holdings = Holdings::default();
        let owner = pk(2);
        holdings.replace_owners(
            &[owner],
            &[token(3, 2, 4, 10), token(5, 2, 6, 0), token(7, 2, 8, 2)],
        );
        assert_eq!(holdings.summary(&owner).total, 2);
        assert!(holdings.set_flag(&owner, &pk(7), Some(TokenFlag::Red)));
        assert!(holdings.set_flag(&owner, &pk(3), Some(TokenFlag::White)));

        holdings.replace_owners(&[owner], &[token(3, 2, 4, 11), token(5, 2, 6, 0)]);
        let page = holdings.page(&owner, 0, 10);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].pubkey, pk(3));
        assert_eq!(page[0].amount, 11);
        assert_eq!(page[0].flag, Some(TokenFlag::White));
        assert_eq!(holdings.summary(&owner).red, 0);

        holdings.set_flag(&owner, &pk(3), Some(TokenFlag::Red));
        holdings.replace_owners(&[owner], &[token(3, 2, 4, 0), token(9, 2, 8, 4)]);
        let page = holdings.page(&owner, 0, 10);
        assert_eq!(page[0].flag, Some(TokenFlag::Red));
        assert_eq!(page[0].amount, 0);
        assert_eq!(page[1].pubkey, pk(9));
        assert!(page[1].flag.is_none());
    }

    #[test]
    fn holdings_json_uses_pubkey_strings() {
        let mut store = HoldingsStore::open(MemoryStore::new());
        store
            .data_mut()
            .replace_owners(&[pk(2)], &[token(3, 2, 4, 10)]);
        store
            .data_mut()
            .set_flag(&pk(2), &pk(3), Some(TokenFlag::Red));
        let json = serde_json::to_string_pretty(store.data()).unwrap();
        assert!(json.contains(&format!("\"{}\"", pk(3))));
        assert!(json.contains(&format!("\"{}\"", pk(4))));
        assert!(json.contains("\"red\""));
        assert!(!json.contains("[3,3,3,3"));
        let loaded: Holdings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.page(&pk(2), 0, 1)[0].flag, Some(TokenFlag::Red));
    }
}
