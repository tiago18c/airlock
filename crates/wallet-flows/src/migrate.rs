//! Moving everything a wallet holds to another address: its token accounts,
//! its stake accounts' authorities, the nonce accounts it controls, then its
//! SOL.
//!
//! [`check_destination`] says whether an address can receive, and warns of
//! one nothing shows anyone uses. [`read`] puts together a [`Snapshot`] of
//! what the wallet holds. The screen picks what to do ([`default_choices`]
//! is where it starts), [`plan`] says what will happen, and [`prepare`]
//! compiles the next step for the Sign screen. Token steps come first. The
//! last step hands the stake accounts over, closes the nonce accounts and
//! sweeps the SOL, at an amount worked out when it is compiled, so the
//! wallet is left with nothing it could use. Nothing is kept between steps:
//! after one lands, the wallet is read and planned again.

use crate::cleanup;
use crate::compile::{compile_for_route, with_nonce_note};
use crate::error::{FlowError, Prepared};
use crate::stake::fetch_stakes;
use crate::transfer::{fee_payer, is_plain_key};
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet};
use wallet_core::migrate::{
    chunk_ops, close_nonce, hand_over_stake, must_keep, sweep, sweep_amount, NonceHeld,
    NonceOutcome, Now, StakeChoice, StakeOutcome,
};
use wallet_core::spl::{associated_token_address, is_native_mint, Mint, TokenAccount};
use wallet_core::stake::StakeAccount;
use wallet_core::token_plan::{self, Choice, Held, Outcome, Receiver, TokenPlan};
use wallet_core::{
    compile_v1, decode_v1_transaction, format_sol, parse_nonce_account, Phrase, RawAccount,
    SigningRoute, SolanaRpc, STAKE_PROGRAM_ID, SYSTEM_PROGRAM_ID,
};

/// The size of a System nonce account, and where its authority sits.
const NONCE_ACCOUNT_LEN: u64 = 80;
const NONCE_AUTHORITY_OFFSET: usize = 8;

/// How many stake or nonce instructions go in one step.
const OPS_PER_STEP_PLAIN: usize = 12;
const OPS_PER_STEP_WRAPPED: usize = 4;

/// What [`check_destination`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DestinationCheck {
    /// It holds no SOL and the node knows of no transaction naming it, so
    /// nothing shows that anyone holds its key. Worth a warning when it was
    /// pasted rather than picked from the vault.
    pub unused: bool,
}

/// Whether `to` can receive what `source` holds. Refused for the wallet
/// itself, a program, a mint, a token account, a stake account, and an
/// account another program owns, unless it is `known`: a wallet of this
/// vault, such as a Vector account, which its program owns.
pub async fn check_destination(
    rpc: &dyn SolanaRpc,
    source: Pubkey,
    to: Pubkey,
    known: bool,
) -> Result<DestinationCheck, FlowError> {
    if to == source {
        return Err(FlowError::refused("That is the wallet being moved."));
    }
    let account = rpc.get_account(&to).await?;
    if let Some(account) = &account {
        if account.executable {
            return Err(FlowError::refused(format!(
                "{to} is a program. Nothing it holds could be moved again."
            )));
        }
        if Mint::parse(account).is_some() {
            return Err(FlowError::refused(format!("{to} is a token mint.")));
        }
        if TokenAccount::parse(account).is_some() {
            return Err(FlowError::refused(format!(
                "{to} is a token account. Give the wallet that owns it."
            )));
        }
        if account.owner == STAKE_PROGRAM_ID {
            return Err(FlowError::refused(format!("{to} is a stake account.")));
        }
        if account.owner != SYSTEM_PROGRAM_ID && !known {
            return Err(FlowError::refused(format!(
                "{to} belongs to the program {}. Add it as a wallet first to move into it.",
                account.owner
            )));
        }
    }
    let empty = account.as_ref().is_none_or(|account| account.lamports == 0);
    let unused = empty
        && rpc
            .get_signatures_for_address(&to, 1)
            .await
            // A node that cannot say is no evidence of use.
            .map_or(true, |found| found.is_empty());
    Ok(DestinationCheck { unused })
}

