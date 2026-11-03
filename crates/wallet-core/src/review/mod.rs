//! Reading a transaction before signing it: what it does in words, who still
//! has to sign, and whether a short prompt is enough.

pub(crate) mod blind;
pub(crate) mod clock;
pub(crate) mod context;
pub(crate) mod explain;
pub(crate) mod quick_sign;
pub(crate) mod risk;
pub(crate) mod signing;

pub use blind::{blind_in, stored_in, BlindApproval, StoredTarget};
pub use clock::{
    blocks_left, blocks_left_after, format_countdown, read_clock, seconds_left, ClockReading,
    FRESH_BLOCKHASH_BLOCKS, SECONDS_PER_BLOCK,
};
pub use context::{
    carrying_stored, context_is_for, context_note, context_note_for, context_note_with,
    display_note,
};
pub use explain::{
    explain_instructions, explain_transaction, explain_with_note, missing_catalog_programs,
    note_supplies_accounts, programs_of, AuthorizingKey, ContextPinMatch, DisplayNote, ExplainedIx,
    ExplainedTx, IdlAccountLine, IdlExplanation, IdlField, IdlPinStatus, InnerKind, InnerMessage,
    NextKey, ProgramLine, RunsAs,
};
pub use quick_sign::{quick_sign, quick_sign_in, QuickSign};
pub use risk::risks_in;
pub use signing::{
    batch_steps, keys_asked_for, next_step, only_simulate_and_sign, sign_locally, sign_locally_in,
    sign_locally_in_with, sign_steps, signing_status, signing_status_in, tx_lifetime, LocalKeys,
    LocalSigning, NextSignature, SignFinish, SignStep, SignStepInputs, SignStepKind, SignStepState,
    SignerKey, SignerLine, SignerRole, SignerState, SigningStatus, SimulationStage, TxLifetime,
};
