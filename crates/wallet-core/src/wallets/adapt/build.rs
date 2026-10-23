//! Carrying a plan out: the rebuilt transaction and what was done to it.

use super::*;
use crate::wallets::proposals::{
    executor_for, keyless_together, proposal_shapes, usable_voters, ChosenMember, ProposalShape,
};
use crate::wallets::relay::{PASSTHROUGH_CU, RELAY_OVERHEAD_CU};
use crate::wallets::squads::PERMISSION_EXECUTE;
use crate::wallets::vector::{must_rotate, SchemeId};

/// Why a shape did not fit, when the next one should be tried.
enum ShapeSkip {
    Size(usize),
    Compute(u32),
    /// No order of its instructions keeps a programmatic member's together.
    Order,
}

/// A shape that fit: the wire, the wraps it took, and the shape as built,
/// with its approvals in instruction order and the executor that took part.
type BuiltShape = (Vec<u8>, Vec<AdaptedStep>, ProposalShape);

impl AdaptPlan {
    /// The wallets without a key this plan rebuilds for: the multisig first,
    /// then the programmatic signers or the Vector wallets.
    pub fn asked(&self) -> Vec<AskedWallet> {
        let mut asked = Vec::new();
        if let Some(plan) = &self.squads {
            asked.push(AskedWallet::Squads {
                wallet: plan.label.clone(),
                vault: plan.vault,
                member: plan.member,
            });
            for member in &plan.members {
                if let Some(vector) = &member.vector {
                    asked.push(AskedWallet::Vector {
                        wallet: vector.label.clone(),
                        scheme: vector.scheme,
                        account: vector.account,
                    });
                }
                if let Some(programmatic) = &member.programmatic {
                    asked.push(AskedWallet::Programmatic {
                        wallet: programmatic.label.clone(),
                        authority: programmatic.authority,
                        address: programmatic.address,
                    });
                }
            }
        }
        asked.extend(
            self.programmatic
                .iter()
                .map(|plan| AskedWallet::Programmatic {
                    wallet: plan.label.clone(),
                    authority: plan.authority,
                    address: plan.address,
                }),
        );
        asked.extend(self.vector.iter().map(|plan| AskedWallet::Vector {
            wallet: plan.label.clone(),
            scheme: plan.scheme,
            account: plan.account,
        }));
        asked
    }

    /// Accounts whose current data [`Self::build`] needs.
    pub fn accounts_to_read(&self) -> Vec<Pubkey> {
        let mut accounts = Vec::new();
        if let Some(plan) = &self.squads {
            accounts.push(plan.squad.address);
            for member in &plan.members {
                if let Some(vector) = &member.vector {
                    accounts.push(vector.account);
                }
                if let Some(nonce) = member.nonce {
                    accounts.push(nonce);
                }
            }
        }
        accounts.extend(self.nonce.as_ref().map(|nonce| nonce.account));
        accounts.extend(self.vector.iter().map(|plan| plan.account));
        accounts
    }