/// What a wallet holds, as read for a move to `to`.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub source: Pubkey,
    pub to: Pubkey,
    /// The wallet's own account. `None` when it holds nothing.
    pub account: Option<RawAccount>,
    /// What the wallet's account must keep to exist, when it is not a plain
    /// account a sweep may empty.
    pub rent_exempt: u64,
    pub tokens: Vec<Held>,
    /// The destination's associated token account of each token, by its
    /// address, `None` when it does not exist yet.
    pub receivers: BTreeMap<Pubkey, Option<TokenAccount>>,
    pub stakes: Vec<StakeAccount>,
    /// Why stake accounts could not be listed, when they could not.
    pub stakes_unread: Option<String>,
    pub nonces: Vec<NonceHeld>,
    /// Why nonce accounts could not be listed: many nodes refuse to list the
    /// System program's accounts.
    pub nonces_unread: Option<String>,
    pub now: Now,
}

/// Read what the wallet of `route` holds, for a move to `to`. `unix_time`
/// is now, for stake lockups.
pub async fn read(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
    to: Pubkey,
    unix_time: i64,
) -> Result<Snapshot, FlowError> {
    let tokens = cleanup::read(rpc, route.address).await?;
    let source = route.address;
    let (stakes, stakes_unread, epoch) = match fetch_stakes(rpc, &[source]).await {
        Ok(snapshot) => (
            snapshot
                .of(&source)
                .into_iter()
                .map(|listed| listed.account)
                .collect(),
            None,
            snapshot.epoch.epoch,
        ),
        Err(err) => (Vec::new(), Some(err.to_string()), 0),
    };
    let (nonces, nonces_unread) = match rpc
        .get_program_accounts(
            &SYSTEM_PROGRAM_ID,
            NONCE_ACCOUNT_LEN,
            NONCE_AUTHORITY_OFFSET,
            &source,
        )
        .await
    {
        Ok(rows) => (
            rows.iter()
                .filter_map(|(address, raw)| {
                    let nonce = parse_nonce_account(*address, raw)?;
                    (nonce.authority == source).then_some(NonceHeld {
                        address: *address,
                        lamports: raw.lamports,
                    })
                })
                .collect(),
            None,
        ),
        Err(err) => (Vec::new(), Some(err.to_string())),
    };
    let mut snapshot = snapshot_with(rpc, route, to, tokens, Now { epoch, unix_time }).await?;
    snapshot.stakes = stakes;
    snapshot.stakes_unread = stakes_unread;
    snapshot.nonces = nonces;
    snapshot.nonces_unread = nonces_unread;
    Ok(snapshot)
}

/// A snapshot of `tokens`, with the wallet's own account and the
/// destination's token accounts read, and no stake or nonce accounts. What
/// [`read`] builds on, and what a test that lists accounts itself starts
/// from.
pub async fn snapshot_with(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
    to: Pubkey,
    tokens: Vec<Held>,
    now: Now,
) -> Result<Snapshot, FlowError> {
    let source = route.address;
    let receiving: Vec<Pubkey> = tokens
        .iter()
        .map(|held| associated_token_address(&to, &held.account.mint, &held.account.program))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let read = rpc.get_accounts(&receiving).await?;
    let receivers = receiving
        .into_iter()
        .zip(read)
        .map(|(address, raw)| (address, raw.as_ref().and_then(TokenAccount::parse)))
        .collect();
    let account = rpc.get_account(&source).await?;
    let rent_exempt = match &account {
        Some(raw) if must_keep(raw, 1) > 0 => rpc.get_rent_exemption(raw.data.len()).await?,
        _ => 0,
    };
    Ok(Snapshot {
        source,
        to,
        account,
        rent_exempt,
        tokens,
        receivers,
        stakes: Vec::new(),
        stakes_unread: None,
        nonces: Vec::new(),
        nonces_unread: None,
        now,
    })
}

