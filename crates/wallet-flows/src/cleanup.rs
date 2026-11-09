//! Burning and closing a wallet's token accounts, to get their rent back.
//!
//! [`read`] lists the wallet's token accounts with their mints, from the
//! chain. The screen picks what to do with each ([`default_choice`] is where
//! it starts), [`plan`] says what will happen and packs it into steps, and
//! [`prepare`] compiles one step for the Sign screen. Nothing is kept
//! between steps: after one lands, the accounts are read again and planned
//! again, and what was closed is simply no longer there.

use crate::compile::{compile_for_route, with_nonce_note};
use crate::error::{FlowError, Prepared};
use crate::transfer::{fee_payer, is_plain_key};
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet};
use wallet_core::spl::{is_native_mint, Mint, TokenAccount};
use wallet_core::token_plan::{self, Choice, Held, Limits, Outcome, Parties, TokenPlan};
use wallet_core::{format_sol, Phrase, SigningRoute, SolanaRpc};

/// The token accounts of `owner`, each with its mint when it could be read,
/// in the order the node listed them.
pub async fn read(rpc: &dyn SolanaRpc, owner: Pubkey) -> Result<Vec<Held>, FlowError> {
    let listed = rpc.get_token_accounts_by_owner(&owner).await?;
    let addresses: Vec<Pubkey> = listed.iter().map(|found| found.pubkey).collect();
    let raws = rpc.get_accounts(&addresses).await?;
    let mut accounts = Vec::new();
    for (address, raw) in addresses.into_iter().zip(raws) {
        // One closed since it was listed is gone, which is what was wanted.
        let Some(raw) = raw else { continue };
        let Some(account) = TokenAccount::parse(&raw) else {
            continue;
        };
        accounts.push((address, raw.lamports, account));
    }
    let mints: Vec<Pubkey> = accounts
        .iter()
        .map(|(_, _, account)| account.mint)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mint_accounts = rpc.get_accounts(&mints).await?;
    let mints: BTreeMap<Pubkey, Mint> = mints
        .into_iter()
        .zip(mint_accounts)
        .filter_map(|(mint, raw)| Some((mint, Mint::parse(&raw?)?)))
        .collect();
    Ok(accounts
        .into_iter()
        .map(|(address, lamports, account)| Held {
            address,
            lamports,
            mint: mints.get(&account.mint).cloned(),
            account,
        })
        .collect())
}

/// What the screen starts with for `held`: close an empty account, burn and
/// close one of a token the user or the market flagged, and leave the rest.
/// Wrapped SOL is left too: closing it unwraps it, which is for the user to
/// ask for.
pub fn default_choice(held: &Held, flagged: bool) -> Choice {
    if is_native_mint(&held.account.mint) {
        Choice::Leave
    } else if held.account.amount == 0 {
        Choice::Close
    } else if flagged {
        Choice::Burn
    } else {
        Choice::Leave
    }
}

/// Who takes part in the steps of the wallet of `route`: it signs, its fee
/// payer pays, and closed accounts' rent goes back to it.
pub fn parties(route: &SigningRoute) -> Result<Parties, FlowError> {
    let payer = fee_payer(route)?;
    Ok(Parties {
        owner: route.address,
        fee_payer: payer,
        rent_payer: if is_plain_key(route) {
            payer
        } else {
            route.address
        },
        rent_to: route.address,
    })
}

/// How much one step may hold for the wallet of `route`.
pub fn limits(route: &SigningRoute) -> Limits {
    if is_plain_key(route) {
        Limits::PLAIN
    } else {
        Limits::WRAPPED
    }
}

/// Plan `held` with what was `chosen` for each account, by address. An
/// account with no choice is left.
pub fn plan(
    route: &SigningRoute,
    held: Vec<Held>,
    chosen: &BTreeMap<Pubkey, Choice>,
) -> Result<TokenPlan, FlowError> {
    let parties = parties(route)?;
    let items = held
        .into_iter()
        .map(|held| {
            let choice = chosen.get(&held.address).cloned().unwrap_or(Choice::Leave);
            (held, choice)
        })
        .collect();
    Ok(token_plan::plan(parties, items, limits(route)))
}

