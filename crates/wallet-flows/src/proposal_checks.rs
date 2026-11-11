//! Adding the Squads clear signing program's checks to v4 votes.
//!
//! A vote signs only the address of a proposal, so a device with no network
//! cannot see what it says yes to. Rebuilt here with a `verifyProposal` for
//! each voting member (a `verifyProposalCpi` for one that signs through a
//! programmatic signer or a Vector wallet), and a `verifyBufferHash` for the
//! build each program upgrade deploys, the transaction carries every
//! instruction the proposal stores. The chain refuses a vote unless the
//! proposal holds exactly that, so the device that signs reads what it
//! approves from the transaction alone (`wallet_core::checked_in`).
//!
//! Which shapes are built is the deployed program's to say
//! (`wallet_core::squads::ClearSigningAccepts`), which the caller passes:
//! today one member's vote, signed with its own key or through a
//! programmatic signer or a Vector wallet. The rest, an execution, several
//! members, a durable nonce, are built the same way once a program takes
//! them. A vote through a programmatic signer or a Vector wallet is rebuilt
//! for that wallet again around the checks, from the instructions as they
//! were before its first rebuild (`wallet_core::plain_approval`), and the
//! checks move into its `Submit` or `Passthrough` beside the vote.
//!
//! What is carried is read from the node again here, not taken from what a
//! screen or a context note showed, and checked the way
//! [`crate::squads::read_stored`] checks it. A buffer is hashed whole, which
//! for a program is megabytes, so a caller with a window runs
//! [`CheckPlan::hash`] away from it.

use crate::error::FlowError;
use crate::simulate::measure;
use solana_pubkey::Pubkey;
use wallet_core::adapt::{AdaptError, AdaptPlan, Adapted};
use wallet_core::program_code::{loader_state, LoaderState, BPF_LOADER_UPGRADEABLE_ID};
use wallet_core::relay::{fit_compute_units, fit_loaded_accounts, WrapError};
use wallet_core::squads::{
    compile_with_checks, upgrade_buffers, BufferHashCheck, ClearSigningAccepts, ShapeRefused,
    StoredAccount, VaultTransaction,
};
use wallet_core::{
    checked_in, checked_instructions, decode_v1_transaction, explain_transaction, plain_approval,
    BuildError, CheckedApproval, DecodedTx, Instruction, NoIdls, Phrase, PlainApproval, SolanaRpc,
    MAX_TRANSACTION_SIZE,
};

pub const NOT_VOTES: &str = "Only a transaction of Squads v4 approvals of one proposal, and its execution, with nothing else beside them, can carry what the proposal holds.";
pub const PROPOSAL_UNREAD: &str =
    "The proposal could not be read on this cluster, so what it holds cannot be added.";
pub const STORED_UNREAD: &str = "The transaction this proposal stores could not be read on this cluster, so what it holds cannot be added.";
pub const STORED_LOOKUP_TABLES: &str = "The transaction this proposal stores reads accounts from lookup tables, which this app does not read, so what it holds cannot be added.";
pub const STORED_OTHER: &str = "This proposal stores a change to the multisig or a batch, not a vault transaction, so what it holds cannot be added.";
pub const STORED_CHANGED: &str = "What the proposal stores is not what this screen shows, so nothing was added. Load the transaction again to read it anew.";
pub const NOT_REBUILT: &str = "A member votes through a wallet without a key, and this transaction could not be rebuilt for that wallet around what the proposal holds, so nothing was added.";
pub const NOT_CHECKED: &str =
    "The transaction built does not read as checked, so nothing was added.";

/// Why the program on this cluster does not take the checks beside what the
/// transaction holds.
pub fn refused_shape(refused: ShapeRefused) -> String {
    let beside = match refused {
        ShapeRefused::DurableNonce => "a durable nonce",
        ShapeRefused::Execute => "an execution of the proposal",
        ShapeRefused::SeveralVotes => "several members' votes",
        ShapeRefused::InSubmit => "a vote through a programmatic signer's authorization",
        ShapeRefused::InPassthrough => "a vote through a Vector wallet",
    };
    format!("The clear signing program on this cluster does not take what the proposal holds beside {beside}, so nothing was added.")
}

/// What the node holds for the votes, read and checked, before any buffer
/// is hashed.
#[derive(Clone, Debug)]
pub struct CheckPlan {
    tx: DecodedTx,
    plain: PlainApproval,
    /// The vault transaction the proposal stores, which every check carries.
    pub stored: VaultTransaction,
    /// Each upgrade's buffer and what its account holds, checked to be a
    /// buffer of the upgradeable loader.
    buffers: Vec<(Pubkey, Vec<u8>)>,
}

/// A [`CheckPlan`] with each buffer hashed.
#[derive(Clone, Debug)]
pub struct HashedPlan {
    tx: DecodedTx,
    plain: PlainApproval,
    stored: VaultTransaction,
    buffers: Vec<BufferHashCheck>,
}