    /// Rebuild `tx`, the transaction this plan was made for. `accounts` holds
    /// what [`Self::accounts_to_read`] asked for, as read just now. An
    /// account that does not exist is left out.
    pub fn build(
        &self,
        tx: &DecodedTx,
        accounts: &[(Pubkey, RawAccount)],
        blockhash: Hash,
    ) -> Result<Adapted, AdaptError> {
        let read = |address: &Pubkey| {
            accounts
                .iter()
                .find(|(key, _)| key == address)
                .map(|(_, account)| account)
        };
        let mut steps = Vec::new();
        let mut context = Vec::new();
        let mut current = tx.clone();

        if let Some(plan) = &self.squads {
            let account = read(&plan.squad.address)
                .ok_or_else(|| AdaptError::NotOnChain(plan.label.clone()))?;
            let state = Some(account)
                .filter(|account| account.owner == plan.squad.program.program_id())
                .and_then(|account| plan.squad.parse_state(&account.data))
                .ok_or_else(|| AdaptError::Unreadable(plan.label.clone()))?;
            let permissions = state
                .members
                .iter()
                .find(|member| member.key == plan.member)
                .map(|member| member.permissions)
                .ok_or_else(|| AdaptError::NotAMember {
                    squad: plan.label.clone(),
                    member: plan.member,
                })?;
            if permissions & PERMISSION_INITIATE == 0 {
                return Err(AdaptError::CannotPropose {
                    squad: plan.label.clone(),
                    member: plan.member,
                });
            }
            let index = state.transaction_index.checked_add(1).ok_or_else(|| {
                AdaptError::Message("the multisig has no transaction number left".into())
            })?;

            let chosen: Vec<ChosenMember> = plan
                .members
                .iter()
                .map(|member| ChosenMember {
                    wallet: member.wallet,
                    address: member.address,
                    permissions: state
                        .members
                        .iter()
                        .find(|known| known.key == member.address)
                        .map(|known| known.permissions)
                        .unwrap_or(0),
                    kind: member.kind,
                })
                .collect();
            // Co-voters are optional: one that could not sign here is left
            // out, not allowed to stop the proposal. The acting member's
            // problems still stop it, because it creates.
            let chosen: Vec<ChosenMember> = chosen
                .into_iter()
                .zip(&plan.members)
                .enumerate()
                .filter(|(at, (_, member))| *at == 0 || co_voter_can_sign(member, read))
                .map(|(_, (chosen, _))| chosen)
                .collect();
            let shapes = proposal_shapes(&state, &[], &chosen);
            let structural = structural_held_back(&state, &chosen);
            let stored = site_instructions(tx, plan.vault);

            let mut previous_failure: Option<HeldBack> = None;
            let mut built = None;
            for (at, shape) in shapes.iter().enumerate() {
                match try_squads_shape(
                    self,
                    plan,
                    tx,
                    &stored,
                    index,
                    shape,
                    &chosen,
                    read,
                    blockhash,
                    &mut context,
                ) {
                    Ok((wire, wrap_steps, shape)) => {
                        let held_back = if at == 0 && shape.executor.is_some() {
                            None
                        } else if at == 0 {
                            Some(structural.clone())
                        } else {
                            previous_failure.clone()
                        };
                        // A key made here signs only the transaction that
                        // executes, and is not kept for a later one.
                        if let (Some(signer), Some(held)) = (plan.one_time.first(), &held_back) {
                            return Err(AdaptError::NotAtOnce {
                                squad: plan.label.clone(),
                                signer: *signer,
                                held: held.clone(),
                            });
                        }
                        steps.push(AdaptedStep::Squads {
                            wallet: plan.label.clone(),
                            vault: plan.vault,
                            member: plan.member,
                            index,
                            threshold: state.threshold,
                            voters: shape.voters.clone(),
                            executor: shape.executor,
                            held_back,
                        });
                        steps.extend(wrap_steps);
                        built = Some(wire);
                        break;
                    }
                    Err(Err(skip)) => {
                        previous_failure = Some(match skip {
                            ShapeSkip::Size(bytes) => HeldBack::Size { bytes },
                            ShapeSkip::Compute(units) => HeldBack::Compute { units },
                            ShapeSkip::Order => HeldBack::Order,
                        });
                    }
                    Err(Ok(error)) => return Err(error),
                }
            }
            let wire = built.ok_or_else(|| {
                let bytes = previous_failure
                    .and_then(|held| match held {
                        HeldBack::Size { bytes } => Some(bytes),
                        _ => None,
                    })
                    .unwrap_or(0);
                AdaptError::Message(format!(
                    "the stored instructions are too large for this member's scheme in one transaction ({bytes} bytes)"
                ))
            })?;
            current =
                decode_v1_transaction(&wire).map_err(|err| AdaptError::Message(err.to_string()))?;
        } else {
            // Programmatic first so a Vector wrap can cover the `Submit`.
            if let Some(planned) = &self.nonce {
                let (wire, wrap_steps) = wrap_programmatic_plans(
                    self,
                    &self.programmatic,
                    planned,
                    &current,
                    read,
                    blockhash,
                )?;
                steps.extend(wrap_steps);
                current = decode_v1_transaction(&wire)
                    .map_err(|err| AdaptError::Message(err.to_string()))?;
            }
            if !self.vector.is_empty() {
                let (wire, wrap_steps) =
                    wrap_vector_plans(self, &self.vector, &current, read, blockhash, &mut context)?;
                steps.extend(wrap_steps);
                current = decode_v1_transaction(&wire)
                    .map_err(|err| AdaptError::Message(err.to_string()))?;
            }
        }

        Ok(Adapted {
            wire: current.wire,
            steps,
            fee_payer: self.fee_payer,
            voided: tx.signers.iter().filter(|slot| slot.filled).count(),
            context,
        })
    }
}

