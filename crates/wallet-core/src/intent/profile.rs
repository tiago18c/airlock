//! How an account of this device signs, as a connected site may ask before
//! it builds anything (the table "Airlock's profiles" of `docs/intents.md`).

use super::notes::Delivery;
use super::text;
use crate::ids::WalletId;
use crate::wallets::programmatic::ED25519_SIGNER_PROGRAM_ID;
use crate::wallets::record::{Protocol, WalletKind, WalletRecord};
use crate::wallets::route::{signing_route, RouteError, RouteStep};
use crate::wallets::squads::{
    SquadState, PERMISSION_EXECUTE, PERMISSION_INITIATE, PERMISSION_VOTE,
};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use thiserror::Error;

/// The most transactions one request may carry, for every account here.
pub const MAX_PROFILE_TRANSACTIONS: u8 = 16;

/// Whether a resolution finishes while the request is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Timing {
    Sync,
    /// Later, and maybe never: a key on another device, or votes this
    /// device cannot cast.
    Async,
}

impl Timing {
    /// How long a request waits for the person: ten minutes for `sync`, a
    /// day for `async`.
    pub fn approval_window_secs(self) -> u64 {
        match self {
            Self::Sync => 10 * 60,
            Self::Async => 24 * 60 * 60,
        }
    }
}

/// Where the instructions run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Execution {
    TopLevel,
    /// Inside a program's instruction, which the profile names.
    Carrier,
}

/// Who may pay a request's fees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Payer {
    Account,
    Wallet,
    App,
}

/// How a new account may be signed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NewAccountKey {
    /// A key made on this device takes the app's key's place. It signs a
    /// transaction that runs while the request is open.
    WalletKey,
    /// The carrier program signs, at an address it derives: a Squads
    /// multisig's ephemeral signer. No key exists for it, so it holds
    /// however much later the instructions run and whoever runs them.
    Carrier,
    /// The app's key stays, and the app signs for it.
    AppKey,
}

/// How a message may be signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageMode {
    /// A key signs the bytes.
    Signature,
    /// The message is a memo in a transaction the account signs.
    Memo,
}

