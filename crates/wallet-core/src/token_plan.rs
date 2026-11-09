//! Emptying a wallet's token accounts: what happens to each one, and the
//! transactions that do it.
//!
//! Each account gets a [`Choice`]: close it, burn its tokens and close it,
//! move its tokens to another wallet and close it, or leave it. [`plan`]
//! checks each choice against what the chain says of the account and its
//! mint, says what will happen or why it cannot, and packs the instructions
//! into steps that each fit one transaction. A closed account's rent goes to
//! the address the caller names, which is the wallet itself, so a migration
//! sweeps it with the rest of the SOL at the end.
//!
//! The plan is made from what was read, and the steps are compiled later,
//! one at a time, by whoever runs them. Nothing here reads the chain.

use crate::spl::{
    associated_token_address, burn_checked, close_account, create_ata_idempotent,
    harvest_withheld_to_mint, is_native_mint, memo, transfer_checked, Mint, TokenAccount,
};
use crate::tx::compile_v1;
use crate::TOKEN_2022_PROGRAM_ID;
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_message::v1::MAX_TRANSACTION_SIZE;
use solana_pubkey::Pubkey;

/// A token account of the wallet, as read from the chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub address: Pubkey,
    /// Everything it holds in lamports: its rent, and for wrapped SOL the
    /// wrapped amount too.
    pub lamports: u64,
    pub account: TokenAccount,
    /// Its mint, when it could be read.
    pub mint: Option<Mint>,
}

/// What to do with one token account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Close it. Only an account with no tokens, or wrapped SOL, which is
    /// unwrapped.
    Close,
    /// Burn its tokens, then close it.
    Burn,
    /// Send its tokens to another wallet, then close it.
    Move(Receiver),
    /// Do nothing with it.
    Leave,
}

/// The wallet tokens are moved to, and what the chain says of its account
/// for the mint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receiver {
    /// The wallet: its associated token account receives.
    pub owner: Pubkey,
    /// That associated token account, when it exists. `None` creates it.
    pub existing: Option<TokenAccount>,
}

/// What the plan does with an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// An empty account, closed.
    Closed,
    /// A wrapped SOL account, closed, which unwraps `lamports`.
    Unwrapped { lamports: u64 },
    /// `amount` burned, then the account closed.
    Burned { amount: u64 },
    /// `amount` sent to `to`'s associated token account, created first when
    /// `created`. The account is closed unless its close authority is
    /// another address.
    Moved {
        amount: u64,
        to: Pubkey,
        created: bool,
        closed: bool,
    },
    /// Nothing is done, for this reason.
    Left(Blocked),
}

/// Why an account is left as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Left by choice.
    Chosen,
    /// Frozen by its mint's freeze authority: nothing can move, burn or
    /// close it.
    Frozen,
    /// Another wallet owns it.
    NotOwned { owner: Pubkey },
    /// Close was chosen for an account that still holds tokens.
    HoldsTokens,
    /// Only `authority` may close it, and burning without closing would
    /// gain nothing.
    CloseAuthority { authority: Pubkey },
    /// It holds a confidential balance, which this app cannot empty.
    Confidential,
    /// Its mint could not be read, so its decimals are not known.
    MintUnknown,
    /// Its tokens cannot be sent: the mint's reason.
    CannotMove(&'static str),
    /// Its tokens cannot be burned: the mint's reason.
    CannotBurn(&'static str),
    /// The receiving account is frozen, or would be created frozen.
    ReceiverFrozen,
    /// The receiving account is this account.
    ReceiverIsSource,
}

impl Blocked {
    /// Why, for a person.
    pub fn reason(&self) -> String {
        match self {
            Blocked::Chosen => "Left as it is.".into(),
            Blocked::Frozen => {
                "Its mint has frozen it, so it cannot be moved, burned or closed.".into()
            }
            Blocked::NotOwned { owner } => format!("It belongs to {owner}, not this wallet."),
            Blocked::HoldsTokens => "It still holds tokens. Burn or move them to close it.".into(),
            Blocked::CloseAuthority { authority } => {
                format!("Only {authority} may close it.")
            }
            Blocked::Confidential => {
                "It holds a confidential balance, which this app cannot empty.".into()
            }
            Blocked::MintUnknown => "Its mint could not be read.".into(),
            Blocked::CannotMove(why) | Blocked::CannotBurn(why) => (*why).into(),
            Blocked::ReceiverFrozen => {
                "The receiving wallet's account for this token is frozen, or would start frozen."
                    .into()
            }
            Blocked::ReceiverIsSource => "The receiving account is this account.".into(),
        }
    }
}

/// One account in the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub address: Pubkey,
    pub mint: Pubkey,
    pub outcome: Outcome,
    /// Lamports that come back to the rent address when it is closed: its
    /// rent, plus the unwrapped amount for wrapped SOL. 0 when it stays open.
    pub returned: u64,
}

