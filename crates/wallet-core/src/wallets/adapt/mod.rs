//! A transaction a site built for a wallet that has no key.
//!
//! A site that was shown a programmatic signer's address or a Squads vault
//! builds what it would for any wallet: that address signs, and usually pays
//! the fee too. No key signs as either, so the transaction cannot be
//! completed as it stands. It is rebuilt first, and only the result is read
//! and signed:
//!
//! - **Squads vault.** The site's instructions are stored as a vault
//!   transaction. The new transaction creates it, opens its proposal and, as
//!   far as fits after the Vector or programmatic wrap, approves and executes
//!   it — including co-voters the multisig's row names under "Approves
//!   together with".
//! - **Programmatic signer.** The instructions move into an authorization its
//!   authority signs (see `relay`). This also applies to a Squads member that
//!   is a programmatic signer.
//! - **Vector wallet.** The instructions it signs run inside its
//!   `Passthrough`, behind one `Advance` its key signs (see `relay`). This
//!   also applies to a Squads member that is a Vector wallet.
//!
//! Either way a hot wallet pays and signs the transaction that reaches the
//! chain, and the address without a key is an ordinary account in it, not a
//! signer.
//!
//! Rebuilding needs the chain: the multisig's next transaction number, the
//! signer's nonce, the Vector account, a blockhash. So it is two steps.
//! [`plan_adaptation`] says whether anything has to change and which
//! accounts to read.
//! [`AdaptPlan::build`] does it with what was read.

use crate::chain::RawAccount;
use crate::error::BuildError;
use crate::ids::WalletId;
use crate::tx::builder::compile_message;
use crate::tx::decode::{decode_v1_transaction, DecodedTx};
use crate::wallets::programmatic::{ExecutorNonce, EXECUTOR_NONCE_PROGRAM_ID};
use crate::wallets::proposals::MemberKind;
use crate::wallets::record::{Protocol, WalletKind, WalletRecord};
use crate::wallets::relay::{
    authority_slots, wrap_for_programmatic, wrap_for_vector, VectorContext, VectorWrap, WrapError,
};
use crate::wallets::route::{signing_route, RouteError, RouteStep};
use crate::wallets::squads::{Squad, PERMISSION_INITIATE};
use crate::wallets::vector::{SchemeId, VectorAccount, VectorError};
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use thiserror::Error;

mod build;
mod payer;
mod plan;

#[cfg(test)]
mod tests;

pub use payer::*;
pub use plan::*;