/// `SigningProfile` of `docs/intents.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningProfile {
    #[serde(with = "text::address")]
    pub account: Pubkey,
    pub timing: Timing,
    pub execution: Execution,
    /// The carrier program, when there is one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::option_pubkey_string"
    )]
    pub carrier: Option<Pubkey>,
    pub fee_payer: Vec<Payer>,
    pub new_accounts: Vec<NewAccountKey>,
    pub delivery: Vec<Delivery>,
    pub messages: Vec<MessageMode>,
    pub max_transactions: u8,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProfileError {
    #[error("{0} is not a wallet of this device.")]
    NotAWallet(Pubkey),
    #[error(transparent)]
    Route(#[from] RouteError),
}

/// The profile of `account`, a wallet of this device, from `wallets`, the
/// vault's records.
///
/// For a Squads multisig, `squad` is what the chain says of it, when it was
/// read. Without it the record's members and threshold decide. The record
/// keeps neither the members' permissions nor the time lock, so without
/// `squad` every member is taken to hold every permission and the multisig
/// to have no time lock: what the rebuild reads from the chain decides in
/// the end, and a request the multisig then cannot carry at once waits for
/// votes like any `async` one.
pub fn signing_profile(
    wallets: &[WalletRecord],
    account: Pubkey,
    squad: Option<&SquadState>,
) -> Result<SigningProfile, ProfileError> {
    let wallet = wallets
        .iter()
        .find(|wallet| wallet.address() == Some(account))
        .ok_or(ProfileError::NotAWallet(account))?;
    let profile = |timing, execution, carrier| SigningProfile {
        account,
        timing,
        execution,
        carrier,
        fee_payer: vec![Payer::Wallet],
        new_accounts: vec![NewAccountKey::WalletKey],
        delivery: vec![Delivery::Send],
        messages: vec![MessageMode::Memo],
        max_transactions: MAX_PROFILE_TRANSACTIONS,
    };
    // An authorization is ordered by a nonce that the one before it moves
    // (a programmatic signer's executor nonce, a Vector account's own), and
    // each transaction of a request is rebuilt on its own, from the nonce
    // as the chain holds it now: only the first of several could run.
    let one_at_a_time = signing_route(wallets, wallet.id).is_ok_and(|route| {
        route.steps.iter().any(|step| {
            matches!(
                step,
                RouteStep::Programmatic { .. } | RouteStep::Vector { .. }
            )
        })
    });
    let mut made = match &wallet.kind {
        WalletKind::Keypair { .. } | WalletKind::Seed { .. } | WalletKind::Hardware { .. } => {
            SigningProfile {
                fee_payer: vec![Payer::Account, Payer::Wallet, Payer::App],
                new_accounts: vec![NewAccountKey::WalletKey, NewAccountKey::AppKey],
                delivery: vec![Delivery::Send, Delivery::Cosign, Delivery::Return],
                messages: vec![MessageMode::Signature, MessageMode::Memo],
                ..profile(Timing::Sync, Execution::TopLevel, None)
            }
        }
        WalletKind::Offline { .. } => SigningProfile {
            fee_payer: vec![Payer::Account, Payer::Wallet],
            ..profile(Timing::Async, Execution::TopLevel, None)
        },
        WalletKind::Programmatic { protocol, .. } => {
            let carrier = match protocol {
                Protocol::Ed25519Signer { .. } => ED25519_SIGNER_PROGRAM_ID,
                Protocol::Vector { program, .. } => *program,
            };
            let timing = if signing_route(wallets, wallet.id)?.is_local() {
                Timing::Sync
            } else {
                Timing::Async
            };
            profile(timing, Execution::Carrier, Some(carrier))
        }
        WalletKind::Squads { program, .. } => {
            let timing = if squad_carried_here(wallets, wallet, squad)? {
                Timing::Sync
            } else {
                Timing::Async
            };
            // The program signs for a new account at an ephemeral signer of
            // the proposal's stored transaction, however much later it runs
            // and whoever runs it (docs/intents.md, "Airlock's profiles").
            SigningProfile {
                new_accounts: vec![NewAccountKey::Carrier],
                ..profile(timing, Execution::Carrier, Some(program.program_id()))
            }
        }
    };
    if one_at_a_time {
        made.max_transactions = 1;
    }
    Ok(made)
}

/// Whether this device can carry `wallet`, a multisig, past its threshold
/// alone: its acting member and the wallets that approve with it, those
/// whose route ends on this device, reach the threshold, one of them may
/// execute, and there is no time lock. Its acting member's own route has
/// to end here whatever the threshold.
fn squad_carried_here(
    wallets: &[WalletRecord],
    wallet: &WalletRecord,
    squad: Option<&SquadState>,
) -> Result<bool, ProfileError> {
    let WalletKind::Squads {
        members,
        threshold,
        member_wallet,
        approves_with,
        ..
    } = &wallet.kind
    else {
        return Ok(false);
    };
    // The route also refuses a member that is not one, or is a multisig.
    let route = signing_route(wallets, wallet.id)?;
    if !route.is_local() {
        return Ok(false);
    }
    let (threshold, time_lock) = match squad {
        Some(state) => (state.threshold, state.time_lock),
        None => (*threshold, 0),
    };
    let members_known = squad.is_some() || !members.is_empty();
    if threshold == 0 || !members_known || time_lock > 0 {
        return Ok(false);
    }
    // Every permission a member holds, as far as this device knows.
    let permissions = |member: &Pubkey| -> Option<u8> {
        match squad {
            Some(state) => state
                .members
                .iter()
                .find(|known| known.key == *member)
                .map(|known| known.permissions),
            None => members
                .contains(member)
                .then_some(PERMISSION_INITIATE | PERMISSION_VOTE | PERMISSION_EXECUTE),
        }
    };
    let acting: Option<WalletId> = *member_wallet;
    let mut initiates = false;
    let mut votes = 0usize;
    let mut executes = false;
    for (at, id) in acting.iter().chain(approves_with).enumerate() {
        let Some(voter) = wallets.iter().find(|candidate| candidate.id == *id) else {
            continue;
        };
        let local = signing_route(wallets, voter.id).is_ok_and(|route| route.is_local());
        let Some(granted) = voter.address().and_then(|address| permissions(&address)) else {
            continue;
        };
        if !local {
            continue;
        }
        if at == 0 {
            initiates = granted & PERMISSION_INITIATE != 0;
        }
        if granted & PERMISSION_VOTE != 0 {
            votes += 1;
        }
        executes |= granted & PERMISSION_EXECUTE != 0;
    }
    Ok(initiates && executes && votes >= usize::from(threshold))
}