/// The instructions of one transaction, and the accounts they empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub accounts: Vec<Pubkey>,
    pub instructions: Vec<Instruction>,
}

/// What [`plan`] made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenPlan {
    /// Every account given, in the order given.
    pub accounts: Vec<Planned>,
    pub steps: Vec<Step>,
}

impl TokenPlan {
    /// Lamports that come back to the rent address in all.
    pub fn returned(&self) -> u64 {
        self.accounts.iter().map(|planned| planned.returned).sum()
    }

    /// How many accounts the steps close.
    pub fn closing(&self) -> usize {
        self.accounts
            .iter()
            .filter(|planned| {
                matches!(
                    planned.outcome,
                    Outcome::Closed
                        | Outcome::Unwrapped { .. }
                        | Outcome::Burned { .. }
                        | Outcome::Moved { closed: true, .. }
                )
            })
            .count()
    }

    /// The accounts left as they are, with why.
    pub fn left(&self) -> impl Iterator<Item = (&Planned, &Blocked)> {
        self.accounts
            .iter()
            .filter_map(|planned| match &planned.outcome {
                Outcome::Left(why) => Some((planned, why)),
                _ => None,
            })
    }
}

/// How much one step may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The largest wire, signatures included.
    pub max_wire: usize,
    /// The most compute units the step is estimated to use.
    pub max_units: u32,
    /// The most entries the step is estimated to put in the runtime's
    /// instruction trace, which counts every instruction and every call
    /// one makes to a program, and stops a transaction at 64.
    pub max_trace: usize,
}

impl Limits {
    /// For a wallet whose key signs the step as it is. Room is left for a
    /// durable nonce's advance and for the signatures of other wallets.
    pub const PLAIN: Limits = Limits {
        max_wire: MAX_TRANSACTION_SIZE - 512,
        max_units: 1_000_000,
        max_trace: 56,
    };

    /// For a wallet Sign wraps the step for (programmatic signer, Vector,
    /// Squads), which adds its own accounts and instructions, and for a
    /// Squads vault stores the step inside the transaction that creates it.
    /// Measured on burn-and-close steps: a programmatic signer's wrap is
    /// about 2.6 times the step, and a Squads v4 vault runs out of heap
    /// executing more than about 14 accounts. 1,100 bytes is about 12.
    pub const WRAPPED: Limits = Limits {
        max_wire: 1_100,
        max_units: 500_000,
        max_trace: 32,
    };
}

/// What an instruction is estimated to cost a step: compute units, and
/// entries in the instruction trace, itself and the calls it makes.
#[derive(Clone, Copy, Default)]
struct Cost {
    units: u32,
    trace: usize,
}

// On the high side, so a step stays clear of its limits before Sign
// measures it. Creating an associated token account asks the token program
// for the size, has the System program create it, and initializes it.
const CREATE_ATA: Cost = Cost {
    units: 40_000,
    trace: 7,
};
const TRANSFER: Cost = Cost {
    units: 15_000,
    trace: 1,
};
const BURN: Cost = Cost {
    units: 12_000,
    trace: 1,
};
const CLOSE: Cost = Cost {
    units: 10_000,
    trace: 1,
};
const HARVEST: Cost = Cost {
    units: 12_000,
    trace: 1,
};
const MEMO: Cost = Cost {
    units: 20_000,
    trace: 1,
};

