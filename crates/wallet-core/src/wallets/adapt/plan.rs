//! Deciding how a transaction has to be rebuilt for the wallets it names.

use super::*;

/// Decide whether `tx` names a programmatic signer, a Vector wallet or a
/// Squads vault of `wallets` as a signer, and how to rebuild it if so.
/// `None` when every signer is an ordinary key.
///
/// A fee payer the transaction already names stays when it is a hot or
/// offline wallet of `wallets`, which is how Build compiles one. Otherwise the fee payer is
/// the one set for the wallet (see `route`), and when none is set, `selected`
/// pays if it is a hot wallet.
pub fn plan_adaptation(
    tx: &DecodedTx,
    wallets: &[WalletRecord],
    selected: Option<WalletId>,
) -> Result<Option<AdaptPlan>, AdaptError> {
    plan_adaptation_with(tx, wallets, selected, &[])
}

/// [`plan_adaptation`], where `made_here` are keys this device made for the
/// transaction in place of a site's one-time keys, such as a new mint. A
/// vault proposal may name them as signers besides its vault: they sign the
/// transaction that executes it, so it has to execute at once.
pub fn plan_adaptation_with(
    tx: &DecodedTx,
    wallets: &[WalletRecord],
    selected: Option<WalletId>,
    made_here: &[Pubkey],
) -> Result<Option<AdaptPlan>, AdaptError> {
    let involved: Vec<&WalletRecord> = wallets
        .iter()
        .filter(|wallet| {
            matches!(
                wallet.kind,
                WalletKind::Programmatic { .. } | WalletKind::Squads { .. }
            ) && wallet
                .address()
                .is_some_and(|address| tx.signers.iter().any(|slot| slot.pubkey == address))
        })
        .collect();
    let Some(first) = involved.first() else {
        return Ok(None);
    };
    if tx.uses_durable_nonce {
        return Err(AdaptError::DurableNonce(first.label.clone()));
    }
    if !authority_slots(tx).is_empty() {
        return Err(WrapError::AlreadyWrapped.into());
    }
    let hot = |id: WalletId| {
        wallets
            .iter()
            .find(|wallet| wallet.id == id && wallet.kind.can_sign())
            .and_then(|wallet| wallet.address())
    };
    let own_payer = tx.fee_payer().filter(|payer| {
        wallets
            .iter()
            .any(|wallet| wallet.kind.can_pay() && wallet.address() == Some(*payer))
    });
    let payer_for = |wallet: &WalletRecord| -> Result<Pubkey, AdaptError> {
        signing_route(wallets, wallet.id)?
            .fee_payer
            .map(|(_, pubkey)| pubkey)
            .or_else(|| selected.and_then(hot))
            .ok_or_else(|| AdaptError::NoFeePayer(wallet.label.clone()))
    };

    let mut multisigs = involved
        .iter()
        .filter(|wallet| matches!(wallet.kind, WalletKind::Squads { .. }));
    if let Some(wallet) = multisigs.next() {
        if multisigs.next().is_some() {
            return Err(AdaptError::SeveralMultisigs);
        }
        let route = signing_route(wallets, wallet.id)?;
        let Some(RouteStep::Squads {
            squad,
            vault,
            member,
            ..
        }) = route.steps.first().cloned()
        else {
            return Err(RouteError::UnknownWallet.into());
        };
        // The vault signs inside the program. Besides it, only a key made
        // here can, from the transaction that executes the proposal. The
        // fee payer's message-level signer bit is not an inner signature
        // (see [`site_instructions`]).
        let mut one_time = Vec::new();
        for meta in site_instructions(tx, vault)
            .iter()
            .flat_map(|instruction| instruction.accounts.clone())
            .filter(|meta| meta.is_signer && meta.pubkey != vault)
        {
            if !made_here.contains(&meta.pubkey) {
                return Err(AdaptError::OtherSigner {
                    squad: wallet.label.clone(),
                    signer: meta.pubkey,
                });
            }
            if !one_time.contains(&meta.pubkey) {
                one_time.push(meta.pubkey);
            }
        }
        let vault_index = (0..=u8::MAX)
            .find(|index| squad.vault(*index) == vault)
            .ok_or_else(|| AdaptError::Unreadable(wallet.label.clone()))?;
        let members = planned_members(wallets, wallet)?;
        // The acting member must have a nonce when it is programmatic: the
        // create instruction always needs it to wrap.
        if let Some(acting) = members.iter().find(|planned| planned.address == member) {
            if matches!(acting.kind, MemberKind::Programmatic { .. }) && acting.nonce.is_none() {
                return Err(AdaptError::NoNonce(acting.label.clone()));
            }
        }
        return Ok(Some(AdaptPlan {
            squads: Some(SquadsPlan {
                label: wallet.label.clone(),
                squad,
                vault,
                vault_index,
                member,
                members,
                one_time,
            }),
            programmatic: Vec::new(),
            vector: Vec::new(),
            nonce: None,
            // A proposal's creator pays its rent, so only a wallet of this
            // device is kept as the payer.
            fee_payer: match own_payer {
                Some(payer) => payer,
                None => payer_for(wallet)?,
            },
        }));
    }

    let mut programmatic = Vec::new();
    let mut vector = Vec::new();
    let mut nonce = None;
    for wallet in &involved {
        if let Some(plan) = vector_plan(wallet)? {
            vector.push(plan);
            continue;
        }
        let WalletKind::Programmatic {
            protocol:
                Protocol::Ed25519Signer {
                    authority,
                    nonce: own,
                },
            ..
        } = &wallet.kind
        else {
            continue;
        };
        // One nonce orders the authorization all of them sign, so the first
        // signer that has one lends it.
        if nonce.is_none() {
            nonce = own.map(|account| NoncePlan {
                owner: wallet.label.clone(),
                account,
            });
        }
        programmatic.push(ProgrammaticPlan {
            label: wallet.label.clone(),
            authority: *authority,
            address: wallet.address().unwrap_or_default(),
        });
    }
    if let (Some(plan), None) = (programmatic.first(), &nonce) {
        return Err(AdaptError::NoNonce(plan.label.clone()));
    }
    // The transaction's own payer stays when it is a key. A site that made
    // the programmatic signer or the Vector account pay gets the hot wallet
    // chosen for it.
    let keyless = |payer: Pubkey| {
        programmatic.iter().any(|plan| plan.address == payer)
            || vector.iter().any(|plan| plan.account == payer)
    };
    let fee_payer = match tx.fee_payer() {
        Some(payer) if !keyless(payer) => payer,
        _ => payer_for(first)?,
    };
    Ok(Some(AdaptPlan {
        squads: None,
        programmatic,
        vector,
        nonce,
        fee_payer,
    }))
}