/// Step `index` of `plan`, compiled for the wallet of `route` on a fresh
/// blockhash or its durable nonce, and the height at which a blockhash
/// expires. `done` is how many steps of this cleanup landed before, so the
/// note can count them.
pub async fn prepare(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
    plan: &TokenPlan,
    index: usize,
    done: usize,
) -> Result<(Prepared, Option<u64>), FlowError> {
    let step = plan
        .steps
        .get(index)
        .ok_or_else(|| FlowError::refused("There is nothing left to close."))?;
    let payer = fee_payer(route)?;
    let (wire, height) = compile_for_route(rpc, route, payer, step.instructions.clone()).await?;
    let note = step_note(route.address, plan, index, done);
    Ok((
        Prepared {
            wire,
            note: with_nonce_note(note, route),
        },
        height,
    ))
}

/// What step `index` of `plan` does, in a sentence or two.
fn step_note(owner: Pubkey, plan: &TokenPlan, index: usize, done: usize) -> Phrase {
    let step = &plan.steps[index];
    let in_step: Vec<_> = plan
        .accounts
        .iter()
        .filter(|planned| step.accounts.contains(&planned.address))
        .collect();
    let mut closed = 0;
    let mut burned = 0;
    let mut unwrapped = 0u64;
    let mut returned = 0u64;
    for planned in &in_step {
        match planned.outcome {
            Outcome::Closed => closed += 1,
            Outcome::Burned { .. } => {
                closed += 1;
                burned += 1;
            }
            Outcome::Unwrapped { lamports } => {
                closed += 1;
                unwrapped += lamports;
            }
            Outcome::Moved { closed: true, .. } => closed += 1,
            Outcome::Moved { closed: false, .. } | Outcome::Left(_) => {}
        }
        returned += planned.returned;
    }
    let total = done + plan.steps.len();
    let mut note = Phrase::default();
    if total > 1 {
        note = note.t(format!("Step {} of {total}. ", done + index + 1));
    }
    note = note
        .t(format!("This closes {} of ", accounts(closed)))
        .a(owner);
    if burned > 0 {
        note = note.t(format!(
            ", burning what {} held",
            if burned == 1 {
                "one of them".to_string()
            } else {
                format!("{burned} of them")
            }
        ));
    }
    if unwrapped > 0 {
        note = note.t(format!(", and unwraps {}", format_sol(unwrapped)));
    }
    note.t(format!(". {} goes back to ", format_sol(returned)))
        .a(owner)
        .t(".")
}