/// Compute units allowed for storing a vault transaction, opening its
/// proposal and approving it. Executing adds the stored instructions' own
/// limit on top.
pub const SQUADS_OVERHEAD_CU: u32 = 200_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdaptError {
    #[error("A transaction that advances a durable nonce cannot be rebuilt for {0}. The site has to build it on a recent blockhash.")]
    DurableNonce(String),
    #[error("This transaction asks more than one multisig to sign. A proposal belongs to one.")]
    SeveralMultisigs,
    #[error("{signer} also has to sign inside this transaction. A proposal for {squad} can only sign as its vault.")]
    OtherSigner { squad: String, signer: Pubkey },
    #[error("Reshaping this transaction is not supported for this website with {squad}. {signer}, the key made here for it, signs only when the proposal executes, so {squad} has to create, approve and execute it in one transaction, and here it cannot: {}.", held.reason())]
    NotAtOnce {
        squad: String,
        signer: Pubkey,
        held: HeldBack,
    },
    #[error("{0} has no nonce account yet. Create one on the Wallets screen and send that transaction first.")]
    NoNonce(String),
    #[error("Choose a hot wallet to pay fees for {0} on the Wallets screen.")]
    NoFeePayer(String),
    #[error("{0} is not on this cluster.")]
    NotOnChain(String),
    #[error("The account of {0} could not be read. It may belong to another program.")]
    Unreadable(String),
    #[error("{member} is not a member of {squad} on this cluster.")]
    NotAMember { squad: String, member: Pubkey },
    #[error("{member} may not create proposals for {squad}.")]
    CannotPropose { squad: String, member: Pubkey },
    #[error("The nonce account of {0} is controlled by another address.")]
    ForeignNonce(String),
    #[error("{0} is a Vector wallet, and Vector wallets are turned off. Turn them on in Settings to use it.")]
    VectorOff(String),
    #[error(
        "{0} was made under another deployment of the Vector program than the one this build uses."
    )]
    OtherProgram(String),
    #[error("{0} is not initialized on this cluster. Initialize it on the Wallets screen.")]
    NotInitialized(String),
    #[error("The account of {wallet} cannot sign: {error}")]
    VectorAccount { wallet: String, error: VectorError },
    #[error("The account of {0} on this cluster was created with another key or scheme than its wallet says.")]
    OtherKey(String),
    #[error(transparent)]
    Route(#[from] RouteError),
    #[error(transparent)]
    Wrap(#[from] WrapError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("{0}")]
    Message(String),
}

/// The site's instructions as a vault proposal should store them.
///
/// [`DecodedTx::native_instructions`] flags every account with the message's
/// signer bit. The fee payer is always a message signer, so when it also
/// appears inside an instruction — as the recipient of a transfer, for
/// example — it is reconstructed as a signer there too. A vault proposal
/// may only name the vault (and keys made here) as signers; the fee payer
/// only pays. Clearing its signer flag on every account but the vault keeps
/// the stored message honest and stops [`AdaptError::OtherSigner`] from
/// refusing a send the member both pays for and receives.
fn site_instructions(tx: &DecodedTx, vault: Pubkey) -> Vec<Instruction> {
    let payer = tx.fee_payer();
    tx.native_instructions()
        .into_iter()
        .map(|mut instruction| {
            for meta in &mut instruction.accounts {
                if Some(meta.pubkey) == payer && meta.pubkey != vault {
                    meta.is_signer = false;
                }
            }
            instruction
        })
        .collect()
}

impl AdaptError {
    /// Whether the rebuild failed only because the transaction was too large.
    pub fn is_too_large(&self) -> bool {
        matches!(
            self,
            Self::Build(BuildError::TooLarge { .. })
                | Self::Wrap(WrapError::Build(BuildError::TooLarge { .. }))
        )
    }

    /// The measured size when [`Self::is_too_large`], else `None`.
    fn too_large_len(&self) -> Option<usize> {
        match self {
            Self::Build(BuildError::TooLarge { len })
            | Self::Wrap(WrapError::Build(BuildError::TooLarge { len })) => Some(*len),
            _ => None,
        }
    }
}

/// Why a rebuilt proposal is not create, every vote and execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeldBack {
    /// The next larger shape measured this many bytes.
    Size {
        bytes: usize,
    },
    /// The next larger shape measured this many compute units.
    Compute {
        units: u32,
    },
    TimeLock,
    NoExecutor,
    /// A programmatic member creates and executes, and another member's
    /// vote would have to run between them, inside its authorization.
    Order,
    /// Fewer usable voters than votes missing.
    Votes {
        missing: u16,
    },
}