/// The acting member of `multisig` first, then its `approves_with`, as
/// [`PlannedMember`]s. Hardware wallets and nested multisigs are skipped.
fn planned_members(
    wallets: &[WalletRecord],
    multisig: &WalletRecord,
) -> Result<Vec<PlannedMember>, AdaptError> {
    let WalletKind::Squads {
        member_wallet,
        approves_with,
        ..
    } = &multisig.kind
    else {
        return Ok(Vec::new());
    };
    let resolve = |id: WalletId| -> Result<Option<PlannedMember>, AdaptError> {
        let Some(wallet) = wallets.iter().find(|wallet| wallet.id == id) else {
            return Ok(None);
        };
        let address = wallet.address().unwrap_or_default();
        Ok(Some(match &wallet.kind {
            WalletKind::Keypair { .. } | WalletKind::Seed { .. } => PlannedMember {
                wallet: id,
                label: wallet.label.clone(),
                address,
                kind: MemberKind::HotKey,
                vector: None,
                programmatic: None,
                nonce: None,
            },
            WalletKind::Offline { .. } => PlannedMember {
                wallet: id,
                label: wallet.label.clone(),
                address,
                kind: MemberKind::Offline,
                vector: None,
                programmatic: None,
                nonce: None,
            },
            WalletKind::Programmatic {
                protocol: Protocol::Ed25519Signer { authority, nonce },
                ..
            } => PlannedMember {
                wallet: id,
                label: wallet.label.clone(),
                address,
                kind: MemberKind::Programmatic {
                    has_nonce: nonce.is_some(),
                },
                vector: None,
                programmatic: Some(ProgrammaticPlan {
                    label: wallet.label.clone(),
                    authority: *authority,
                    address,
                }),
                nonce: *nonce,
            },
            WalletKind::Programmatic {
                protocol: Protocol::Vector { .. },
                ..
            } => PlannedMember {
                wallet: id,
                label: wallet.label.clone(),
                address,
                kind: MemberKind::Vector,
                vector: vector_plan(wallet)?,
                programmatic: None,
                nonce: None,
            },
            WalletKind::Hardware { .. } | WalletKind::Squads { .. } => return Ok(None),
        }))
    };
    let Some(acting) = member_wallet.map(resolve).transpose()?.flatten() else {
        return Ok(Vec::new());
    };
    let mut members = vec![acting];
    for id in approves_with {
        let Some(member) = resolve(*id)? else {
            continue;
        };
        if !members.iter().any(|known| known.address == member.address) {
            members.push(member);
        }
    }
    Ok(members)
}