/// What to do with a token account in a move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenMove {
    /// Send its tokens to the destination, then close it. An empty account
    /// is just closed, and wrapped SOL unwrapped.
    Move,
    /// Burn its tokens, then close it.
    Burn,
    Leave,
}

/// What the user picked, by address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    pub tokens: BTreeMap<Pubkey, TokenMove>,
    pub stakes: BTreeMap<Pubkey, StakeChoice>,
    /// The nonce accounts to close.
    pub nonces: BTreeSet<Pubkey>,
}

/// What the screen starts with: move every token but the flagged ones,
/// which are burned; hand both authorities of every stake account over;
/// close every nonce account the wallet's transactions do not live on.
pub fn default_choices(
    route: &SigningRoute,
    snapshot: &Snapshot,
    flagged: &BTreeSet<Pubkey>,
) -> Choices {
    Choices {
        tokens: snapshot
            .tokens
            .iter()
            .map(|held| {
                let burn = flagged.contains(&held.address)
                    && held.account.amount > 0
                    && !is_native_mint(&held.account.mint);
                let choice = if burn {
                    TokenMove::Burn
                } else {
                    TokenMove::Move
                };
                (held.address, choice)
            })
            .collect(),
        stakes: snapshot
            .stakes
            .iter()
            .map(|stake| (stake.pubkey, StakeChoice::BOTH))
            .collect(),
        nonces: snapshot
            .nonces
            .iter()
            .map(|nonce| nonce.address)
            .filter(|address| route.durable_nonce != Some(*address))
            .collect(),
    }
}

/// What a move does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationPlan {
    pub tokens: TokenPlan,
    pub stakes: Vec<(Pubkey, StakeOutcome)>,
    pub nonces: Vec<(Pubkey, NonceOutcome)>,
    /// The stake and nonce instructions, step by step. The sweep joins the
    /// last.
    ops: Vec<Vec<Instruction>>,
    /// About what the sweep sends, from what was read: the wallet's
    /// balance, with the rent of the accounts that close and less that of
    /// the receiving accounts it creates. The step itself works it out
    /// again exactly.
    pub sweep_estimate: u64,
}

impl MigrationPlan {
    /// Steps after the token steps: those of the stake and nonce
    /// instructions, the sweep with the last, or the sweep alone.
    pub fn final_steps(&self) -> usize {
        if self.ops.is_empty() {
            usize::from(self.sweep_estimate > 0)
        } else {
            self.ops.len()
        }
    }

    /// Every step the move takes from here.
    pub fn steps(&self) -> usize {
        self.tokens.steps.len() + self.final_steps()
    }
}

/// Plan `choices` for the wallet of `route`.
pub fn plan(
    route: &SigningRoute,
    snapshot: &Snapshot,
    choices: &Choices,
) -> Result<MigrationPlan, FlowError> {
    let parties = cleanup::parties(route)?;
    let to = snapshot.to;
    let items = snapshot
        .tokens
        .iter()
        .map(|held| {
            let choice = match choices.tokens.get(&held.address) {
                Some(TokenMove::Move) => {
                    let ata =
                        associated_token_address(&to, &held.account.mint, &held.account.program);
                    Choice::Move(Receiver {
                        owner: to,
                        existing: snapshot.receivers.get(&ata).cloned().flatten(),
                    })
                }
                Some(TokenMove::Burn) => Choice::Burn,
                Some(TokenMove::Leave) | None => Choice::Leave,
            };
            (held.clone(), choice)
        })
        .collect();
    let tokens = token_plan::plan(parties, items, cleanup::limits(route));

    let mut ops = Vec::new();
    let stakes = snapshot
        .stakes
        .iter()
        .map(|stake| {
            let choice = choices
                .stakes
                .get(&stake.pubkey)
                .copied()
                .unwrap_or_default();
            let (outcome, instructions) =
                hand_over_stake(snapshot.source, to, stake, choice, snapshot.now);
            ops.push(instructions);
            (stake.pubkey, outcome)
        })
        .collect();
    let nonces: Vec<_> = snapshot
        .nonces
        .iter()
        .map(|nonce| {
            let close = choices.nonces.contains(&nonce.address);
            let (outcome, instructions) =
                close_nonce(snapshot.source, nonce, close, route.durable_nonce);
            ops.push(instructions);
            (nonce.address, outcome)
        })
        .collect();
    let per_step = if is_plain_key(route) {
        OPS_PER_STEP_PLAIN
    } else {
        OPS_PER_STEP_WRAPPED
    };

    let balance = snapshot.account.as_ref().map_or(0, |raw| raw.lamports);
    let created: u64 = tokens
        .accounts
        .iter()
        .filter(|planned| matches!(planned.outcome, Outcome::Moved { created: true, .. }))
        .count() as u64;
    let created_rent = if parties.rent_payer == snapshot.source {
        created.saturating_mul(ATA_RENT)
    } else {
        0
    };
    let incoming = tokens.returned().saturating_add(closed_lamports(&nonces));
    let keep = snapshot
        .account
        .as_ref()
        .map_or(0, |raw| must_keep(raw, snapshot.rent_exempt));
    let sweep_estimate = sweep_amount(balance, incoming, created_rent, keep);

    Ok(MigrationPlan {
        tokens,
        stakes,
        nonces,
        ops: chunk_ops(ops, per_step),
        sweep_estimate,
    })
}