/// Why the fullest shape has no execute, when it is permissions or the
/// threshold rather than size.
fn structural_held_back(
    state: &crate::wallets::squads::SquadState,
    chosen: &[ChosenMember],
) -> HeldBack {
    if state.time_lock != 0 {
        return HeldBack::TimeLock;
    }
    let missing = state.threshold;
    let voters = usable_voters(chosen, &[], false);
    if (voters.len() as u16) < missing {
        return HeldBack::Votes {
            missing: missing.saturating_sub(voters.len() as u16),
        };
    }
    if executor_for(&voters, chosen, false).is_none() {
        return HeldBack::NoExecutor;
    }
    // Enough voters and an executor, but time lock already handled: the
    // fullest shape still omits execute only when votes do not reach the
    // threshold after filtering — already covered by Votes.
    HeldBack::Votes {
        missing: missing.saturating_sub(voters.len() as u16).max(1),
    }
}

/// Compile the Squads instructions for `shape`, wrap the signers it needs,
/// and refuse when the result is too large or its compute estimate exceeds
/// the limit.
#[allow(clippy::too_many_arguments)]
fn try_squads_shape<'a>(
    plan: &AdaptPlan,
    squads: &SquadsPlan,
    site: &DecodedTx,
    stored: &[Instruction],
    index: u64,
    shape: &ProposalShape,
    chosen: &[ChosenMember],
    read: impl Fn(&Pubkey) -> Option<&'a RawAccount>,
    blockhash: Hash,
    context: &mut Vec<VectorContext>,
) -> Result<BuiltShape, Result<AdaptError, ShapeSkip>> {
    let shape = &arrange(squads.member, shape, chosen).ok_or(Err(ShapeSkip::Order))?;
    let create = squads
        .squad
        .create_transaction(
            index,
            squads.vault_index,
            stored,
            &squads.member,
            &plan.fee_payer,
        )
        .ok_or_else(|| {
            Ok(AdaptError::Message(
                "the transaction names more accounts than a proposal can store".into(),
            ))
        })?;
    let mut instructions = vec![
        create,
        squads
            .squad
            .create_proposal(index, &squads.member, &plan.fee_payer),
    ];
    for voter in &shape.voters {
        instructions.push(squads.squad.approve(index, voter));
    }
    if let Some(executor) = &shape.executor {
        instructions.push(
            squads
                .squad
                .execute(index, squads.vault_index, stored, executor),
        );
    }
    let limit = if shape.executor.is_some() {
        site.stated_cu_limit().saturating_add(SQUADS_OVERHEAD_CU)
    } else {
        SQUADS_OVERHEAD_CU
    };
    let config = site.budget.rebuilt_config(limit);
    let wire = compile_message(&plan.fee_payer, &instructions, blockhash, config)
        .map_err(|err| shape_error(err.into()))?
        .wire;
    let mut current =
        decode_v1_transaction(&wire).map_err(|err| Ok(AdaptError::Message(err.to_string())))?;

    let mut signers: Vec<Pubkey> = vec![squads.member];
    for voter in &shape.voters {
        if !signers.contains(voter) {
            signers.push(*voter);
        }
    }
    if let Some(executor) = shape.executor {
        if !signers.contains(&executor) {
            signers.push(executor);
        }
    }

    let vector_plans: Vec<&VectorPlan> = squads
        .members
        .iter()
        .filter(|member| signers.contains(&member.address))
        .filter_map(|member| member.vector.as_ref())
        .collect();
    let programmatic_plans: Vec<&ProgrammaticPlan> = squads
        .members
        .iter()
        .filter(|member| signers.contains(&member.address))
        .filter_map(|member| member.programmatic.as_ref())
        .collect();
    let nonce = squads.members.iter().find_map(|member| {
        if !signers.contains(&member.address) {
            return None;
        }
        member.nonce.map(|account| NoncePlan {
            owner: member.label.clone(),
            account,
        })
    });

    // Programmatic first: its `Submit` is left at the top level for the
    // Vector wrap to cover. The Vector digest includes that signature.
    let mut wrap_steps = Vec::new();
    let mut context_local = Vec::new();
    if !programmatic_plans.is_empty() {
        let plans: Vec<ProgrammaticPlan> = programmatic_plans.into_iter().cloned().collect();
        let planned = nonce.ok_or_else(|| {
            Ok(AdaptError::NoNonce(
                plans
                    .first()
                    .map(|plan| plan.label.clone())
                    .unwrap_or_default(),
            ))
        })?;
        let estimate = current.stated_cu_limit().saturating_add(RELAY_OVERHEAD_CU);
        if estimate > MAX_CU_LIMIT {
            return Err(Err(ShapeSkip::Compute(estimate)));
        }
        let (wire, steps) =
            wrap_programmatic_plans(plan, &plans, &planned, &current, &read, blockhash)
                .map_err(shape_error)?;
        wrap_steps.extend(steps);
        current =
            decode_v1_transaction(&wire).map_err(|err| Ok(AdaptError::Message(err.to_string())))?;
    }
    if !vector_plans.is_empty() {
        let plans: Vec<VectorPlan> = vector_plans.into_iter().cloned().collect();
        let estimate = estimate_vector_cu(&current, &plans);
        if estimate > MAX_CU_LIMIT {
            return Err(Err(ShapeSkip::Compute(estimate)));
        }
        let (wire, steps) =
            wrap_vector_plans(plan, &plans, &current, read, blockhash, &mut context_local)
                .map_err(shape_error)?;
        wrap_steps.extend(steps);
        current =
            decode_v1_transaction(&wire).map_err(|err| Ok(AdaptError::Message(err.to_string())))?;
    }
    context.extend(context_local);
    Ok((current.wire, wrap_steps, shape.clone()))
}

