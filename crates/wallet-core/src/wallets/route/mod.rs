//! From a wallet to the key that signs for it, and who pays.
//!
//! A hot wallet signs for itself. An offline wallet is signed for on another
//! device. A programmatic signer is signed for by its authority key, which is
//! a wallet of its own or, for a Vector account, may be kept in the signer's
//! record. A Squads multisig is acted on by one of its members, which may
//! itself be a programmatic signer. So the longest chain is
//!
//! ```text
//! Squads multisig  →  programmatic signer  →  hot key
//! ```
//!
//! and the fee payer is a separate question: the key at the end of the chain
//! may hold no funds at all.

use crate::ids::WalletId;
use crate::wallets::programmatic::programmatic_signer_address;
use crate::wallets::record::{Authority, Protocol, WalletKind, WalletRecord};
use crate::wallets::squads::Squad;
use crate::wallets::vector::SchemeId;
use solana_pubkey::Pubkey;
use thiserror::Error;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteStep {
    /// A multisig, acted on as `member`.
    Squads {
        wallet: WalletId,
        squad: Squad,
        vault: Pubkey,
        member: Pubkey,
    },
    /// A programmatic signer: `authority` signs a message, `address` acts.
    Programmatic {
        wallet: WalletId,
        authority: Pubkey,
        address: Pubkey,
        nonce: Option<Pubkey>,
    },
    /// A Vector account: the key that controls it signs a digest of the
    /// transaction, and `account` acts.
    Vector {
        wallet: WalletId,
        scheme: SchemeId,
        account: Pubkey,
    },
    /// A key in this vault.
    Key { wallet: WalletId, pubkey: Pubkey },
    /// A key in this vault that is no wallet of its own: it is kept in the
    /// record of the one account it controls, and has no address.
    HeldKey { wallet: WalletId },
    /// A key kept somewhere else. Its signature has to be brought in.
    OutsideKey { pubkey: Pubkey },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningRoute {
    /// The address that acts: where the funds are and what programs see.
    pub address: Pubkey,
    /// From the wallet itself down to the key that signs.
    pub steps: Vec<RouteStep>,
    /// The wallet that pays fees and signs the transaction that reaches the
    /// chain: a hot wallet, or an offline one whose signature is brought in
    /// from the device that holds its key. `None` when none is set and the
    /// key at the end of the chain is not on this device.
    pub fee_payer: Option<(WalletId, Pubkey)>,
    /// The durable nonce a new transaction lives on: set for an offline
    /// wallet that signs for itself and has one. A chain through a
    /// programmatic signer, a Vector account or a multisig has its own
    /// ordering and leaves this `None`. Deprecated once the programmatic
    /// signer or Vector is on mainnet.
    pub durable_nonce: Option<Pubkey>,
}

impl SigningRoute {
    /// The key at the end of the chain.
    pub fn signing_key(&self) -> Option<Pubkey> {
        match self.steps.last()? {
            RouteStep::Key { pubkey, .. } | RouteStep::OutsideKey { pubkey } => Some(*pubkey),
            _ => None,
        }
    }

    /// Every step can be signed on this device.
    pub fn is_local(&self) -> bool {
        matches!(
            self.steps.last(),
            Some(RouteStep::Key { .. } | RouteStep::HeldKey { .. })
        )
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RouteError {
    #[error("That wallet is no longer in the vault.")]
    UnknownWallet,
    #[error("Hardware wallets are not supported yet.")]
    Hardware,
    #[error("Choose which wallet acts as a member of {0}.")]
    NoMember(String),
    #[error("{member} is not a member of {squad}.")]
    NotAMember { squad: String, member: String },
    #[error("A multisig cannot be the member of a multisig here. Pick a hot wallet or a programmatic signer.")]
    NestedSquads,
    #[error("{0} is set as a fee payer but is not a key. Pick a hot or offline wallet.")]
    FeePayerNotAKey(String),
    #[error("These wallets refer to each other in a loop.")]
    Loop,
    #[error("{0} keeps its authority key in its own record, which only a Vector wallet can.")]
    HeldAuthority(String),
}

/// Resolve how `id` signs and who pays.
///
/// The fee payer is the first one set along the chain, starting at `id`. When
/// none is set, the key at the end of the chain pays, if this vault holds it.
pub fn signing_route(wallets: &[WalletRecord], id: WalletId) -> Result<SigningRoute, RouteError> {
    let find = |id: WalletId| {
        wallets
            .iter()
            .find(|wallet| wallet.id == id)
            .ok_or(RouteError::UnknownWallet)
    };

    let first = find(id)?;
    let address = first.address().ok_or(RouteError::Hardware)?;
    let mut steps = Vec::new();
    let mut chosen_payer = None;
    let mut current = first;
    loop {
        if steps.len() > wallets.len() {
            return Err(RouteError::Loop);
        }
        if chosen_payer.is_none() {
            chosen_payer = current.kind.fee_payer();
        }
        match &current.kind {
            WalletKind::Keypair { .. } | WalletKind::Seed { .. } => {
                let pubkey = current.address().ok_or(RouteError::UnknownWallet)?;
                steps.push(RouteStep::Key {
                    wallet: current.id,
                    pubkey,
                });
                break;
            }
            WalletKind::Offline { pubkey, .. } => {
                steps.push(RouteStep::OutsideKey { pubkey: *pubkey });
                break;
            }
            WalletKind::Hardware { .. } => return Err(RouteError::Hardware),
            WalletKind::Programmatic {
                protocol: Protocol::Ed25519Signer { authority, nonce },
                authority: held_by,
                ..
            } => {
                steps.push(RouteStep::Programmatic {
                    wallet: current.id,
                    authority: *authority,
                    address: programmatic_signer_address(authority),
                    nonce: *nonce,
                });
                match held_by {
                    Authority::Wallet { wallet } => current = find(*wallet)?,
                    Authority::Outside => {
                        steps.push(RouteStep::OutsideKey { pubkey: *authority });
                        break;
                    }
                    Authority::Held { .. } => {
                        return Err(RouteError::HeldAuthority(current.label.clone()))
                    }
                }
            }
            WalletKind::Programmatic {
                protocol: Protocol::Vector { scheme, .. },
                authority: held_by,
                ..
            } => {
                steps.push(RouteStep::Vector {
                    wallet: current.id,
                    scheme: *scheme,
                    account: current.address().ok_or(RouteError::UnknownWallet)?,
                });
                match held_by {
                    Authority::Wallet { wallet } => current = find(*wallet)?,
                    Authority::Held { .. } => {
                        steps.push(RouteStep::HeldKey { wallet: current.id });
                        break;
                    }
                    // A key of another scheme is no address, so there is no
                    // key to name: the chain ends at the account.
                    Authority::Outside => break,
                }
            }
            WalletKind::Squads {
                vault,
                members,
                member_wallet,
                ..
            } => {
                if !steps.is_empty() {
                    return Err(RouteError::NestedSquads);
                }
                let member_wallet =
                    member_wallet.ok_or_else(|| RouteError::NoMember(current.label.clone()))?;
                let next = find(member_wallet)?;
                let member = next.address().ok_or(RouteError::Hardware)?;
                // An empty list means the members were never read from the chain.
                if !members.is_empty() && !members.contains(&member) {
                    return Err(RouteError::NotAMember {
                        squad: current.label.clone(),
                        member: next.label.clone(),
                    });
                }
                steps.push(RouteStep::Squads {
                    wallet: current.id,
                    squad: current.kind.squad().expect("a squads wallet"),
                    vault: *vault,
                    member,
                });
                current = next;
            }
        }
    }

    let fee_payer = match chosen_payer {
        Some(payer) => {
            let wallet = find(payer)?;
            if !wallet.kind.can_pay() {
                return Err(RouteError::FeePayerNotAKey(wallet.label.clone()));
            }
            wallet.address().map(|pubkey| (wallet.id, pubkey))
        }
        None => match steps.last() {
            Some(RouteStep::Key { wallet, pubkey }) => Some((*wallet, *pubkey)),
            _ => None,
        },
    };

    // Only a key that signs the transaction itself advances its nonce.
    let durable_nonce = match steps.as_slice() {
        [RouteStep::OutsideKey { .. }] => first.kind.durable_nonce(),
        _ => None,
    };

    Ok(SigningRoute {
        address,
        steps,
        fee_payer,
        durable_nonce,
    })
}
