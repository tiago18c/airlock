//! A `resolve` request on this device (`docs/intents.md`): its notes checked
//! against the chain, keys made for its replaceable new accounts or the
//! addresses a multisig's program signs for them at, and the replaced
//! addresses written into its transactions, once for the request however
//! often it is prepared. What follows is a site's request like any other:
//! converted, rebuilt for a wallet without a key, reviewed, signed.
//!
//! A multisig's addresses follow its next proposal number, so they are
//! derived again whenever a preparation finds that number moved.
//!
//! The wallet chooses a resolution's blockhash, so its transactions are moved
//! onto a fresh one before the first signature, as this app's own are.

use std::collections::BTreeMap;

use app_core::bridge::ResolveCall;
use app_core::diagnostics::TxLog;
use app_core::FlowId;
use solana_pubkey::Pubkey;
use wallet_core::intent::{
    apply_to_request, carried_transactions, check_notes, tx_role, Composed, Delivery, Requirement,
    ResolutionFailure, TxRole, Unsupported,
};
use wallet_core::{
    decode_transaction, parse_wire, renew, sha256_bytes, KeypairSigner, LookupTables, RouteStep,
};

use super::sentence;
use super::sign::lookup_tables_for;
use crate::state::forms::ResolvedRequest;
use crate::state::session::Session;

/// The `resolve` request `request_id` from `origin` with its notes applied:
/// the one kept from an earlier preparation, or made now. A note that fails
/// a check that needed the lookup tables or a node, or a rewrite the notes
/// do not allow, fails the resolution with `bad_notes`, which takes the
/// request out of the queue; the message says why. A new account a
/// multisig cannot sign for fails it with `unsupported`. A node that does
/// not answer is an error to try again on.
///
/// A request whose new accounts a multisig's program signs for reads the
/// multisig's transaction number on every preparation, and is made again
/// when it moved since the one kept.
pub async fn resolve_request(
    session: Session,
    origin: &str,
    request_id: &str,
    call: &ResolveCall,
) -> Result<ResolvedRequest, String> {
    let form = session.forms.sign;
    let carries = !call.plan.carried.is_empty();
    if let Some(kept) = form.resolved(request_id).filter(|_| !carries) {
        return Ok(kept);
    }
    let wires = call
        .transactions
        .iter()
        .map(|transaction| transaction.wire.bytes().map(<[u8]>::to_vec))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "A transaction of this request could not be read.".to_string())?;

    // A v0 transaction that reads lookup tables could not be checked when
    // it arrived.
    if !call.notes_checked {
        let mut decoded = Vec::with_capacity(wires.len());
        for wire in &wires {
            let tables = lookup_tables_for(session, wire).await?;
            decoded.push(decode_transaction(wire, &tables).map_err(sentence)?);
        }
        if let Err(note) = check_notes(&decoded, &call.notes, &call.account) {
            return Err(fail(session, origin, call, note.to_string()));
        }
    }
    // Check 3's second half. Without a node it is skipped.
    if let Ok(rpc) = session.rpc() {
        match wallet_flows::intent::new_accounts_unseen(rpc.as_ref(), &call.notes).await {
            Ok(None) => {}
            Ok(Some(note)) => return Err(fail(session, origin, call, note.to_string())),
            Err(err) => return Err(sentence(err)),
        }
    }
    let carried = if carries {
        Some(carried_addresses(session, origin, call, &wires).await?)
    } else {
        None
    };
    let derived_at = carried.as_ref().map(|(at, _)| *at);
    // Another preparation of the same request may have finished meanwhile.
    if let Some(kept) = form
        .resolved(request_id)
        .filter(|kept| kept.derived_at == derived_at)
    {
        return Ok(kept);
    }
    let carried: Vec<Pubkey> = carried.map(|(_, addresses)| addresses).unwrap_or_default();

    // From here to keeping it nothing waits, so no other preparation of
    // this request runs in between and makes keys of its own.
    let keys: Vec<KeypairSigner> = call
        .plan
        .new_keys
        .iter()
        .map(|_| KeypairSigner::new_random())
        .collect();
    let made: Vec<Pubkey> = keys.iter().map(wallet_core::WalletSigner::pubkey).collect();
    let (rewritten, replacements) =
        match apply_to_request(&wires, &call.notes, &call.plan.replaced(&made, &carried)) {
            Ok(done) => done,
            Err(note) => return Err(fail(session, origin, call, note.to_string())),
        };
    let app_signs = call.plan.app_signs.clone();
    let mut keep: Vec<Pubkey> = call
        .notes
        .accounts
        .iter()
        .map(|(address, _)| address)
        .chain(replacements.addresses.values())
        .chain(made.iter())
        .copied()
        .filter(|address| !app_signs.contains(address))
        .collect();
    keep.sort();
    keep.dedup();
    let resolved = ResolvedRequest {
        request: request_id.to_owned(),
        origin: origin.to_owned(),
        resolution: call.resolution.clone(),
        account: call.account,
        delivery: call.delivery,
        timing: call.profile.timing,
        wires: rewritten,
        replaced: replacements.addresses.clone(),
        new_keys: call.plan.new_keys.iter().copied().zip(made).collect(),
        carried: call.plan.carried.iter().copied().zip(carried).collect(),
        derived_at,
        fee_payer: call.plan.fee_payer,
        app_signs,
        keep,
        message: call.message.clone(),
        elsewhere: false,
    };
    let kept = form.keep_resolved(resolved.clone());
    if kept != resolved {
        return Ok(kept);
    }
    // The keys sign with this device's others, as a key made in place of a
    // site's one-time key does.
    session.keep_made_here(call.plan.new_keys.iter().copied().zip(keys.iter()));
    if !replacements.addresses.is_empty() {
        let _ = session.command(|core| {
            core.resolution_set_replaced(origin, &call.resolution, replacements.addresses)
        });
    }
    Ok(kept)
}

