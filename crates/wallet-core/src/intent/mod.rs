//! Intents: a site's transactions with notes on their accounts, which the
//! wallet resolves however its account signs. `docs/intents.md` is the
//! reference.
//!
//! A request goes through these in order:
//!
//! 1. [`ResolveInput`] and its [`Notes`] read from the site's JSON.
//! 2. [`check_notes`] against the decoded transactions. The node half of
//!    check 3 is in `wallet_flows::intent`.
//! 3. [`signing_profile`] of the account, then [`plan_resolution`]: which
//!    new accounts get a key here, whether the fee payer is replaced, which
//!    signatures the app owes, or what is [`Unsupported`].
//! 4. [`Plan::replaced`] with the keys made and the addresses carried, then
//!    [`apply_to_request`]: the transactions with the replaced addresses
//!    written in.
//! 5. A [`Resolution`], changed as the person decides and the chain
//!    answers. [`tx_role`] names each transaction the wallet made.
//!
//! A message signed as a memo is built by [`memo_transaction`].

mod apply;
mod check;
mod error;
mod memo;
mod notes;
mod plan;
mod profile;
mod resolution;
mod role;
mod text;

#[cfg(test)]
mod tests;

pub use apply::{apply_notes, apply_to_request, replacements, Replacements};
pub use check::check_notes;
pub use error::{NoteError, Requirement, Unsupported};
pub use memo::{
    is_memo_of, memo_format, memo_instruction, memo_text, memo_transaction, MemoError,
    MAX_MEMO_BYTES,
};
pub use notes::{
    valid_id, AccountNote, AccountNotes, DataHolds, DataNote, Delivery, Derivation, Notes, Order,
    Pda, ResolveInput, Seed, SignsBy, WithSeed, MAX_TRANSACTIONS,
};
pub use plan::{carried_transactions, plan_resolution, Plan};
pub use profile::{
    signing_profile, Execution, MessageMode, NewAccountKey, Payer, ProfileError, SigningProfile,
    Timing, MAX_PROFILE_TRANSACTIONS,
};
pub use resolution::{
    Awaiting, Composed, MemoMessage, Ran, Resolution, ResolutionError, ResolutionFailure,
    ResolvedTx, SendResult, State, TxState,
};
pub use role::{proposal_opened, tx_role, TxRole};
