//! What a transaction that landed did to balances, from what the node kept
//! of it.

use crate::chain::LandedTx;
use crate::tx::decode::parse_wire;
use solana_pubkey::Pubkey;

/// What a landed transaction did to one address's balances.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LandedChange {
    /// Lamports it gained, or lost when negative, fees included.
    pub lamports: i128,
    /// Each token it gained or lost, in raw units.
    pub tokens: Vec<TokenDelta>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenDelta {
    pub mint: Pubkey,
    pub decimals: u8,
    pub amount: i128,
}

impl LandedChange {
    pub fn is_empty(&self) -> bool {
        self.lamports == 0 && self.tokens.is_empty()
    }
}

/// Every account of a landed transaction, in the order the node lists their
/// balances: the message's own, then what its lookup tables loaded.
pub fn landed_accounts(landed: &LandedTx) -> Vec<Pubkey> {
    let Ok(view) = parse_wire(&landed.wire) else {
        return Vec::new();
    };
    let mut keys = view.static_keys;
    keys.extend(&landed.loaded_writable);
    keys.extend(&landed.loaded_readonly);
    keys
}

/// Each account whose lamports changed, with what it held before and after.
pub fn landed_balances(landed: &LandedTx) -> Vec<(Pubkey, u64, u64)> {
    landed_accounts(landed)
        .into_iter()
        .zip(landed.pre_balances.iter().zip(&landed.post_balances))
        .filter(|(_, (pre, post))| pre != post)
        .map(|(pubkey, (pre, post))| (pubkey, *pre, *post))
        .collect()
}

/// What `owner` gained or lost: lamports at its own address, and tokens in
/// the token accounts it owns.
pub fn change_for(landed: &LandedTx, owner: &Pubkey) -> LandedChange {
    let lamports = landed_balances(landed)
        .iter()
        .filter(|(pubkey, ..)| pubkey == owner)
        .map(|(_, pre, post)| i128::from(*post) - i128::from(*pre))
        .sum();
    let mut tokens: Vec<TokenDelta> = Vec::new();
    for balance in &landed.token_balances {
        if balance.owner.as_ref() != Some(owner) {
            continue;
        }
        let amount = i128::from(balance.post) - i128::from(balance.pre);
        match tokens.iter_mut().find(|known| known.mint == balance.mint) {
            Some(known) => known.amount += amount,
            None => tokens.push(TokenDelta {
                mint: balance.mint,
                decimals: balance.decimals,
                amount,
            }),
        }
    }
    tokens.retain(|delta| delta.amount != 0);
    LandedChange { lamports, tokens }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::TokenBalanceChange;
    use crate::fixtures::{compile, pk, transfer};

    #[test]
    fn what_one_address_gained_and_lost() {
        let (from, to, mint) = (pk(1), pk(7), pk(8));
        let wire = compile(from, vec![transfer(from, to, 1_000)]).wire;
        let mut landed = LandedTx {
            slot: 1,
            block_time: None,
            wire,
            err: None,
            fee: 5_000,
            loaded_writable: Vec::new(),
            loaded_readonly: Vec::new(),
            pre_balances: Vec::new(),
            post_balances: Vec::new(),
            token_balances: vec![
                TokenBalanceChange {
                    account_index: 9,
                    mint,
                    owner: Some(to),
                    decimals: 6,
                    pre: 0,
                    post: 2_500_000,
                },
                TokenBalanceChange {
                    account_index: 10,
                    mint,
                    owner: Some(from),
                    decimals: 6,
                    pre: 9,
                    post: 9,
                },
            ],
            logs: Vec::new(),
        };
        let keys = landed_accounts(&landed);
        assert_eq!(keys[0], from);
        assert!(keys.contains(&to));
        for key in &keys {
            let (pre, post) = match key {
                key if *key == from => (10_000, 4_000),
                key if *key == to => (0, 1_000),
                _ => (1, 1),
            };
            landed.pre_balances.push(pre);
            landed.post_balances.push(post);
        }
        assert_eq!(
            landed_balances(&landed),
            keys.iter()
                .filter(|key| **key == from || **key == to)
                .map(|key| if *key == from {
                    (from, 10_000, 4_000)
                } else {
                    (to, 0, 1_000)
                })
                .collect::<Vec<_>>()
        );
        let paid = change_for(&landed, &from);
        assert_eq!(paid.lamports, -6_000);
        assert!(paid.tokens.is_empty());
        let got = change_for(&landed, &to);
        assert_eq!(got.lamports, 1_000);
        assert_eq!(
            got.tokens,
            vec![TokenDelta {
                mint,
                decimals: 6,
                amount: 2_500_000
            }]
        );
    }
}