/// About what an associated token account's rent is, for the estimate.
const ATA_RENT: u64 = 2_039_280;

fn closed_lamports(nonces: &[(Pubkey, NonceOutcome)]) -> u64 {
    nonces
        .iter()
        .map(|(_, outcome)| match outcome {
            NonceOutcome::Closed { lamports } => *lamports,
            NonceOutcome::Left(_) => 0,
        })
        .sum()
}

/// The next step of `plan`, compiled for the wallet of `route`, and the
/// height at which a blockhash expires. `done` is how many steps of this
/// move landed before, so the note can count them. `None` when nothing is
/// left to do.
pub async fn prepare(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
    snapshot: &Snapshot,
    plan: &MigrationPlan,
    done: usize,
) -> Result<Option<(Prepared, Option<u64>)>, FlowError> {
    let total = done + plan.steps();
    let counted = |note: Phrase| {
        if total > 1 {
            Phrase::default()
                .t(format!("Step {} of {total}. ", done + 1))
                .and(note)
        } else {
            note
        }
    };
    let payer = fee_payer(route)?;
    let (instructions, note) = if let Some(step) = plan.tokens.steps.first() {
        (step.instructions.clone(), token_note(snapshot, plan, step))
    } else if plan.ops.len() > 1 {
        let ops = plan.ops[0].clone();
        let note = ops_note(snapshot, &ops).t(".");
        (ops, note)
    } else {
        let ops = plan.ops.first().cloned().unwrap_or_default();
        // Read now: the token steps changed it.
        let account = rpc.get_account(&snapshot.source).await?;
        let balance = account.as_ref().map_or(0, |raw| raw.lamports);
        let keep = account
            .as_ref()
            .map_or(0, |raw| must_keep(raw, snapshot.rent_exempt));
        let incoming = closed_lamports(&plan.nonces);
        let fee = if payer == snapshot.source {
            fee_of(payer, &ops, snapshot)?
        } else {
            0
        };
        let amount = sweep_amount(balance, incoming, fee, keep);
        if amount == 0 && ops.is_empty() {
            return Ok(None);
        }
        let mut instructions = ops.clone();
        let mut note = if ops.is_empty() {
            Phrase::default().t("This")
        } else {
            ops_note(snapshot, &ops).t(", and")
        };
        if amount > 0 {
            instructions.push(sweep(snapshot.source, snapshot.to, amount));
            note = note
                .t(format!(" sends {} to ", format_sol(amount)))
                .a(snapshot.to)
                .t(", ");
            note = if keep == 0 {
                note.t("which leaves ").a(snapshot.source).t(" empty.")
            } else {
                note.t(format!("which leaves {} in ", format_sol(keep)))
                    .a(snapshot.source)
                    .t(": the rent its account has to keep.")
            };
        } else {
            note = note.t(" leaves no SOL to send.");
        }
        (instructions, note)
    };
    let (prepared, height) = compile(rpc, route, payer, instructions, counted(note)).await?;
    Ok(Some((prepared, height)))
}