impl CheckPlan {
    /// Hash every buffer. A program's build is megabytes: a window runs this
    /// on another thread.
    pub fn hash(self) -> HashedPlan {
        let buffers = self
            .buffers
            .iter()
            .filter_map(|(buffer, data)| {
                // Checked to be a buffer when it was read.
                let hash = wallet_core::program_code::buffer_hash(data)?;
                Some(BufferHashCheck::new(*buffer, &hash))
            })
            .collect();
        HashedPlan {
            tx: self.tx,
            plain: self.plain,
            stored: self.stored,
            buffers,
        }
    }

    /// A member votes through a wallet without a key, so the transaction is
    /// rebuilt for it again around the checks: from [`Self::plain_tx`], with
    /// the plan the caller makes for that.
    pub fn wrapped(&self) -> bool {
        self.plain.shape.wrapped()
    }

    /// The transaction as it was before a keyless wallet's rebuild, without
    /// checks: what the caller plans that rebuild from. It names the same
    /// signers as the transaction with checks.
    pub fn plain_tx(&self) -> Result<DecodedTx, FlowError> {
        let fee_payer = self
            .tx
            .fee_payer()
            .ok_or_else(|| FlowError::refused(NOT_CHECKED))?;
        let wire = compile_with_checks(
            fee_payer,
            &self.tx.budget,
            &with_nonce(&self.plain, self.plain.instructions.clone()),
            self.tx.lifetime,
        )
        .map_err(FlowError::refused)?;
        decode_v1_transaction(&wire).map_err(FlowError::refused)
    }
}

fn with_nonce(plain: &PlainApproval, rest: Vec<Instruction>) -> Vec<Instruction> {
    plain.nonce.clone().into_iter().chain(rest).collect()
}

/// The votes with their checks, ready for the Sign screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithChecks {
    pub wire: Vec<u8>,
    /// The height at which its blockhash expires. `None` on a durable
    /// nonce, which does not.
    pub last_valid_height: Option<u64>,
    /// What `checked_in` reads in it: every vote and the execution pinned.
    pub approval: CheckedApproval,
    /// Signatures the transaction held, which no longer apply.
    pub voided: usize,
    /// The rebuild for a keyless wallet it went through again, when a member
    /// votes through one: what signing for it needs.
    pub adapted: Option<Adapted>,
}

/// Read what `tx`, votes on one v4 proposal, says yes to, and the buffer of
/// each program upgrade in it. `shown` is what the screen showed as stored
/// behind it: when what the node holds now is not that, nothing is added.
pub async fn read_for_checks(
    rpc: &dyn SolanaRpc,
    tx: &DecodedTx,
    shown: Option<&VaultTransaction>,
) -> Result<CheckPlan, FlowError> {
    let refused = |text: &str| FlowError::Refused(text.into());
    let plain = plain_approval(tx).ok_or_else(|| refused(NOT_VOTES))?;
    let squad = plain.shape.squad;
    let program = squad.program.program_id();
    // The proposal says which transaction it is about: the one derived for
    // its number, which has to be the proposal derived for it too.
    let proposal = rpc
        .get_account(&plain.shape.proposal)
        .await?
        .filter(|account| account.owner == program)
        .and_then(|account| squad.parse_proposal(&account.data))
        .filter(|proposal| squad.proposal(proposal.index) == plain.shape.proposal)
        .ok_or_else(|| refused(PROPOSAL_UNREAD))?;
    let address = squad.transaction(proposal.index);
    let account = rpc
        .get_account(&address)
        .await?
        .filter(|account| account.owner == program)
        .ok_or_else(|| refused(STORED_UNREAD))?;
    let stored = match squad.parse_transaction(&account.data) {
        Some(StoredAccount::Vault(stored)) if stored.index == proposal.index => stored,
        Some(StoredAccount::LookupTables) => return Err(refused(STORED_LOOKUP_TABLES)),
        Some(StoredAccount::Other) => return Err(refused(STORED_OTHER)),
        _ => return Err(refused(STORED_UNREAD)),
    };
    if shown.is_some_and(|shown| *shown != stored) {
        return Err(refused(STORED_CHANGED));
    }
    let mut buffers = Vec::new();
    for buffer in upgrade_buffers(&stored.instructions) {
        let account = rpc
            .get_account(&buffer)
            .await?
            .filter(|account| account.owner == BPF_LOADER_UPGRADEABLE_ID)
            .filter(|account| {
                matches!(
                    loader_state(&account.owner, &account.data),
                    Some(LoaderState::Buffer { .. })
                )
            })
            .ok_or_else(|| FlowError::Refused(unread_buffer(&buffer)))?;
        buffers.push((buffer, account.data));
    }
    Ok(CheckPlan {
        tx: tx.clone(),
        plain,
        stored,
        buffers,
    })
}

