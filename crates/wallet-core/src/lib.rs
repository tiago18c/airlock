//! UI-agnostic Solana wallet domain.
//!
//! Dioxus (or any other front end) should call these types rather than owning
//! address-book rules, wallet records, or transaction building itself. Sealing
//! the vault is in `wallet-vault`, and the node client in `wallet-rpc`.

pub mod book;
mod chain;
mod cluster;
pub mod edit;
mod error;
pub mod explorer;
pub mod idl;
mod ids;
pub mod keys;
mod market;
mod phrase;
pub mod portfolio;
pub mod review;
pub mod spl;
pub mod transport;
pub mod tx;
mod units;
pub mod wallets;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod layers;

// Every group is public, and the root names everything in them, so
// `wallet_core::X` and `wallet_core::<group>::X` are the same item. Modules
// whose functions need their module's name to make sense, such as `stake`
// and `squads`, are reached as `wallet_core::stake::X`. The lists are
// written out, not globbed, so the public surface reads here, and an item
// can move between modules without its root name changing.
pub use book::{
    AddressBook, AddressBookFile, AddressEntry, AddressKind, AddressPlace, BookGroupInfo, BookRow,
    Category, Cluster, Described, Directory, Fact, Group, HoldingSummary, Holdings, HoldingsStore,
    KnownCluster, Origin, TokenFlag, TokenHolding, WalletSort, Workspace, WorkspaceStore,
    DEFAULT_AUTO_LOCK_SECS, DEVNET_GENESIS, LOCAL_GROUP_ID, LOCAL_RPC_URL, MAINNET_BETA_GENESIS,
    TESTNET_GENESIS,
};
#[cfg(not(target_arch = "wasm32"))]
pub use idl::DirIdlBodies;
pub use idl::{
    anchor_idl_account_data, anchor_idl_address, build_idl_instruction, builtin_idl,
    idl_bytes_from_anchor_account, idl_bytes_from_program_metadata, idl_instruction_form,
    idls_from_pack, outline_idl, pinned_program, pretty_idl, program_metadata_account_data,
    program_metadata_idl_address, sha256_bytes, BuiltinIdl, EmbeddedIdl, IdlAccountField,
    IdlAccountSource, IdlAccountUse, IdlArgField, IdlArgKind, IdlArgOutline, IdlBodies,
    IdlBuildFailure, IdlBuildInput, IdlBundleReport, IdlCatalog, IdlChainSource, IdlCheck,
    IdlDraft, IdlError, IdlErrorOutline, IdlForm, IdlIxOutline, IdlNamed, IdlNetwork, IdlOrigin,
    IdlOutline, IdlPackError, IdlPdaOutline, IdlRecord, IdlSource, IdlSpec, IdlStore, MemoryBodies,
    PinnedProgram, BUILTIN_IDLS, ED25519_SIGNER_PROGRAM_ID, MAX_IDL_BYTES, PINNED_PROGRAMS,
};
pub use keys::{
    concrete_path, derive_batch, new_phrase, path_has_wildcard, secret_seed_from_import,
    DerivedAccount, KeypairSigner, WalletSigner, DERIVE_BATCH, SOLANA_PATH_TEMPLATE,
};
pub use phrase::{Part, Phrase};
pub use review::{
    blocks_left, blocks_left_after, context_is_for, context_note, context_note_for,
    context_note_with, display_note, explain_instructions, explain_transaction, explain_with_note,
    format_countdown, missing_catalog_programs, next_step, note_supplies_accounts, quick_sign,
    quick_sign_in, read_clock, seconds_left, sign_locally, sign_locally_in, sign_locally_in_with,
    sign_steps, signing_status, signing_status_in, tx_lifetime, AuthorizingKey, ClockReading,
    ContextPinMatch, DisplayNote, ExplainedIx, ExplainedTx, IdlAccountLine, IdlExplanation,
    IdlField, IdlPinStatus, InnerKind, InnerMessage, LocalKeys, LocalSigning, NextKey,
    NextSignature, ProgramLine, QuickSign, RunsAs, SignFinish, SignStep, SignStepInputs,
    SignStepKind, SignStepState, SignerKey, SignerLine, SignerRole, SignerState, SigningStatus,
    SimulationStage, TxLifetime, FRESH_BLOCKHASH_BLOCKS, SECONDS_PER_BLOCK,
};
pub use transport::{protocol, signature_objects, SignatureObject};
pub use tx::{
    apply_local_signatures, compile_draft, compile_probe, compile_v1, compile_v1_budget,
    decode_transaction, decode_v1_transaction, insert_signature, lookup_tables_needed, onto_nonce,
    parse_lookup_table_addresses, parse_wire, renew, runtime_default_cu_limit, simulate,
    sol_transfer, to_v1, to_v1_swapping, to_v1_with, CompiledTx, ComputeBudgetSpec, ConvertError,
    Converted, DecodedAccount, DecodedSigner, DecodedTx, LimitFrom, Limits, LookupTables,
    NonceSpec, RawInstruction, TxBudget, TxFormat, TxSettings, WireView, LAMPORTS_PER_SIGNATURE,
    LOOKUP_TABLE_META_SIZE,
};
pub use wallets::{
    acting_wallet, adapt, enroll, name_offline_wallet, name_signers, offline_nonce, programmatic,
    proposals, refresh_targets, relay, shareable_accounts, shared_keys, signer_of, signing_route,
    squads, stake, vector, Authority, EnrollError, HardwareVendor, KeyPhrase, NewSigner,
    OfflinePath, Protocol, PublicBytes, RosterError, RouteError, RouteStep, ShareableAccount,
    SigningRoute, Vault, VaultData, VectorSettings, WalletClass, WalletKind, WalletListStore,
    WalletRecord, WalletSummary,
};

pub use chain::{
    parse_nonce_account, ChainSnapshot, ConfirmationLevel, DiscoveredNonce, DiscoveredTokenAccount,
    Lifetime, RawAccount, SignatureStatus, SolanaRpc,
};
pub use codama_dynamic_client::{IntentBody, IntentSpan, IntentValue, ScaleSource};
pub use error::{
    BookFileError, BuildError, DecodeError, DeriveError, RpcError, SignError, VaultError,
};
pub use ids::{
    short_pubkey, WalletId, ASSOCIATED_TOKEN_PROGRAM_ID, COMPUTE_BUDGET_PROGRAM_ID,
    STAKE_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};
pub use solana_instruction::Instruction;
/// Limits and sizes of a Solana v1 transaction, from the crates that define them.
pub use solana_message::v1::{MAX_TRANSACTION_SIZE, V1_PREFIX};
pub use solana_signature::SIGNATURE_BYTES;
pub use units::{format_sol, format_token_amount, parse_amount, LAMPORTS_PER_SOL};
pub use wallet_vault::{LoadProblem, MemoryStore, SecretBytes, SecretString, VaultStore};

#[cfg(not(target_arch = "wasm32"))]
pub use wallet_vault::FileStore;