/// The memo put before a transfer into an account that wants one.
pub const MOVE_MEMO: &str = "Moved by Airlock";

/// Who takes part in the steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parties {
    /// The wallet whose token accounts these are. It signs every step.
    pub owner: Pubkey,
    /// Who pays each step's fee.
    pub fee_payer: Pubkey,
    /// Who pays the rent of a receiving account that has to be created: the
    /// fee payer for a plain key, the wallet itself for one Sign wraps.
    pub rent_payer: Pubkey,
    /// Where a closed account's lamports go.
    pub rent_to: Pubkey,
}

/// Plan what happens to each of `items`, token accounts of the owner, and
/// pack what is done into steps within `limits`.
pub fn plan(parties: Parties, items: Vec<(Held, Choice)>, limits: Limits) -> TokenPlan {
    let mut out = TokenPlan::default();
    let mut packer = Packer::new(parties.fee_payer, limits);
    for (held, choice) in items {
        let (outcome, work) = match decide(&parties, &held, &choice) {
            Ok(done) => done,
            Err(why) => (Outcome::Left(why), Work::default()),
        };
        let returned = match &outcome {
            Outcome::Closed
            | Outcome::Unwrapped { .. }
            | Outcome::Burned { .. }
            | Outcome::Moved { closed: true, .. } => held.lamports,
            Outcome::Moved { closed: false, .. } | Outcome::Left(_) => 0,
        };
        if !work.instructions.is_empty() {
            packer.add(held.address, work);
        }
        out.accounts.push(Planned {
            address: held.address,
            mint: held.account.mint,
            outcome,
            returned,
        });
    }
    out.steps = packer.finish();
    out
}

/// The instructions for one account, and what they are estimated to cost.
#[derive(Default)]
struct Work {
    instructions: Vec<Instruction>,
    cost: Cost,
}

impl Work {
    fn push(&mut self, instruction: Instruction, cost: Cost) {
        self.instructions.push(instruction);
        self.cost.units += cost.units;
        self.cost.trace += cost.trace;
    }
}

fn decide(parties: &Parties, held: &Held, choice: &Choice) -> Result<(Outcome, Work), Blocked> {
    let Parties {
        owner,
        rent_payer,
        rent_to,
        ..
    } = *parties;
    let account = &held.account;
    if *choice == Choice::Leave {
        return Err(Blocked::Chosen);
    }
    if account.owner != owner {
        return Err(Blocked::NotOwned {
            owner: account.owner,
        });
    }
    if account.frozen {
        return Err(Blocked::Frozen);
    }
    let can_close = if account.closer() != owner {
        Err(Blocked::CloseAuthority {
            authority: account.closer(),
        })
    } else if account.confidential {
        Err(Blocked::Confidential)
    } else {
        Ok(())
    };
    let mut work = Work::default();

    // Wrapped SOL is unwrapped by closing it, whatever was chosen.
    if is_native_mint(&account.mint) {
        can_close?;
        close(&mut work, held, rent_to, owner);
        return Ok((
            Outcome::Unwrapped {
                lamports: account.amount,
            },
            work,
        ));
    }

    if account.amount == 0 {
        can_close?;
        close(&mut work, held, rent_to, owner);
        return Ok((Outcome::Closed, work));
    }

    match choice {
        Choice::Leave => Err(Blocked::Chosen),
        Choice::Close => Err(Blocked::HoldsTokens),
        Choice::Burn => {
            can_close?;
            let mint = held.mint.as_ref().ok_or(Blocked::MintUnknown)?;
            if let Some(why) = mint.burn_refusal() {
                return Err(Blocked::CannotBurn(why));
            }
            work.push(
                burn_checked(
                    account.program,
                    held.address,
                    account.mint,
                    owner,
                    account.amount,
                    mint.decimals,
                ),
                BURN,
            );
            close(&mut work, held, rent_to, owner);
            Ok((
                Outcome::Burned {
                    amount: account.amount,
                },
                work,
            ))
        }
        Choice::Move(receiver) => {
            let mint = held.mint.as_ref().ok_or(Blocked::MintUnknown)?;
            if let Some(why) = mint.refusal() {
                return Err(Blocked::CannotMove(why));
            }
            let to = associated_token_address(&receiver.owner, &account.mint, &account.program);
            if to == held.address {
                return Err(Blocked::ReceiverIsSource);
            }
            let created = receiver.existing.is_none();
            match &receiver.existing {
                Some(existing) if existing.frozen => return Err(Blocked::ReceiverFrozen),
                None if mint.frozen_by_default => return Err(Blocked::ReceiverFrozen),
                Some(existing) if existing.requires_memo => {
                    work.push(memo(MOVE_MEMO), MEMO);
                }
                Some(_) => {}
                None => work.push(
                    create_ata_idempotent(
                        rent_payer,
                        receiver.owner,
                        account.mint,
                        account.program,
                    ),
                    CREATE_ATA,
                ),
            }
            work.push(
                transfer_checked(
                    account.program,
                    held.address,
                    account.mint,
                    to,
                    owner,
                    account.amount,
                    mint.decimals,
                ),
                TRANSFER,
            );
            let closed = can_close.is_ok();
            if closed {
                close(&mut work, held, rent_to, owner);
            }
            Ok((
                Outcome::Moved {
                    amount: account.amount,
                    to,
                    created,
                    closed,
                },
                work,
            ))
        }
    }
}