/// Why a buffer stops the rebuild.
pub fn unread_buffer(buffer: &Pubkey) -> String {
    format!(
        "The buffer {buffer} that the proposal's program upgrade deploys from could not be read as a buffer, so its hash cannot be added. The proposal could not run as it stands either."
    )
}

/// Build the votes of `plan` with their checks, when `accepts` says the
/// program takes the shape, and set both limits from what the node says the
/// result uses, plus the usual margin.
///
/// On a recent blockhash it moves to a fresh one; on a durable nonce it
/// stays on it, the advance first. A member that votes through a keyless
/// wallet is rebuilt for it again with `rebuild`, the plan for
/// [`CheckPlan::plain_tx`]. Refused when it does not fit, when the node says
/// it would fail, or when what was built does not read with every vote and
/// the execution pinned and every upgrade's buffer hashed.
pub async fn build_with_checks(
    rpc: &dyn SolanaRpc,
    plan: HashedPlan,
    accepts: ClearSigningAccepts,
    rebuild: Option<&AdaptPlan>,
) -> Result<WithChecks, FlowError> {
    let HashedPlan {
        tx,
        plain,
        stored,
        buffers,
    } = plan;
    let instructions = checked_instructions(&plain, &stored, &buffers, accepts)
        .map_err(|refused| FlowError::Refused(refused_shape(refused)))?;
    let fee_payer = tx
        .fee_payer()
        .ok_or_else(|| FlowError::refused(NOT_CHECKED))?;
    let size = |err: BuildError| match err {
        BuildError::TooLarge { len } => FlowError::Refused(too_large(len)),
        other => FlowError::refused(other),
    };
    let (wire, last_valid_height, adapted) = if plain.shape.wrapped() {
        let rebuild = rebuild.ok_or_else(|| FlowError::refused(NOT_REBUILT))?;
        let unwrapped =
            compile_with_checks(fee_payer, &tx.budget, &instructions, tx.lifetime).map_err(size)?;
        let unwrapped = decode_v1_transaction(&unwrapped).map_err(FlowError::refused)?;
        // Reads the wallets' accounts, moves to a fresh blockhash and fits
        // the limits, as the first rebuild did. The `Submit` or the
        // `Advance` makes it larger, so it may not fit where the plain list
        // did.
        let (adapted, height) =
            crate::adapt::rebuild_or(rpc, &unwrapped, rebuild, |err| match err {
                AdaptError::Build(BuildError::TooLarge { len })
                | AdaptError::Wrap(WrapError::Build(BuildError::TooLarge { len })) => {
                    FlowError::Refused(too_large(len))
                }
                other => FlowError::refused(other),
            })
            .await?;
        (adapted.wire.clone(), Some(height), Some(adapted))
    } else {
        let (lifetime, height) = if plain.nonce.is_some() {
            (tx.lifetime, None)
        } else {
            let (blockhash, height) = rpc.get_latest_blockhash_with_height().await?;
            (blockhash, Some(height))
        };
        let probe =
            compile_with_checks(fee_payer, &tx.budget, &instructions, lifetime).map_err(size)?;
        let probed = decode_v1_transaction(&probe).map_err(FlowError::refused)?;
        // Unsigned: a node simulates without checking signatures.
        let simulation = measure(rpc, &probed).await?;
        if let Some(failure) = &simulation.error {
            let at = failure
                .instruction
                .map(|index| format!("instruction {}: ", index + 1))
                .unwrap_or_default();
            return Err(FlowError::Refused(format!(
                "The node says the approval with these instructions would fail ({at}{}), so nothing was added.",
                failure.message
            )));
        }
        let mut wire = probe;
        if let Some(units) = simulation.units_consumed {
            wire = fit_compute_units(&wire, units).map_err(FlowError::refused)?;
        }
        if let Some(size) = simulation.loaded_accounts_data_size {
            wire = fit_loaded_accounts(&wire, size).map_err(FlowError::refused)?;
        }
        (wire, height, None)
    };
    // Shown only if it is what it says: the chain checks every vote here and
    // the execution, and the hash of every upgrade's buffer.
    let built = decode_v1_transaction(&wire).map_err(FlowError::refused)?;
    let shape = &plain.shape;
    let approval = checked_in(&explain_transaction(&NoIdls, &built))
        .into_iter()
        .find(|found| {
            found.proposal == shape.proposal
                && found.squad == shape.squad
                && shape
                    .votes
                    .iter()
                    .all(|vote| found.pins_vote_of(&vote.member))
                && (!shape.execute || found.execute.is_some())
                && found.upgrades.iter().all(|upgrade| upgrade.hash.is_some())
        })
        .ok_or_else(|| FlowError::refused(NOT_CHECKED))?;
    Ok(WithChecks {
        wire,
        last_valid_height,
        approval,
        voided: tx.signers.iter().filter(|slot| slot.filled).count(),
        adapted,
    })
}

