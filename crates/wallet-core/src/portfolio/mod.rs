//! What the wallets on this device, and the addresses watched with them, hold
//! and are worth. Amounts come from this app's own reads of the chain; prices
//! and asset groups from the market cache.

#[cfg(test)]
mod tests;

pub use crate::market::*;

use crate::book::{AddressBook, Holdings, TokenFlag};
use crate::units::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;

/// An address whose holdings count: a wallet, or a watched book entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub address: Pubkey,
    pub label: String,
    /// Watched from the address book, not a wallet of this device.
    pub watched: bool,
}

/// A stake account and the address it is counted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldStake {
    pub owner: Pubkey,
    pub account: Pubkey,
    pub lamports: u64,
}

/// What a line holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Asset {
    Sol,
    StakedSol,
    Token(Pubkey),
}

impl Asset {
    /// The mint that prices it.
    pub fn price_mint(&self) -> Pubkey {
        match self {
            Asset::Sol | Asset::StakedSol => WRAPPED_SOL_MINT,
            Asset::Token(mint) => *mint,
        }
    }
}

/// One owner's share of a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Share {
    pub owner: Pubkey,
    pub raw: u64,
    /// The token or stake accounts it sits in, largest first. Empty for SOL.
    pub accounts: Vec<Pubkey>,
    pub usd: Option<f64>,
}

/// One asset, summed over every owner shown.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub asset: Asset,
    pub symbol: String,
    pub name: Option<String>,
    pub raw: u128,
    pub decimals: u8,
    pub price_usd: Option<f64>,
    pub change_24h_pct: Option<f64>,
    pub usd: Option<f64>,
    pub check: Option<TokenCheck>,
    /// Largest first.
    pub shares: Vec<Share>,
}

/// Lines that belong together: SOL with staked SOL and liquid staking
/// tokens, or the variants of one asset, such as USDC and USDT.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// "sol", the service's asset id, or the mint.
    pub key: String,
    pub title: String,
    /// Largest first.
    pub lines: Vec<Line>,
    pub usd: f64,
    pub change_24h_usd: Option<f64>,
}

/// The whole picture, for every owner or for one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Portfolio {
    /// Groups with at least one priced line, largest first.
    pub groups: Vec<Group>,
    /// Tokens with no price.
    pub unpriced: Vec<Line>,
    /// One-of-a-kind tokens: no decimals, one held, no price.
    pub collectibles: Vec<Line>,
    /// Tokens flagged red, or that pass themselves off as another token.
    /// Listed, not counted.
    pub left_out: Vec<Line>,
    pub total_usd: f64,
    pub change_24h_usd: Option<f64>,
    /// No price was known for anything: the total means nothing.
    pub no_prices: bool,
}

/// Decimals of an asset, and how much of it each owner holds and in which
/// accounts. Keyed by the asset and whether it is left out of the total.
type Holders = (u8, Vec<Held>);

/// An owner, their amount, and the accounts it sits in with each one's amount.
type Held = (Pubkey, u64, Vec<(Pubkey, u64)>);