impl HeldBack {
    /// Why, in a few words.
    pub fn reason(&self) -> String {
        match self {
            Self::Size { bytes } => format!(
                "executing too would make it {bytes} of {} bytes",
                crate::MAX_TRANSACTION_SIZE
            ),
            Self::Compute { units } => {
                format!("executing too would need {units} compute units, above the limit")
            }
            Self::TimeLock => "its time lock holds execution".into(),
            Self::NoExecutor => "no member of this device may execute".into(),
            Self::Order => {
                "another member's vote would have to run inside the programmatic signer's authorization".into()
            }
            Self::Votes { missing: 1 } => "it needs 1 more approval than this device holds".into(),
            Self::Votes { missing } => {
                format!("it needs {missing} more approvals than this device holds")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SquadsPlan {
    label: String,
    squad: Squad,
    vault: Pubkey,
    vault_index: u8,
    /// The address that acts as a member: a key, or a programmatic signer.
    member: Pubkey,
    /// The acting member, then `approves_with`, in the row's order.
    members: Vec<PlannedMember>,
    /// Keys made here that the stored instructions name as signers. They
    /// sign the transaction that executes the proposal.
    one_time: Vec<Pubkey>,
}

/// A wallet of this device that may vote for the multisig, as known at plan
/// time. Permissions come from the chain in [`AdaptPlan::build`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct PlannedMember {
    wallet: WalletId,
    label: String,
    address: Pubkey,
    kind: MemberKind,
    vector: Option<VectorPlan>,
    programmatic: Option<ProgrammaticPlan>,
    /// Its executor nonce, when it has one.
    nonce: Option<Pubkey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProgrammaticPlan {
    label: String,
    authority: Pubkey,
    address: Pubkey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VectorPlan {
    label: String,
    scheme: SchemeId,
    /// The key the account was created with.
    key: Vec<u8>,
    account: Pubkey,
    /// Pending next XMSS public key for a rotating wrap.
    next_key: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NoncePlan {
    /// The label of the wallet the nonce account was taken from.
    owner: String,
    account: Pubkey,
}

/// What has to happen to a transaction before it can be signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdaptPlan {
    squads: Option<SquadsPlan>,
    programmatic: Vec<ProgrammaticPlan>,
    /// In the order the `Advance` lists them.
    vector: Vec<VectorPlan>,
    /// The nonce account that orders the authorization. It belongs to one of
    /// the programmatic signers: the first that has one.
    nonce: Option<NoncePlan>,
    /// The hot wallet that pays for and signs the rebuilt transaction.
    pub fee_payer: Pubkey,
}

/// A wallet without a key that a plan rebuilds the transaction for, for the
/// screen that offers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskedWallet {
    Squads {
        wallet: String,
        vault: Pubkey,
        /// The address that acts as a member.
        member: Pubkey,
    },
    Programmatic {
        wallet: String,
        authority: Pubkey,
        address: Pubkey,
    },
    Vector {
        wallet: String,
        scheme: SchemeId,
        account: Pubkey,
    },
}

/// One change made to the transaction, for the screen that shows the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdaptedStep {
    /// The instructions were stored for a vault to run.
    Squads {
        wallet: String,
        vault: Pubkey,
        member: Pubkey,
        /// The number the stored transaction and its proposal get.
        index: u64,
        threshold: u16,
        /// Members that approve in this transaction, in instruction order.
        voters: Vec<Pubkey>,
        /// Who executes. `None` leaves the proposal for later.
        executor: Option<Pubkey>,
        /// Why the shape is not the whole job, when it is not.
        held_back: Option<HeldBack>,
    },
    /// The instructions were moved into an authorization.
    Programmatic {
        wallet: String,
        authority: Pubkey,
        address: Pubkey,
    },
    /// The instructions the account signs run in its `Passthrough`.
    Vector {
        wallet: String,
        scheme: SchemeId,
        account: Pubkey,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapted {
    pub wire: Vec<u8>,
    /// In the order they were applied.
    pub steps: Vec<AdaptedStep>,
    pub fee_payer: Pubkey,
    /// Signatures the site's transaction carried. They covered the old
    /// message and are gone.
    pub voided: usize,
    /// The Vector accounts as they were read for the rebuild: what signing
    /// for them needs.
    pub context: Vec<VectorContext>,
}

impl AdaptPlan {
    /// This plan, unless it rebuilds for a Vector wallet while Vector
    /// wallets are turned off. A Vector co-voter of a multisig is left out
    /// instead.
    pub fn unless_vector_off(mut self, offered: bool) -> Result<Self, AdaptError> {
        if offered {
            return Ok(self);
        }
        if let Some(plan) = self.vector.first() {
            return Err(AdaptError::VectorOff(plan.label.clone()));
        }
        if let Some(squads) = &mut self.squads {
            // The acting member creates, so it has to sign. A co-voter is
            // only left out.
            if let Some(vector) = squads
                .members
                .first()
                .and_then(|acting| acting.vector.as_ref())
            {
                return Err(AdaptError::VectorOff(vector.label.clone()));
            }
            squads
                .members
                .retain(|member| member.kind != MemberKind::Vector);
        }
        Ok(self)
    }
}

/// The plan for the Vector wallet `wallet`, which has to be of the program
/// this build signs for.
fn vector_plan(wallet: &WalletRecord) -> Result<Option<VectorPlan>, AdaptError> {
    let WalletKind::Programmatic {
        protocol:
            Protocol::Vector {
                scheme,
                program,
                key,
                ..
            },
        ..
    } = &wallet.kind
    else {
        return Ok(None);
    };
    if *program != crate::wallets::vector::PROGRAM_ID {
        return Err(AdaptError::OtherProgram(wallet.label.clone()));
    }
    Ok(Some(VectorPlan {
        label: wallet.label.clone(),
        scheme: *scheme,
        key: key.0.clone(),
        account: wallet.address().unwrap_or_default(),
        next_key: wallet.vector_next_pubkey().map(<[u8]>::to_vec),
    }))
}