async fn compile(
    rpc: &dyn SolanaRpc,
    route: &SigningRoute,
    payer: Pubkey,
    instructions: Vec<Instruction>,
    note: Phrase,
) -> Result<(Prepared, Option<u64>), FlowError> {
    let (wire, height) = compile_for_route(rpc, route, payer, instructions).await?;
    Ok((
        Prepared {
            wire,
            note: with_nonce_note(note, route),
        },
        height,
    ))
}

/// The fee of the last step, which the wallet pays itself: its
/// signatures, as the step compiles. The sweep's amount does not change it.
fn fee_of(payer: Pubkey, ops: &[Instruction], snapshot: &Snapshot) -> Result<u64, FlowError> {
    let mut instructions = ops.to_vec();
    instructions.push(sweep(snapshot.source, snapshot.to, 1));
    let compiled =
        compile_v1(payer, instructions, None, Some(Hash::default())).map_err(FlowError::refused)?;
    let tx = decode_v1_transaction(&compiled.wire).map_err(FlowError::refused)?;
    Ok(tx.fee_lamports())
}

/// What a token step does.
fn token_note(snapshot: &Snapshot, plan: &MigrationPlan, step: &token_plan::Step) -> Phrase {
    let mut moved = 0;
    let mut burned = 0;
    let mut closed = 0;
    for planned in plan
        .tokens
        .accounts
        .iter()
        .filter(|planned| step.accounts.contains(&planned.address))
    {
        match planned.outcome {
            Outcome::Moved { .. } => moved += 1,
            Outcome::Burned { .. } => burned += 1,
            Outcome::Closed | Outcome::Unwrapped { .. } => closed += 1,
            Outcome::Left(_) => {}
        }
    }
    let mut parts = Vec::new();
    if moved > 0 {
        parts.push(format!("moves the tokens of {}", accounts(moved)));
    }
    if burned > 0 {
        parts.push(format!("burns those of {}", accounts(burned)));
    }
    if closed > 0 {
        parts.push(format!("closes {} with nothing in it", accounts(closed)));
    }
    Phrase::default()
        .t(format!("This {} of ", join(&parts)))
        .a(snapshot.source)
        .t(", to ")
        .a(snapshot.to)
        .t(". Their rent goes back to ")
        .a(snapshot.source)
        .t(", and leaves with its SOL in the last step.")
}

/// What a step of stake and nonce instructions does, up to its last word.
fn ops_note(snapshot: &Snapshot, ops: &[Instruction]) -> Phrase {
    let stakes: BTreeSet<Pubkey> = ops
        .iter()
        .filter(|ix| ix.program_id == STAKE_PROGRAM_ID)
        .filter_map(|ix| ix.accounts.first().map(|meta| meta.pubkey))
        .collect();
    let nonces = ops
        .iter()
        .filter(|ix| ix.program_id == SYSTEM_PROGRAM_ID)
        .count();
    let mut parts = Vec::new();
    if !stakes.is_empty() {
        parts.push(format!(
            "hands {} over",
            if stakes.len() == 1 {
                "a stake account".to_string()
            } else {
                format!("{} stake accounts", stakes.len())
            }
        ));
    }
    if nonces > 0 {
        parts.push(format!(
            "closes {}",
            if nonces == 1 {
                "a nonce account".to_string()
            } else {
                format!("{nonces} nonce accounts")
            }
        ));
    }
    Phrase::default()
        .t(format!("This {} of ", join(&parts)))
        .a(snapshot.source)
        .t(" to ")
        .a(snapshot.to)
}

fn accounts(count: usize) -> String {
    if count == 1 {
        "one token account".into()
    } else {
        format!("{count} token accounts")
    }
}

fn join(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests;