/// Put together what `owners` hold, valued with `market`. `only` narrows it
/// to one owner. Red-flagged token accounts are left out of the total.
pub fn build(
    owners: &[Owner],
    book: &AddressBook,
    holdings: &Holdings,
    stakes: &[HeldStake],
    market: &MarketCache,
    only: Option<Pubkey>,
) -> Portfolio {
    let shown: Vec<&Owner> = owners
        .iter()
        .filter(|owner| only.is_none_or(|one| one == owner.address))
        .collect();

    let mut amounts: BTreeMap<(Asset, bool), Holders> = BTreeMap::new();
    let mut add = |asset: Asset,
                   left_out: bool,
                   decimals: u8,
                   owner: Pubkey,
                   account: Option<Pubkey>,
                   raw: u64| {
        if raw == 0 {
            return;
        }
        let slot = amounts
            .entry((asset, left_out))
            .or_insert_with(|| (decimals, Vec::new()));
        let held = match slot.1.iter_mut().position(|(who, _, _)| *who == owner) {
            Some(at) => &mut slot.1[at],
            None => {
                slot.1.push((owner, 0, Vec::new()));
                slot.1.last_mut().expect("just pushed")
            }
        };
        held.1 = held.1.saturating_add(raw);
        if let Some(account) = account {
            held.2.push((account, raw));
        }
    };
    let mut seen_stakes = Vec::new();
    for owner in &shown {
        if let Some(lamports) = book.get(&owner.address).and_then(|entry| entry.lamports) {
            add(Asset::Sol, false, 9, owner.address, None, lamports);
        }
        for held in holdings.page(&owner.address, 0, usize::MAX) {
            let impersonates = market
                .check(&held.mint)
                .is_some_and(TokenCheck::impersonates);
            let left_out = held.flag == Some(TokenFlag::Red) || impersonates;
            add(
                Asset::Token(held.mint),
                left_out,
                held.decimals,
                owner.address,
                Some(held.pubkey),
                held.amount,
            );
        }
        for stake in stakes.iter().filter(|stake| stake.owner == owner.address) {
            // A stake account listed under two owners counts once.
            if seen_stakes.contains(&stake.account) {
                continue;
            }
            seen_stakes.push(stake.account);
            add(
                Asset::StakedSol,
                false,
                9,
                owner.address,
                Some(stake.account),
                stake.lamports,
            );
        }
    }

    let mut portfolio = Portfolio::default();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for ((asset, left_out), (decimals, mut shares)) in amounts {
        shares.sort_by_key(|share| std::cmp::Reverse(share.1));
        for share in &mut shares {
            share.2.sort_by_key(|(_, raw)| std::cmp::Reverse(*raw));
        }
        let line = line_for(asset, decimals, &shares, market);
        if left_out {
            portfolio.left_out.push(line);
            continue;
        }
        let Some(usd) = line.usd else {
            if decimals == 0 && line.raw == 1 {
                portfolio.collectibles.push(line);
            } else {
                portfolio.unpriced.push(line);
            }
            continue;
        };
        let (key, title) = group_of(&line, market);
        let group = groups.entry(key.clone()).or_insert_with(|| Group {
            key,
            title,
            lines: Vec::new(),
            usd: 0.0,
            change_24h_usd: None,
        });
        if let Some(change) = change_usd(usd, line.change_24h_pct) {
            group.change_24h_usd = Some(group.change_24h_usd.unwrap_or(0.0) + change);
        }
        group.usd += usd;
        group.lines.push(line);
    }
    for group in groups.values_mut() {
        group
            .lines
            .sort_by(|a, b| b.usd.unwrap_or(0.0).total_cmp(&a.usd.unwrap_or(0.0)));
    }
    let mut groups: Vec<Group> = groups.into_values().collect();
    groups.sort_by(|a, b| b.usd.total_cmp(&a.usd));
    portfolio.total_usd = groups.iter().map(|group| group.usd).sum();
    portfolio.change_24h_usd = groups
        .iter()
        .filter_map(|group| group.change_24h_usd)
        .reduce(|a, b| a + b);
    portfolio.no_prices = market.quotes.is_empty();
    portfolio.groups = groups;
    portfolio
}