/// [`read_for_checks`], [`CheckPlan::hash`] and [`build_with_checks`] in one,
/// hashing where it runs. `plan_rebuild` gives the plan for a keyless wallet
/// a member votes through, from [`CheckPlan::plain_tx`].
pub async fn add_checks(
    rpc: &dyn SolanaRpc,
    tx: &DecodedTx,
    shown: Option<&VaultTransaction>,
    accepts: ClearSigningAccepts,
    plan_rebuild: impl Fn(&DecodedTx) -> Result<Option<AdaptPlan>, FlowError>,
) -> Result<WithChecks, FlowError> {
    let plan = read_for_checks(rpc, tx, shown).await?;
    let rebuild = if plan.wrapped() {
        plan_rebuild(&plan.plain_tx()?)?
    } else {
        None
    };
    build_with_checks(rpc, plan.hash(), accepts, rebuild.as_ref()).await
}

fn too_large(len: usize) -> String {
    format!(
        "The proposal holds too much to carry in the approval: it would take {len} of {MAX_TRANSACTION_SIZE} bytes. The transaction is left as it was."
    )
}

/// What was added, for the note above the transaction.
pub fn checks_note(with: &WithChecks) -> Phrase {
    let approval = &with.approval;
    let carries = match (approval.votes.len(), approval.execute.is_some()) {
        (0 | 1, false) => "the approval now carries every instruction the proposal stores, and the chain refuses the vote unless the proposal holds exactly those.".to_string(),
        (0 | 1, true) => "the approval and the execution now carry every instruction the proposal stores, and the chain refuses both unless the proposal holds exactly those.".to_string(),
        (n, false) => format!("the {n} approvals now carry every instruction the proposal stores, and the chain refuses each vote unless the proposal holds exactly those."),
        (n, true) => format!("the {n} approvals and the execution now carry every instruction the proposal stores, and the chain refuses each of them unless the proposal holds exactly those."),
    };
    let mut note = Phrase::default()
        .t(format!(
            "Added what proposal #{} of multisig ",
            approval.transaction_index
        ))
        .a(approval.squad.address)
        .t(format!(" holds: {carries}"));
    for upgrade in &approval.upgrades {
        note = note
            .t(" It carries the hash of buffer ")
            .a(upgrade.buffer)
            .t(", which program ")
            .a(upgrade.program)
            .t(" is upgraded from, too.");
    }
    note = note.t(if approval.votes.len() > 1 || approval.execute.is_some() {
        " This costs more compute than the transaction without them."
    } else {
        " This costs more compute than the vote alone."
    });
    if with.adapted.is_some() {
        note = note.t(" It was rebuilt again for the wallet without a key that votes.");
    }
    if with.voided > 0 {
        note = note.t(format!(
            " {} signature(s) made over it earlier were cleared and have to be made again.",
            with.voided
        ));
    }
    note.t(" Read it again before signing.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, FakeRpc};
    use solana_instruction::AccountMeta;
    use wallet_core::program_code::buffer_hash;
    use wallet_core::simulate::SimulationReply;
    use wallet_core::squads::{ProposalCheck, Squad, SquadsProgram};
    use wallet_core::{compile_v1, RawAccount, SYSTEM_PROGRAM_ID};

    const DEPLOYED: ClearSigningAccepts = ClearSigningAccepts::DEPLOYED;
    const EVERYTHING: ClearSigningAccepts = ClearSigningAccepts::EVERYTHING;

    fn no_rebuild(_: &DecodedTx) -> Result<Option<AdaptPlan>, FlowError> {
        Ok(None)
    }

    /// The v4 tags of the two accounts written here.
    const VAULT_TRANSACTION: [u8; 8] = [0xa8, 0xfa, 0xa2, 0x64, 0x51, 0x0e, 0xa2, 0xcf];
    const PROPOSAL: [u8; 8] = [0x1a, 0x5e, 0xbd, 0xbb, 0x74, 0x88, 0x35, 0x21];

    fn squad() -> Squad {
        Squad::v4(&pk(40))
    }

    fn member() -> Pubkey {
        pk(9)
    }

    fn payer() -> Pubkey {
        pk(8)
    }

    fn owned_by(owner: Pubkey, data: Vec<u8>) -> RawAccount {
        RawAccount {
            lamports: 1,
            data,
            owner,
            executable: false,
            rent_epoch: 0,
        }
    }

    /// Proposal `index` as v4 writes it, open for votes.
    fn proposal_account(index: u64) -> Vec<u8> {
        let mut data = PROPOSAL.to_vec();
        data.extend_from_slice(squad().address.as_ref());
        data.extend_from_slice(&index.to_le_bytes());
        data.push(1); // Active
        data.extend_from_slice(&0i64.to_le_bytes());
        data.push(255); // bump
        data.extend_from_slice(&0u32.to_le_bytes()); // approved
        data.extend_from_slice(&0u32.to_le_bytes()); // rejected
        data.extend_from_slice(&0u32.to_le_bytes()); // cancelled
        data
    }

    /// Vault transaction `index` as v4 stores it: the vault the only signer,
    /// then the writable accounts, then the rest, programs among them.
    fn stored_account(index: u64, instructions: &[Instruction], lookups: bool) -> Vec<u8> {
        let vault = squad().vault(0);
        let mut keys = vec![vault];
        let mut writable = Vec::new();
        let mut readonly = Vec::new();
        for instruction in instructions {
            for meta in &instruction.accounts {
                if meta.pubkey == vault || keys.contains(&meta.pubkey) {
                    continue;
                }
                let list = if meta.is_writable {
                    &mut writable
                } else {
                    &mut readonly
                };
                if !list.contains(&meta.pubkey) {
                    list.push(meta.pubkey);
                }
            }
            if !readonly.contains(&instruction.program_id) {
                readonly.push(instruction.program_id);
            }
        }
        readonly.retain(|key| !writable.contains(key));
        let writable_count = writable.len();
        keys.extend(writable);
        keys.extend(readonly);
        let at = |key: &Pubkey| keys.iter().position(|k| k == key).unwrap() as u8;

        let mut data = VAULT_TRANSACTION.to_vec();
        data.extend_from_slice(squad().address.as_ref());
        data.extend_from_slice(pk(9).as_ref()); // creator
        data.extend_from_slice(&index.to_le_bytes());
        data.extend_from_slice(&[255, 0, 255]); // bump, vault index, vault bump
        data.extend_from_slice(&0u32.to_le_bytes()); // ephemeral signers
        data.extend_from_slice(&[1, 1, writable_count as u8]);
        data.extend_from_slice(&(keys.len() as u32).to_le_bytes());
        for key in &keys {
            data.extend_from_slice(key.as_ref());
        }
        data.extend_from_slice(&(instructions.len() as u32).to_le_bytes());
        for instruction in instructions {
            data.push(at(&instruction.program_id));
            data.extend_from_slice(&(instruction.accounts.len() as u32).to_le_bytes());
            for meta in &instruction.accounts {
                data.push(at(&meta.pubkey));
            }
            data.extend_from_slice(&(instruction.data.len() as u32).to_le_bytes());
            data.extend_from_slice(&instruction.data);
        }
        if lookups {
            data.extend_from_slice(&1u32.to_le_bytes());
            data.extend_from_slice(pk(77).as_ref());
            data.extend_from_slice(&1u32.to_le_bytes());
            data.push(0);
            data.extend_from_slice(&0u32.to_le_bytes());
        } else {
            data.extend_from_slice(&0u32.to_le_bytes());
        }
        data
    }

    fn payment() -> Instruction {
        crate::testing::transfer_instruction(squad().vault(0), pk(2))
    }

    fn upgrade(buffer: Pubkey) -> Instruction {
        Instruction {
            program_id: BPF_LOADER_UPGRADEABLE_ID,
            accounts: vec![
                AccountMeta::new(pk(60), false),
                AccountMeta::new(pk(71), false),
                AccountMeta::new(buffer, false),
                AccountMeta::new(pk(61), false),
                AccountMeta::new_readonly(pk(62), false),
                AccountMeta::new_readonly(pk(63), false),
                AccountMeta::new_readonly(squad().vault(0), true),
            ],
            data: 3u32.to_le_bytes().to_vec(),
        }
    }

    fn buffer_account(code: &[u8]) -> RawAccount {
        let mut data = vec![1, 0, 0, 0, 1];
        data.extend_from_slice(squad().vault(0).as_ref());
        data.extend_from_slice(code);
        data.extend_from_slice(&[0; 64]);
        owned_by(BPF_LOADER_UPGRADEABLE_ID, data)
    }

    /// A node holding proposal 3 and its stored `instructions`, answering a
    /// simulation that succeeds.
    fn node(instructions: &[Instruction]) -> FakeRpc {
        let program = SquadsProgram::V4.program_id();
        FakeRpc {
            simulation: Some(SimulationReply {
                units_consumed: Some(40_000),
                loaded_accounts_data_size: Some(300_000),
                ..SimulationReply::default()
            }),
            ..FakeRpc::default()
        }
        .with_account(squad().proposal(3), owned_by(program, proposal_account(3)))
        .with_account(
            squad().transaction(3),
            owned_by(program, stored_account(3, instructions, false)),
        )
    }

    /// The approval of proposal 3 by the member, paid by another key.
    fn approval() -> DecodedTx {
        let wire = compile_v1(
            payer(),
            vec![squad().approve(3, &member())],
            None,
            Some(solana_hash::Hash::new_from_array([1; 32])),
        )
        .unwrap()
        .wire;
        decode_v1_transaction(&wire).unwrap()
    }

    fn stored(rpc: &FakeRpc) -> VaultTransaction {
        let data = &rpc.accounts[&squad().transaction(3)].data;
        match squad().parse_transaction(data) {
            Some(StoredAccount::Vault(stored)) => stored,
            other => panic!("{other:?}"),
        }
    }

    fn add(rpc: &FakeRpc, tx: &DecodedTx) -> Result<WithChecks, FlowError> {
        let shown = stored(rpc);
        pollster::block_on(add_checks(rpc, tx, Some(&shown), DEPLOYED, no_rebuild))
    }

    fn refusal(result: Result<WithChecks, FlowError>) -> String {
        match result {
            Err(FlowError::Refused(text)) => text,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_payment_proposal_is_carried_in_the_approval() {
        let rpc = node(&[payment()]);
        let with = add(&rpc, &approval()).unwrap();
        let tx = decode_v1_transaction(&with.wire).unwrap();
        let native = tx.native_instructions();
        assert_eq!(native.len(), 2);
        let check = ProposalCheck::parse(&native[0]).unwrap();
        assert_eq!(check.instructions, vec![payment()]);
        assert_eq!(check.instructions, stored(&rpc).instructions);
        assert_eq!((check.member, check.transaction_index), (member(), 3));
        // The same vote, by the same member, paid by the same key.
        assert_eq!(native[1], squad().approve(3, &member()));
        assert_eq!(tx.fee_payer(), Some(payer()));
        // Limits from what the node used, plus 10%.
        assert_eq!(tx.budget.compute_unit_limit, Some(44_000));
        assert_eq!(tx.budget.loaded_accounts_data_size_limit, Some(330_000));
        assert_eq!(with.last_valid_height, Some(150));
        assert_eq!(with.approval.proposal, squad().proposal(3));
        assert_eq!(with.voided, 0);
        assert!(!tx.signers.iter().any(|slot| slot.filled));
        assert_eq!(
            checks_note(&with).plain(),
            format!(
                "Added what proposal #3 of multisig {} holds: the approval now carries every instruction the proposal stores, and the chain refuses the vote unless the proposal holds exactly those. This costs more compute than the vote alone. Read it again before signing.",
                wallet_core::short_pubkey(&squad().address)
            )
        );
    }

    #[test]
    fn an_upgrade_carries_the_hash_of_its_buffer_first() {
        let code = b"\x7fELF a build";
        let rpc = node(&[upgrade(pk(70)), payment()]).with_account(pk(70), buffer_account(code));
        let with = add(&rpc, &approval()).unwrap();
        let tx = decode_v1_transaction(&with.wire).unwrap();
        let native = tx.native_instructions();
        assert_eq!(native.len(), 3);
        let buffer = BufferHashCheck::parse(&native[0]).unwrap();
        let expected = buffer_hash(&rpc.accounts[&pk(70)].data).unwrap();
        assert_eq!(buffer, BufferHashCheck::new(pk(70), &expected));
        assert!(ProposalCheck::parse(&native[1]).is_some());
        assert_eq!(with.approval.upgrades.len(), 1);
        assert_eq!(with.approval.upgrades[0].hash, Some(hex::encode(expected)));
        assert!(checks_note(&with)
            .plain()
            .contains("It carries the hash of buffer"));
    }

    #[test]
    fn a_buffer_that_is_missing_or_not_a_buffer_stops_it() {
        let rpc = node(&[upgrade(pk(70))]);
        assert_eq!(refusal(add(&rpc, &approval())), unread_buffer(&pk(70)));
        let mut program_data = buffer_account(b"code");
        program_data.data[0] = 3;
        let rpc = node(&[upgrade(pk(70))]).with_account(pk(70), program_data);
        assert_eq!(refusal(add(&rpc, &approval())), unread_buffer(&pk(70)));
        let mut elsewhere = buffer_account(b"code");
        elsewhere.owner = pk(5);
        let rpc = node(&[upgrade(pk(70))]).with_account(pk(70), elsewhere);
        assert_eq!(refusal(add(&rpc, &approval())), unread_buffer(&pk(70)));
    }

    #[test]
    fn what_the_node_holds_has_to_be_what_the_screen_showed() {
        let rpc = node(&[payment()]);
        let mut shown = stored(&rpc);
        shown.instructions.push(payment());
        let refused = pollster::block_on(add_checks(
            &rpc,
            &approval(),
            Some(&shown),
            DEPLOYED,
            no_rebuild,
        ));
        assert_eq!(refusal(refused), STORED_CHANGED);
        // Nothing stored, or the proposal gone.
        let mut rpc = node(&[payment()]);
        rpc.accounts.remove(&squad().transaction(3));
        let refused = pollster::block_on(add_checks(&rpc, &approval(), None, DEPLOYED, no_rebuild));
        assert_eq!(refusal(refused), STORED_UNREAD);
        let mut rpc = node(&[payment()]);
        rpc.accounts.remove(&squad().proposal(3));
        let refused = pollster::block_on(add_checks(&rpc, &approval(), None, DEPLOYED, no_rebuild));
        assert_eq!(refusal(refused), PROPOSAL_UNREAD);
    }

    #[test]
    fn accounts_another_program_holds_are_not_taken() {
        for address in [squad().proposal(3), squad().transaction(3)] {
            let mut rpc = node(&[payment()]);
            rpc.accounts.get_mut(&address).unwrap().owner = pk(5);
            let refused =
                pollster::block_on(add_checks(&rpc, &approval(), None, DEPLOYED, no_rebuild));
            assert!(
                matches!(refused, Err(FlowError::Refused(_))),
                "{address}: {refused:?}"
            );
        }
        // A proposal of another number at the address voted on.
        let mut rpc = node(&[payment()]);
        let program = SquadsProgram::V4.program_id();
        rpc.accounts
            .insert(squad().proposal(3), owned_by(program, proposal_account(4)));
        let refused = pollster::block_on(add_checks(&rpc, &approval(), None, DEPLOYED, no_rebuild));
        assert_eq!(refusal(refused), PROPOSAL_UNREAD);
    }

    #[test]
    fn a_stored_message_with_lookup_tables_is_not_carried() {
        let program = SquadsProgram::V4.program_id();
        let rpc = node(&[payment()]).with_account(
            squad().transaction(3),
            owned_by(program, stored_account(3, &[payment()], true)),
        );
        let refused = pollster::block_on(add_checks(&rpc, &approval(), None, DEPLOYED, no_rebuild));
        assert_eq!(refusal(refused), STORED_LOOKUP_TABLES);
    }

    #[test]
    fn a_proposal_too_large_to_carry_leaves_the_transaction_as_it_was() {
        let big = Instruction {
            program_id: pk(30),
            accounts: vec![AccountMeta::new(squad().vault(0), true)],
            data: vec![7; 3_000],
        };
        let rpc = node(&[big.clone(), big]);
        let refused = refusal(add(&rpc, &approval()));
        assert!(
            refused.starts_with("The proposal holds too much to carry in the approval"),
            "{refused}"
        );
    }

    fn on_nonce(instructions: Vec<Instruction>) -> DecodedTx {
        let nonce = wallet_core::NonceSpec {
            account: pk(30),
            authority: payer(),
            hash: solana_hash::Hash::new_from_array([3; 32]),
        };
        let wire = compile_v1(payer(), instructions, Some(nonce), None)
            .unwrap()
            .wire;
        decode_v1_transaction(&wire).unwrap()
    }

    fn on_blockhash(instructions: Vec<Instruction>) -> DecodedTx {
        let wire = compile_v1(
            payer(),
            instructions,
            None,
            Some(solana_hash::Hash::new_from_array([1; 32])),
        )
        .unwrap()
        .wire;
        decode_v1_transaction(&wire).unwrap()
    }

    fn add_with(
        rpc: &FakeRpc,
        tx: &DecodedTx,
        accepts: ClearSigningAccepts,
    ) -> Result<WithChecks, FlowError> {
        pollster::block_on(add_checks(rpc, tx, None, accepts, no_rebuild))
    }

    #[test]
    fn only_what_the_deployed_program_takes_is_rebuilt() {
        let rpc = node(&[payment()]);
        let with_payment = on_blockhash(vec![
            squad().approve(3, &member()),
            crate::testing::transfer_instruction(payer(), pk(2)),
        ]);
        assert_eq!(refusal(add(&rpc, &with_payment)), NOT_VOTES);
        let tx = on_nonce(vec![squad().approve(3, &member())]);
        assert_eq!(tx.instructions[0].program_id, SYSTEM_PROGRAM_ID);
        assert_eq!(
            refusal(add(&rpc, &tx)),
            refused_shape(ShapeRefused::DurableNonce)
        );
        let execute = squad().execute(3, 0, &[payment()], &member());
        let tx = on_blockhash(vec![squad().approve(3, &member()), execute]);
        assert_eq!(
            refusal(add_with(&rpc, &tx, DEPLOYED)),
            refused_shape(ShapeRefused::Execute)
        );
        let together = on_blockhash(vec![
            squad().approve(3, &member()),
            squad().approve(3, &pk(10)),
        ]);
        assert_eq!(
            refusal(add_with(&rpc, &together, DEPLOYED)),
            refused_shape(ShapeRefused::SeveralVotes)
        );
    }

    #[test]
    fn approving_together_and_executing_is_built_once_the_program_takes_it() {
        let rpc = node(&[payment()]);
        let execute = squad().execute(3, 0, &[payment()], &member());
        let tx = on_blockhash(vec![
            squad().approve(3, &member()),
            squad().approve(3, &pk(10)),
            execute.clone(),
        ]);
        let with = add_with(&rpc, &tx, EVERYTHING).unwrap();
        assert_eq!(with.approval.votes.len(), 2);
        assert!(with.approval.execute.is_some());
        assert_eq!(with.adapted, None);
        let native = decode_v1_transaction(&with.wire)
            .unwrap()
            .native_instructions();
        let members: Vec<Pubkey> = native
            .iter()
            .filter_map(ProposalCheck::parse)
            .map(|check| check.member)
            .collect();
        assert_eq!(members, [member(), pk(10)]);
        // The execution last, as it was.
        let last = native.last().unwrap();
        assert_eq!(
            (last.program_id, &last.data),
            (execute.program_id, &execute.data)
        );
        assert!(checks_note(&with)
            .plain()
            .contains("the 2 approvals and the execution now carry every instruction"));
    }

    #[test]
    fn a_durable_nonce_is_kept_once_the_program_takes_it() {
        let rpc = node(&[payment()]);
        let tx = on_nonce(vec![squad().approve(3, &member())]);
        let with = add_with(&rpc, &tx, EVERYTHING).unwrap();
        let built = decode_v1_transaction(&with.wire).unwrap();
        assert!(built.uses_durable_nonce);
        assert_eq!(built.lifetime, tx.lifetime);
        assert_eq!(built.native_instructions()[0], tx.native_instructions()[0]);
        // A nonce does not expire with a blockhash.
        assert_eq!(with.last_valid_height, None);
    }

    #[test]
    fn a_vote_through_a_keyless_wallet_needs_its_rebuild_plan() {
        use wallet_core::programmatic::{programmatic_signer_address, ExecutorNonce};
        let authority = pk(20);
        let member = programmatic_signer_address(&authority);
        let plain = compile_v1(
            payer(),
            vec![squad().approve(3, &member)],
            None,
            Some(solana_hash::Hash::new_from_array([1; 32])),
        )
        .unwrap()
        .wire;
        let wrapped = wallet_core::relay::wrap_for_programmatic(
            &decode_v1_transaction(&plain).unwrap(),
            &[authority],
            ExecutorNonce {
                account: pk(22),
                authority: member,
                value: solana_hash::Hash::new_from_array([3; 32]),
            },
            payer(),
            solana_hash::Hash::new_from_array([9; 32]),
        )
        .unwrap();
        let tx = decode_v1_transaction(&wrapped).unwrap();
        let rpc = node(&[payment()]);
        // A program that took no check inside a `Submit` would say so.
        let top_only = ClearSigningAccepts {
            in_submit: false,
            ..DEPLOYED
        };
        assert_eq!(
            refusal(add_with(&rpc, &tx, top_only)),
            refused_shape(ShapeRefused::InSubmit)
        );
        // The deployed program takes it, but without a plan for the wallet,
        // nothing is built.
        assert_eq!(refusal(add_with(&rpc, &tx, DEPLOYED)), NOT_REBUILT);
        assert_eq!(refusal(add_with(&rpc, &tx, EVERYTHING)), NOT_REBUILT);
        // What it is rebuilt from is the vote as first written.
        let plan = pollster::block_on(read_for_checks(&rpc, &tx, None)).unwrap();
        assert!(plan.wrapped());
        let unwrapped = plan.plain_tx().unwrap();
        assert!(unwrapped.signers.iter().any(|slot| slot.pubkey == member));
        assert_eq!(
            unwrapped.native_instructions(),
            [squad().approve(3, &member)]
        );
    }

    #[test]
    fn a_simulation_that_fails_adds_nothing_and_signatures_held_are_counted() {
        let mut rpc = node(&[payment()]);
        rpc.simulation = Some(SimulationReply {
            err: Some(serde_json::json!({ "InstructionError": [0, { "Custom": 6006 }] })),
            ..SimulationReply::default()
        });
        let refused = refusal(add(&rpc, &approval()));
        assert!(
            refused.starts_with(
                "The node says the approval with these instructions would fail (instruction 1:"
            ),
            "{refused}"
        );

        // A signature already made over the approval no longer applies.
        let rpc = node(&[payment()]);
        let mut signed = approval();
        signed.signers[0].filled = true;
        let with = add(&rpc, &signed).unwrap();
        assert_eq!(with.voided, 1);
        assert!(checks_note(&with)
            .plain()
            .contains("1 signature(s) made over it earlier were cleared"));
    }
}