fn accounts(count: usize) -> String {
    if count == 1 {
        "one token account".into()
    } else {
        format!("{count} token accounts")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{mint_account, pk, token_account, FakeRpc};
    use wallet_core::{
        decode_v1_transaction, DiscoveredTokenAccount, RouteStep, WalletId, NATIVE_MINT,
        TOKEN_PROGRAM_ID,
    };

    fn plain(owner: Pubkey) -> SigningRoute {
        SigningRoute {
            address: owner,
            steps: vec![RouteStep::Key {
                wallet: WalletId::new(),
                pubkey: owner,
            }],
            fee_payer: None,
            durable_nonce: None,
        }
    }

    fn listed(owner: Pubkey, account: Pubkey, mint: Pubkey) -> DiscoveredTokenAccount {
        DiscoveredTokenAccount {
            pubkey: account,
            owner,
            mint,
            amount: 0,
            decimals: 0,
        }
    }

    fn with_lamports(mut raw: wallet_core::RawAccount, lamports: u64) -> wallet_core::RawAccount {
        raw.lamports = lamports;
        raw
    }

    /// A wallet with an empty account, one holding tokens, and one whose mint
    /// is not on chain.
    fn node(owner: Pubkey) -> FakeRpc {
        FakeRpc::default()
            .with_tokens(
                owner,
                vec![
                    listed(owner, pk(10), pk(20)),
                    listed(owner, pk(11), pk(21)),
                    listed(owner, pk(12), pk(22)),
                    // Listed, then closed before it was read.
                    listed(owner, pk(13), pk(20)),
                ],
            )
            .with_account(
                pk(10),
                with_lamports(token_account(TOKEN_PROGRAM_ID, pk(20), owner, 0), 2_000),
            )
            .with_account(
                pk(11),
                with_lamports(token_account(TOKEN_PROGRAM_ID, pk(21), owner, 50), 3_000),
            )
            .with_account(pk(12), token_account(TOKEN_PROGRAM_ID, pk(22), owner, 9))
            .with_account(pk(20), mint_account(TOKEN_PROGRAM_ID, 0))
            .with_account(pk(21), mint_account(TOKEN_PROGRAM_ID, 2))
    }

    #[test]
    fn read_lists_each_account_with_its_mint() {
        let owner = pk(1);
        let held = pollster::block_on(read(&node(owner), owner)).unwrap();
        let summary: Vec<_> = held
            .iter()
            .map(|held| {
                (
                    held.address,
                    held.lamports,
                    held.account.amount,
                    held.mint.is_some(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (pk(10), 2_000, 0, true),
                (pk(11), 3_000, 50, true),
                (pk(12), 1, 9, false),
            ]
        );
        assert_eq!(held[1].mint.as_ref().unwrap().decimals, 2);
    }

    #[test]
    fn the_screen_starts_by_closing_empty_accounts_and_burning_flagged_ones() {
        let owner = pk(1);
        let held = pollster::block_on(read(&node(owner), owner)).unwrap();
        assert_eq!(default_choice(&held[0], false), Choice::Close);
        assert_eq!(default_choice(&held[1], false), Choice::Leave);
        assert_eq!(default_choice(&held[1], true), Choice::Burn);
        let mut wrapped = held[1].clone();
        wrapped.account.mint = NATIVE_MINT;
        assert_eq!(default_choice(&wrapped, true), Choice::Leave);
    }

    #[test]
    fn a_step_is_compiled_for_the_wallet_and_says_what_it_does() {
        let owner = pk(1);
        let rpc = node(owner);
        let route = plain(owner);
        let held = pollster::block_on(read(&rpc, owner)).unwrap();
        let chosen = BTreeMap::from([(pk(10), Choice::Close), (pk(11), Choice::Burn)]);
        let plan = plan(&route, held, &chosen).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.returned(), 5_000);
        let (prepared, height) = pollster::block_on(prepare(&rpc, &route, &plan, 0, 0)).unwrap();
        assert!(height.is_some());
        let tx = decode_v1_transaction(&prepared.wire).unwrap();
        assert_eq!(tx.fee_payer(), Some(owner));
        assert_eq!(tx.instructions.len(), 3);
        let note = prepared.note.plain();
        assert!(
            note.starts_with("This closes 2 token accounts of"),
            "{note}"
        );
        assert!(note.contains("burning what one of them held"), "{note}");
        assert!(!note.contains("Step"), "{note}");

        // Counted among the steps of the whole cleanup.
        let (prepared, _) = pollster::block_on(prepare(&rpc, &route, &plan, 0, 2)).unwrap();
        assert!(prepared.note.plain().starts_with("Step 3 of 3. "));
        assert!(pollster::block_on(prepare(&rpc, &route, &plan, 1, 0)).is_err());
    }

    #[test]
    fn a_wallet_sign_wraps_gets_smaller_steps_and_pays_rent_itself() {
        let owner = pk(1);
        let route = SigningRoute {
            address: owner,
            steps: vec![RouteStep::Programmatic {
                wallet: WalletId::new(),
                authority: pk(5),
                address: owner,
                nonce: None,
            }],
            fee_payer: Some((WalletId::new(), pk(6))),
            durable_nonce: None,
        };
        assert_eq!(limits(&route), Limits::WRAPPED);
        let parties = parties(&route).unwrap();
        assert_eq!(
            (parties.fee_payer, parties.rent_payer, parties.rent_to),
            (pk(6), owner, owner)
        );
        let keyless = SigningRoute {
            fee_payer: None,
            ..route
        };
        assert!(matches!(
            super::parties(&keyless),
            Err(FlowError::NoFeePayer)
        ));
    }
}