fn line_for(asset: Asset, decimals: u8, shares: &[Held], market: &MarketCache) -> Line {
    let mint = asset.price_mint();
    let quote = market.quote(&mint);
    let price = quote.and_then(|quote| quote.price_usd);
    let value = |raw: u128| price.map(|price| whole(raw, decimals) * price);
    let raw: u128 = shares.iter().map(|(_, raw, _)| u128::from(*raw)).sum();
    let symbol = match asset {
        Asset::Sol => "SOL".to_string(),
        Asset::StakedSol => "Staked SOL".to_string(),
        Asset::Token(mint) => quote
            .and_then(|quote| quote.symbol.clone())
            .unwrap_or_else(|| crate::short_pubkey(&mint)),
    };
    let name = match asset {
        Asset::Sol => Some("Solana".to_string()),
        Asset::StakedSol => Some("SOL in stake accounts".to_string()),
        Asset::Token(_) => quote.and_then(|quote| quote.name.clone()),
    };
    Line {
        asset,
        symbol,
        name,
        raw,
        decimals,
        price_usd: price,
        change_24h_pct: quote.and_then(|quote| quote.change_24h_pct),
        usd: value(raw),
        check: match asset {
            Asset::Token(mint) => market.check(&mint).cloned(),
            _ => None,
        },
        shares: shares
            .iter()
            .map(|(owner, raw, accounts)| Share {
                owner: *owner,
                raw: *raw,
                accounts: accounts.iter().map(|(account, _)| *account).collect(),
                usd: value(u128::from(*raw)),
            })
            .collect(),
    }
}

/// Which group a line goes in, and the group's title.
fn group_of(line: &Line, market: &MarketCache) -> (String, String) {
    let mint = match line.asset {
        Asset::Sol | Asset::StakedSol => return ("sol".into(), "SOL".into()),
        Asset::Token(mint) if mint == WRAPPED_SOL_MINT => return ("sol".into(), "SOL".into()),
        Asset::Token(mint) => mint,
    };
    match market.group(&mint) {
        Some(group) if group.lst || matches!(group.asset_id.as_str(), "sol" | "solana") => {
            ("sol".into(), "SOL".into())
        }
        Some(group) => {
            let title = group
                .name
                .clone()
                .or_else(|| group.symbol.clone())
                .unwrap_or_else(|| group.asset_id.to_uppercase());
            (format!("asset:{}", group.asset_id), title)
        }
        None => (mint.to_string(), line.symbol.clone()),
    }
}

fn whole(raw: u128, decimals: u8) -> f64 {
    if decimals == 9 {
        return raw as f64 / LAMPORTS_PER_SOL as f64;
    }
    raw as f64 / 10f64.powi(i32::from(decimals))
}

/// How many dollars of `usd` the last 24 hours added, from the percentage.
fn change_usd(usd: f64, pct: Option<f64>) -> Option<f64> {
    let pct = pct?;
    let before = usd / (1.0 + pct / 100.0);
    before.is_finite().then_some(usd - before)
}

/// Dollars as people read them: "$12,431.08", "$0.0004213" for a small price.
pub fn format_usd(usd: f64) -> String {
    let sign = if usd < 0.0 { "-" } else { "" };
    let usd = usd.abs();
    if usd > 0.0 && usd < 0.01 {
        // Four significant digits.
        let digits = (-usd.log10().floor()) as usize + 3;
        return format!("{sign}${usd:.digits$}");
    }
    let cents = (usd * 100.0).round() as u128;
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{sign}${grouped}.{:02}", cents % 100)
}

/// A value in a table: like [`format_usd`], but a value under a cent is
/// "<$0.01" rather than a long fraction.
pub fn format_value(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        "<$0.01".to_string()
    } else {
        format_usd(usd)
    }
}

/// A token amount for a table: thousands grouped, at most `max_frac`
/// decimals, trailing zeros dropped. An amount too small to show is
/// "<0.0001", never "0".
pub fn format_amount(raw: u128, decimals: u8, max_frac: usize) -> String {
    let factor = 10u128.pow(u32::from(decimals));
    let whole = (raw / factor).to_string();
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if decimals == 0 {
        return grouped;
    }
    let frac = format!("{:0width$}", raw % factor, width = decimals as usize);
    let frac: String = frac.chars().take(max_frac).collect();
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() && raw > 0 && raw < factor {
        return format!("<0.{}1", "0".repeat(max_frac.saturating_sub(1)));
    }
    if frac.is_empty() {
        grouped
    } else {
        format!("{grouped}.{frac}")
    }
}