/// Fail the resolution of `call` with `bad_notes`, saying `message`, and
/// answer the message.
fn fail(session: Session, origin: &str, call: &ResolveCall, message: String) -> String {
    let failure = ResolutionFailure {
        code: "bad_notes".into(),
        message: message.clone(),
        requirement: None,
    };
    let _ = session.command(|core| core.resolution_fail(origin, &call.resolution, failure));
    message
}

/// Fail the resolution of `call` with `unsupported`, naming the
/// requirement, and answer the message.
fn fail_unsupported(
    session: Session,
    origin: &str,
    call: &ResolveCall,
    unsupported: Unsupported,
) -> String {
    let failure = ResolutionFailure {
        code: unsupported.code().into(),
        message: unsupported.message.clone(),
        requirement: Some(unsupported.requirement),
    };
    let _ = session.command(|core| core.resolution_fail(origin, &call.resolution, failure));
    unsupported.message
}

/// The addresses the multisig of `call`'s account signs for in place of its
/// carried new accounts, in the plan's order, and the multisig's newest
/// transaction number they were derived at.
///
/// Each carried account signs one transaction of the request. That
/// transaction is stored as a proposal numbered after the multisig's newest
/// and after the proposals the transactions before it store in the same
/// multisig. Its carried accounts take that proposal's ephemeral signers,
/// numbered in the plan's order. `wires` are the site's transactions, before
/// the notes change them.
async fn carried_addresses(
    session: Session,
    origin: &str,
    call: &ResolveCall,
    wires: &[Vec<u8>],
) -> Result<(u64, Vec<Pubkey>), String> {
    let unsupported = |message: String| {
        fail_unsupported(
            session,
            origin,
            call,
            Unsupported::new(Requirement::NewAccounts, message),
        )
    };
    let signs_in = match carried_transactions(wires, &call.plan.carried) {
        Ok(signs_in) => signs_in,
        Err(why) => return Err(fail_unsupported(session, origin, call, why)),
    };
    let wallet = session.core().with_wallets(|data| {
        data?
            .wallets
            .iter()
            .find(|wallet| wallet.address() == Some(call.account))
            .map(|wallet| wallet.id)
    });
    let route = wallet.and_then(|id| session.route_of(id).ok());
    let Some(RouteStep::Squads { squad, .. }) =
        route.and_then(|route| route.steps.first().cloned())
    else {
        return Err(unsupported(format!(
            "{} is not a multisig of this device, so nothing here signs for its new accounts.",
            call.account
        )));
    };
    let Ok(rpc) = session.rpc() else {
        return Err("Preparing this request needs a connection to a node. The multisig signs for its new accounts at addresses that follow its next proposal number, which only the node knows.".into());
    };
    let account = rpc
        .get_account(&squad.address)
        .await
        .map_err(sentence)?
        .ok_or_else(|| format!("The multisig {} is not on this network.", squad.address))?;
    let state = squad.parse_state(&account.data).ok_or_else(|| {
        format!(
            "The account of the multisig {} could not be read.",
            squad.address
        )
    })?;

    // Which transactions store a proposal in this multisig, as the rebuild
    // plans them, with the carried accounts allowed to sign inside.
    let mut stores = Vec::with_capacity(wires.len());
    for wire in wires {
        let tables = super::sign::lookup_tables_for(session, wire).await?;
        let stored = decode_transaction(wire, &tables).ok().and_then(|tx| {
            session
                .plan_adaptation_carrying(&tx, &[], &call.plan.carried)
                .ok()
                .flatten()
                .and_then(|plan| plan.stores_in())
        });
        stores.push(stored == Some(squad));
    }
    let mut addresses = Vec::with_capacity(call.plan.carried.len());
    for (at, (old, transaction)) in call.plan.carried.iter().zip(&signs_in).enumerate() {
        if !stores.get(*transaction).copied().unwrap_or(false) {
            return Err(unsupported(format!(
                "The new account {old} signs transaction {transaction}, which the multisig does not store as a proposal, so it cannot sign for it."
            )));
        }
        let ahead = stores[..*transaction]
            .iter()
            .filter(|stores| **stores)
            .count() as u64;
        let index = state
            .transaction_index
            .checked_add(1 + ahead)
            .ok_or_else(|| "The multisig has no transaction number left.".to_string())?;
        // Numbered among the carried accounts of the same transaction.
        let signer = signs_in[..at]
            .iter()
            .filter(|earlier| *earlier == transaction)
            .count();
        let signer = u8::try_from(signer)
            .map_err(|_| "A proposal takes at most 255 new accounts.".to_string())?;
        addresses.push(squad.ephemeral_signer(index, signer));
    }
    Ok((state.transaction_index, addresses))
}