/// Close `held`, harvesting its withheld fees to the mint first.
fn close(work: &mut Work, held: &Held, rent_to: Pubkey, owner: Pubkey) {
    let account = &held.account;
    if account.withheld > 0 && account.program == TOKEN_2022_PROGRAM_ID {
        work.push(
            harvest_withheld_to_mint(account.mint, &[held.address]),
            HARVEST,
        );
    }
    work.push(
        close_account(account.program, held.address, rent_to, owner),
        CLOSE,
    );
}

/// Fills steps in order, starting a new one when the next account's work
/// does not fit.
struct Packer {
    fee_payer: Pubkey,
    limits: Limits,
    steps: Vec<Step>,
    current: Step,
    cost: Cost,
}

impl Packer {
    fn new(fee_payer: Pubkey, limits: Limits) -> Self {
        Self {
            fee_payer,
            limits,
            steps: Vec::new(),
            current: Step {
                accounts: Vec::new(),
                instructions: Vec::new(),
            },
            cost: Cost::default(),
        }
    }

    fn add(&mut self, account: Pubkey, work: Work) {
        if !self.current.instructions.is_empty() && !self.fits(&work) {
            let full = std::mem::replace(
                &mut self.current,
                Step {
                    accounts: Vec::new(),
                    instructions: Vec::new(),
                },
            );
            self.steps.push(full);
            self.cost = Cost::default();
        }
        // An account's work goes into a step whole, alone if it must: it is
        // a few instructions, which always fit an empty transaction.
        self.current.accounts.push(account);
        self.current.instructions.extend(work.instructions);
        self.cost.units += work.cost.units;
        self.cost.trace += work.cost.trace;
    }

    fn fits(&self, work: &Work) -> bool {
        let limits = &self.limits;
        if self.cost.units + work.cost.units > limits.max_units
            || self.cost.trace + work.cost.trace > limits.max_trace
        {
            return false;
        }
        let mut instructions = self.current.instructions.clone();
        instructions.extend(work.instructions.iter().cloned());
        // The blockhash does not change the size.
        compile_v1(self.fee_payer, instructions, None, Some(Hash::default()))
            .is_ok_and(|compiled| compiled.wire.len() <= limits.max_wire)
    }

    fn finish(mut self) -> Vec<Step> {
        if !self.current.instructions.is_empty() {
            self.steps.push(self.current);
        }
        self.steps
    }
}

#[cfg(test)]
mod tests;