/// `shape` with its approvals in an order where every instruction a
/// programmatic member signs sits next to the others, for one `Submit`, and
/// every instruction a Vector member signs too when some order does, for one
/// `Passthrough`. When the executor makes that impossible, another member
/// that may execute takes its place. `None` when none keeps the programmatic
/// members' together.
fn arrange(
    creator: Pubkey,
    shape: &ProposalShape,
    chosen: &[ChosenMember],
) -> Option<ProposalShape> {
    let advances = |address: &Pubkey| {
        chosen
            .iter()
            .find(|member| member.address == *address)
            .and_then(|member| member.kind.advances())
    };
    let executors: Vec<Option<Pubkey>> = match shape.executor {
        None => vec![None],
        Some(first) => std::iter::once(first)
            .chain(
                chosen
                    .iter()
                    .filter(|member| {
                        member.kind != MemberKind::Offline
                            && member.permissions & PERMISSION_EXECUTE != 0
                    })
                    .map(|member| member.address),
            )
            .map(Some)
            .collect(),
    };
    // Both kinds together with any executor first, then programmatic only.
    [true, false].into_iter().find_map(|vector_too| {
        executors.iter().find_map(|executor| {
            keyless_together(
                Some(creator),
                &shape.voters,
                *executor,
                advances,
                vector_too,
            )
            .map(|voters| ProposalShape {
                voters,
                executor: *executor,
            })
        })
    })
}

/// Whether a co-voter can take part: a programmatic signer with a nonce of
/// its own, a Vector wallet whose account is ready to sign.
fn co_voter_can_sign<'a>(
    member: &PlannedMember,
    read: impl Fn(&Pubkey) -> Option<&'a RawAccount>,
) -> bool {
    match (&member.vector, member.kind) {
        (Some(vector), _) => vector_wrap(vector, read).is_ok(),
        (None, MemberKind::Programmatic { .. }) => member.nonce.is_some(),
        (None, MemberKind::Vector) => false,
        (None, MemberKind::HotKey | MemberKind::Offline) => true,
    }
}

fn shape_error(error: AdaptError) -> Result<AdaptError, ShapeSkip> {
    match error.too_large_len() {
        Some(bytes) => Err(ShapeSkip::Size(bytes)),
        None => Ok(error),
    }
}

/// Compute units the Vector wrap would ask for, before the cap.
fn estimate_vector_cu(tx: &DecodedTx, plans: &[VectorPlan]) -> u32 {
    let accounts: Vec<Pubkey> = plans.iter().map(|plan| plan.account).collect();
    let mut used = Vec::new();
    // One `Passthrough` per run of instructions Vector accounts sign.
    let mut passthroughs = 0u32;
    let mut in_run = false;
    for instruction in tx.native_instructions() {
        let signing: Vec<Pubkey> = accounts
            .iter()
            .copied()
            .filter(|account| {
                instruction
                    .accounts
                    .iter()
                    .any(|meta| meta.is_signer && meta.pubkey == *account)
            })
            .collect();
        for account in &signing {
            if !used.contains(account) {
                used.push(*account);
            }
        }
        if !signing.is_empty() && !in_run {
            passthroughs += 1;
        }
        in_run = !signing.is_empty();
    }
    let signatures: u32 = plans
        .iter()
        .filter(|plan| used.contains(&plan.account))
        .map(|plan| plan.scheme.advance_cu())
        .sum();
    tx.stated_cu_limit()
        .saturating_add(signatures)
        .saturating_add(passthroughs.saturating_mul(PASSTHROUGH_CU))
}