/// Log that transaction `original` of a request became `rewritten` when its
/// notes were applied, under `flow`.
pub fn log_notes_applied(session: Session, flow: FlowId, original: &[u8], rewritten: &[u8]) {
    if original == rewritten {
        return;
    }
    let cluster = session.cluster();
    TxLog {
        flow,
        wire: rewritten,
        cluster: Some(&cluster),
    }
    .log_transformed("intent_notes", &message_hash(original));
}

/// `wire` on the node's latest blockhash, with the height that blockhash
/// expires at. Every signature over it is cleared; an authorization inside
/// it is left as it is. Nothing is guessed or replaced, unlike the Sign
/// screen's move: a resolution's notes already said what is replaced.
pub async fn onto_fresh_blockhash(
    session: Session,
    wire: &[u8],
    flow: FlowId,
) -> Result<(Vec<u8>, u64), String> {
    let rpc = session.rpc().map_err(sentence)?;
    let (hash, last_valid_height) = rpc
        .get_latest_blockhash_with_height()
        .await
        .map_err(sentence)?;
    let moved = renew(wire, hash, &[]).map_err(sentence)?;
    let cluster = session.cluster();
    TxLog {
        flow,
        wire: &moved,
        cluster: Some(&cluster),
    }
    .log_transformed("refresh_blockhash", &message_hash(wire));
    Ok((moved, last_valid_height))
}

