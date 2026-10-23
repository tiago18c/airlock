//! Reading a transaction before signing it: what it does in words, who still
//! has to sign, and whether a short prompt is enough.

pub(crate) mod clock;
pub(crate) mod context;
pub(crate) mod explain;
pub(crate) mod quick_sign;
pub(crate) mod signing;

pub use clock::{
    blocks_left, blocks_left_after, format_countdown, read_clock, seconds_left, ClockReading,
    FRESH_BLOCKHASH_BLOCKS, SECONDS_PER_BLOCK,
};
pub use context::{
    context_is_for, context_note, context_note_for, context_note_with, display_note,
};
pub use explain::{
    explain_instructions, explain_transaction, explain_with_note, missing_catalog_programs,
    note_supplies_accounts, AuthorizingKey, ContextPinMatch, DisplayNote, ExplainedIx, ExplainedTx,
    IdlAccountLine, IdlExplanation, IdlField, IdlPinStatus, InnerKind, InnerMessage, NextKey,
    ProgramLine, RunsAs,
};
pub use quick_sign::{quick_sign, quick_sign_in, QuickSign};
pub use signing::{
    next_step, sign_locally, sign_locally_in, sign_locally_in_with, signing_status,
    signing_status_in, tx_lifetime, LocalKeys, LocalSigning, NextSignature, SignerKey, SignerLine,
    SignerRole, SignerState, SigningStatus, TxLifetime,
};