fn wrap_vector_plans<'a>(
    plan: &AdaptPlan,
    vectors: &[VectorPlan],
    current: &DecodedTx,
    read: impl Fn(&Pubkey) -> Option<&'a RawAccount>,
    blockhash: Hash,
    context: &mut Vec<VectorContext>,
) -> Result<(Vec<u8>, Vec<AdaptedStep>), AdaptError> {
    let mut wraps = Vec::new();
    for vector in vectors {
        let (wrap, known) = vector_wrap(vector, &read)?;
        context.push(known);
        wraps.push(wrap);
    }
    let wire = wrap_for_vector(current, &wraps, plan.fee_payer, blockhash)?;
    let steps = vectors
        .iter()
        .map(|vector| AdaptedStep::Vector {
            wallet: vector.label.clone(),
            scheme: vector.scheme,
            account: vector.account,
        })
        .collect();
    Ok((wire, steps))
}

/// The wrap for one Vector account and what a signer needs to know of it,
/// after checking the account on chain is the one the wallet was made for.
fn vector_wrap<'a>(
    vector: &VectorPlan,
    read: impl Fn(&Pubkey) -> Option<&'a RawAccount>,
) -> Result<(VectorWrap, VectorContext), AdaptError> {
    let raw =
        read(&vector.account).ok_or_else(|| AdaptError::NotInitialized(vector.label.clone()))?;
    let account =
        VectorAccount::parse(&crate::wallets::vector::PROGRAM_ID, raw).map_err(|error| {
            AdaptError::VectorAccount {
                wallet: vector.label.clone(),
                error,
            }
        })?;
    if account.scheme != vector.scheme || !account.created_with(&vector.key) {
        return Err(AdaptError::OtherKey(vector.label.clone()));
    }
    let leaf_floor = account.next_leaf().unwrap_or(0);
    let current_key = account
        .current_key()
        .map_or_else(|| vector.key.clone(), <[u8]>::to_vec);
    let rotates = match vector.scheme {
        SchemeId::Winternitz => Some(format!(
            "{} spends its key on every signature. Prepare the next public key before rebuilding.",
            vector.label
        )),
        SchemeId::Xmss if must_rotate(leaf_floor) => Some(format!(
            "{} has few XMSS leaves left, so only a move to a new key can be sent. Use Rotate key on its row, on the device that holds its key, and send that first.",
            vector.label
        )),
        SchemeId::Xmss
        | SchemeId::Ed25519
        | SchemeId::Eip191
        | SchemeId::Secp256k1
        | SchemeId::Falcon512
        | SchemeId::MlDsa44 => None,
    };
    let next_key = match rotates {
        // An XMSS account names the key it holds to keep it.
        None if vector.scheme == SchemeId::Xmss => Some(current_key.clone()),
        None => None,
        Some(missing) => {
            let next = vector
                .next_key
                .clone()
                .ok_or(AdaptError::Message(missing))?;
            if next == current_key {
                return Err(AdaptError::Message(format!(
                    "{} already holds the next key stored for it. Install it on the device that keeps the key, then scan or make the key after it.",
                    vector.label
                )));
            }
            Some(next)
        }
    };
    let known = VectorContext {
        account: vector.account,
        scheme: vector.scheme,
        nonce: Some(account.nonce),
        key: Some(current_key),
        leaf_floor,
    };
    Ok((
        VectorWrap {
            account: vector.account,
            scheme: vector.scheme,
            key: vector.key.clone(),
            next_key,
        },
        known,
    ))
}

fn wrap_programmatic_plans<'a>(
    plan: &AdaptPlan,
    programmatic: &[ProgrammaticPlan],
    planned: &NoncePlan,
    current: &DecodedTx,
    read: impl Fn(&Pubkey) -> Option<&'a RawAccount>,
    blockhash: Hash,
) -> Result<(Vec<u8>, Vec<AdaptedStep>), AdaptError> {
    let nonce = read(&planned.account)
        .ok_or_else(|| AdaptError::NotOnChain(format!("The nonce account of {}", planned.owner)))?;
    let nonce = Some(nonce)
        .filter(|account| account.owner == EXECUTOR_NONCE_PROGRAM_ID)
        .and_then(|account| ExecutorNonce::parse(planned.account, &account.data))
        .ok_or_else(|| AdaptError::Unreadable(format!("the nonce account of {}", planned.owner)))?;
    if !programmatic
        .iter()
        .any(|member| member.address == nonce.authority)
    {
        return Err(AdaptError::ForeignNonce(planned.owner.clone()));
    }
    let authorities: Vec<Pubkey> = programmatic.iter().map(|member| member.authority).collect();
    let wire = wrap_for_programmatic(current, &authorities, nonce, plan.fee_payer, blockhash)?;
    let steps = programmatic
        .iter()
        .map(|member| AdaptedStep::Programmatic {
            wallet: member.label.clone(),
            authority: member.authority,
            address: member.address,
        })
        .collect();
    Ok((wire, steps))
}