/// Whether `wire` lives on a durable nonce, which no move replaces.
pub fn on_durable_nonce(wire: &[u8], tables: &LookupTables) -> bool {
    decode_transaction(wire, tables).is_ok_and(|tx| tx.uses_durable_nonce)
}

/// `wires`, one for each transaction the site sent, as the transactions the
/// wallet composed for `account`, each with its role.
pub fn composed(wires: &[Vec<u8>], account: &Pubkey) -> Vec<Composed> {
    wires
        .iter()
        .enumerate()
        .map(|(index, wire)| Composed {
            role: decode_transaction(wire, &LookupTables::new())
                .ok()
                .and_then(|tx| tx_role(&tx, account))
                .unwrap_or(TxRole::Direct),
            of: vec![index],
            wire: wire.clone(),
        })
        .collect()
}

/// The resolution of `resolved`, approved here and waiting on another
/// device, takes `wires`: every transaction, complete. It is `sending`, and
/// the online service sends it. Answers why not when it does not take them.
pub fn finish_signed_elsewhere(
    session: &Session,
    resolved: &ResolvedRequest,
    wires: &[Vec<u8>],
) -> Result<(), String> {
    let composed = composed(wires, &resolved.account);
    session
        .command(|core| {
            core.resolution_signed_elsewhere(&resolved.origin, &resolved.resolution, composed)
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// The person stopped request `id` on this device after approving it for
/// another device's signature, before that signature came: its resolution
/// fails, since nothing was sent. Answers whether it was such a request.
pub fn stop_elsewhere(session: &Session, id: &str) -> bool {
    let Some(resolved) = session
        .forms
        .sign
        .resolved(id)
        .filter(|resolved| resolved.elsewhere)
    else {
        return false;
    };
    let failure = ResolutionFailure {
        code: "declined".into(),
        message: "It was stopped on the wallet before the other device signed. Nothing was sent."
            .into(),
        requirement: None,
    };
    session
        .command(|core| core.resolution_fail(&resolved.origin, &resolved.resolution, failure))
        .is_ok()
}

/// What the site asked to have done once this device has signed, in words.
pub fn delivery_words(delivery: Delivery) -> &'static str {
    match delivery {
        Delivery::Send => "Airlock sends it once you sign.",
        Delivery::Cosign => {
            "Once you sign, the site adds its own signatures, and then Airlock sends it. Until the site has signed, it can still cancel."
        }
        Delivery::Return => {
            "Once you sign, it goes back to the site, which adds its signatures and sends it itself. Airlock watches for it on the network."
        }
    }
}

/// How long a request whose signature comes from another device stays
/// valid, in words: on the key's durable nonce until that key signs, or on a
/// recent blockhash, which the other device's signature covers. The wallet
/// chooses a resolution's lifetime, so until a signature is made it can be
/// moved again; once the other device has signed, it cannot.
pub fn lifetime_words(on_durable_nonce: bool) -> &'static str {
    if on_durable_nonce {
        "It is on this wallet's durable nonce, so it stays valid however long the other device takes. It is void if anything else uses that nonce first."
    } else {
        "It is on a recent blockhash, which lasts about a minute, and the other device's signature covers it: bring the signature back before then. If it runs out first, Rebuild on a fresh blockhash on Review and Sign and have it signed again. A durable nonce, from this wallet's Settings, lets a request wait as long as it takes."
    }
}

/// The addresses of `replaced` that are not keys made here, addresses a
/// multisig signs for, or the payer: those derived again from one that was
/// replaced.
pub fn derived_again(resolved: &ResolvedRequest) -> BTreeMap<Pubkey, Pubkey> {
    resolved
        .replaced
        .iter()
        .filter(|(old, _)| {
            !resolved.new_keys.iter().any(|(new, _)| new == *old)
                && !resolved.carried.iter().any(|(new, _)| new == *old)
                && resolved.fee_payer.is_none_or(|(payer, _)| payer != **old)
        })
        .map(|(old, new)| (*old, *new))
        .collect()
}

fn message_hash(wire: &[u8]) -> String {
    parse_wire(wire)
        .map(|view| hex::encode(sha256_bytes(&view.message)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
