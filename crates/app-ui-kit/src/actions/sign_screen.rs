//! What the Sign screen does with the transaction on it: take in what arrives,
//! simulate, sign and submit.

use crate::actions::sentence;
use crate::actions::sign::{prepare_request, rebuild_for_wallets, ReceivedMeta};
use crate::diagnostics::{next_flow_id, source_name, TxLog};
use crate::state::forms::{BlockhashExpiry, SimState, TxSource};
use crate::state::session::Session;
use app_core::bridge::BridgeAnswer;
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::protocol::{QrObject, Signed, TxContext};
use wallet_core::relay::{
    accept_signature_in, authority_signature_objects, authority_slots_in, insert_personal_sign,
    overlay_note, vector_signature_objects, AuthContext, SignatureUse,
};
use wallet_core::{
    decode_transaction, decode_v1_transaction, parse_wire, renew, sha256_bytes, Converted,
    DecodeError, DecodedTx, KeypairSigner, LookupTables, Phrase, TxFormat, WalletSigner,
};
use wallet_flows::adapt::rebuilt_note;
use wallet_flows::request::{
    accounts_created, one_time_signers, replaceable_signers, reshape_unsupported,
};
use wallet_flows::sign::VectorSigning;

/// Rebuild the transaction on Sign for the keyless wallet it asks to sign:
/// inside a Squads proposal, or behind a signer's authorization. A
/// transaction loaded in its place meanwhile is left alone.
pub fn rebuild_loaded(session: Session) {
    let form = session.forms.sign;
    let (mut error, mut note, mut wire) = (form.error, form.note, form.wire);
    let mut rebuilding = form.rebuilding;
    let Some(loaded) = form.wire.peek().clone() else {
        return;
    };
    if *rebuilding.peek() {
        return;
    }
    error.set(None);
    rebuilding.set(true);
    spawn(async move {
        let result = rebuild_for_wallets(session, loaded.clone()).await;
        rebuilding.set(false);
        if form.wire.peek().as_ref() != Some(&loaded) {
            return;
        }
        match result {
            Ok((rebuilt, Some(adapted), height)) => {
                wire.set(Some(rebuilt));
                let mut marked = form.rebuilt;
                marked.set(true);
                form.set_vector_context(adapted.context.clone());
                form.set_exact_blockhash_height(height);
                note.set(Some(rebuilt_note(&adapted)));
            }
            Ok((_, None, _)) => {}
            Err(err) => error.set(Some(err)),
        }
    });
}

/// What [`refresh_blockhash`] did besides moving to a new blockhash.
pub struct Refreshed {
    pub wire: Vec<u8>,
    /// One-time keys never used on chain, each with the key made here to
    /// take its place. The new key is kept and signs with this device's.
    pub replaced: Vec<(Pubkey, Pubkey)>,
}

/// Move `wire` onto the node's latest blockhash. Signatures over the
/// transaction are cleared, and an authorization inside it is left as it is.
/// Records the exact expiry height on the Sign form.
///
/// A signature this device cannot make again blocks the move, with one
/// exception: a signer the chain has never seen is taken for a one-time key
/// (a new account, made by whoever built the transaction), and a key made
/// here takes its place. The result then has to simulate, or the move is
/// refused. The new key is kept, and signs with the rest of this device's.
pub async fn refresh_blockhash(session: Session, wire: &[u8]) -> Result<Refreshed, String> {
    let view = parse_wire(wire).map_err(sentence)?;
    let from_message = hex::encode(sha256_bytes(&view.message));
    let rpc = session.rpc().map_err(sentence)?;
    let local = session.local_signer_pubkeys();
    let tables = session.forms.sign.tables.peek().clone();
    let tx = decode_on_sign(wire, &tables).map_err(sentence)?;
    let created = accounts_created(Some(rpc.as_ref()), &tx, &local).await;
    let candidates = replaceable_signers(&tx, &local, &created);
    let one_time = one_time_signers(rpc.as_ref(), &candidates)
        .await
        .map_err(sentence)?;
    // An unsigned account the transaction creates that the node has seen is
    // left for whoever made it to sign.
    let signed_elsewhere = tx.signers.iter().filter(|slot| slot.valid);
    if let Some(signer) = signed_elsewhere
        .map(|slot| slot.pubkey)
        .find(|signer| candidates.contains(signer) && !one_time.contains(signer))
    {
        return Err(format!(
            "{signer} signed this transaction on another device and has been used on chain before, so its signature cannot be replaced here. Whoever made the transaction has to sign it again on a fresh blockhash."
        ));
    }
    let fresh: Vec<(Pubkey, KeypairSigner)> = one_time
        .into_iter()
        .map(|signer| (signer, KeypairSigner::new_random()))
        .collect();
    let (hash, last_valid_height) = rpc
        .get_latest_blockhash_with_height()
        .await
        .map_err(sentence)?;
    let swaps: Vec<(Pubkey, Pubkey)> = fresh
        .iter()
        .map(|(old, key)| (*old, key.pubkey()))
        .collect();
    let next = renew(wire, hash, &swaps).map_err(sentence)?;
    if !fresh.is_empty() {
        let renewed = decode_on_sign(&next, &tables).map_err(sentence)?;
        let simulation = wallet_flows::simulate::measure(rpc.as_ref(), &renewed)
            .await
            .map_err(sentence)?;
        if let Some(failure) = simulation.error {
            return Err(reshape_unsupported(&swaps, &failure));
        }
        session.keep_made_here(fresh.iter().map(|(old, key)| (*old, key)));
    }
    session
        .forms
        .sign
        .set_exact_blockhash_height(Some(last_valid_height));
    let flow = session.forms.sign.flow.peek().unwrap_or_else(next_flow_id);
    let cluster = session.cluster();
    TxLog {
        flow,
        wire: &next,
        cluster: Some(&cluster),
    }
    .log_transformed("refresh_blockhash", &from_message);
    Ok(Refreshed {
        wire: next,
        replaced: swaps,
    })
}

/// Take in every object of one transfer, in order: a transaction before the
/// note and signatures sent with it. In a transfer of several, a signature
/// the transaction on screen already holds is passed over: a transaction
/// sent with its own signatures carries each one twice.
pub fn accept_all(session: Session, objects: Vec<QrObject>, from: TxSource) {
    let bundled = objects.len() > 1;
    for object in objects {
        if let QrObject::Signature(signed) = &object {
            if bundled && holds_signature(session, signed) {
                continue;
            }
        }
        accept(session, object, from.clone());
    }
}

/// Whether the transaction on screen already carries `signed`.
fn holds_signature(session: Session, signed: &Signed) -> bool {
    let form = session.forms.sign;
    let tables = form.tables.peek().clone();
    let Some(tx) = form
        .wire
        .peek()
        .as_ref()
        .and_then(|wire| decode_on_sign(wire, &tables).ok())
    else {
        return false;
    };
    let known = form.vector.peek().clone();
    let ctx = AuthContext { vector: &known };
    wallet_core::signature_objects(&tx)
        .into_iter()
        .chain(authority_signature_objects(&tx))
        .chain(vector_signature_objects(&tx, &ctx))
        .any(|held| held.signed == *signed)
}

/// Take in whatever arrived: a transaction replaces the one on screen, a
/// signature or a context note is added to it.
pub fn accept(session: Session, object: QrObject, from: TxSource) {
    let form = session.forms.sign;
    let mut error = form.error;
    let mut note = form.note;
    let mut wire = form.wire;
    let mut import = form.import;
    error.set(None);
    match object {
        QrObject::Transaction(bytes) => {
            let format = match parse_wire(&bytes) {
                Ok(view) => view.format,
                Err(err) => {
                    error.set(Some(sentence(err)));
                    return;
                }
            };
            if from == TxSource::Scanned && format != TxFormat::V1 {
                error.set(Some(
                    "Scanning legacy and v0 transactions is not supported. Paste them instead, or use a v1 QR."
                        .into(),
                ));
                return;
            }
            if format == TxFormat::V1 {
                let what = if from == TxSource::Scanned {
                    "Loaded a transaction from QR."
                } else {
                    "Loaded a transaction."
                };
                form.load(bytes, from, Some(what.into()));
                return;
            }
            // Legacy and v0 take the same route as a browser request: signed
            // as is when only hot keys are missing, otherwise rewritten as v1.
            // A pasted or scanned one has no site to sign after this device.
            let site_signs = matches!(from, TxSource::Browser { .. });
            spawn(async move {
                let source = source_name(&from);
                match prepare_request(
                    session,
                    bytes,
                    Some(ReceivedMeta {
                        source,
                        origin: None,
                        code: None,
                    }),
                    None,
                    site_signs,
                )
                .await
                {
                    Ok(prepared) => {
                        let what = loaded_note(format, prepared.converted.as_ref());
                        form.load_prepared_flow(
                            prepared.wire,
                            from,
                            Some(what.into()),
                            prepared.last_valid_height.map(BlockhashExpiry::exact),
                            prepared.tables,
                            Vec::new(),
                            Some(prepared.flow),
                        );
                    }
                    Err(err) => error.set(Some(err)),
                }
            });
        }
        QrObject::Signature(signed) => {
            let Some(current) = form.wire.peek().clone() else {
                error.set(Some(
                    "Load the transaction first, then the signatures for it.".into(),
                ));
                return;
            };
            let known = form.vector.peek().clone();
            let ctx = AuthContext { vector: &known };
            match accept_signature_in(&current, &signed, &ctx) {
                Ok((next, used)) => {
                    let who = Pubkey::new_from_array(match signed {
                        Signed::Transaction { pubkey, .. } => pubkey,
                        Signed::Authorization { authority, .. } => authority,
                        Signed::AccountAuthorization { account, .. } => account,
                    });
                    let flow = form.flow.peek().unwrap_or_else(next_flow_id);
                    let cluster = session.cluster();
                    TxLog {
                        flow,
                        wire: &next,
                        cluster: Some(&cluster),
                    }
                    .log_signed("imported");
                    wire.set(Some(next));
                    import.set(String::new());
                    note.set(Some(match used {
                        SignatureUse::Transaction => {
                            Phrase::default().t("Added the signature of ").a(who).t(".")
                        }
                        SignatureUse::Authorization { dropped: 0 } => Phrase::default()
                            .t("Added the authorization signed by ")
                            .a(who)
                            .t("."),
                        SignatureUse::Authorization { dropped } => Phrase::default()
                            .t("Added the authorization signed by ")
                            .a(who)
                            .t(format!(
                                ". That changed the transaction, so {dropped} signature(s) made over it earlier were removed and have to be made again."
                            )),
                    }));
                }
                Err(err) => error.set(Some(sentence(err))),
            }
        }
        QrObject::Context(context) => attach_context(session, context),
        QrObject::PublicKey { scheme, key } => match session.accept_next_public_key(scheme, key) {
            Ok(label) => {
                import.set(String::new());
                note.set(Some(
                    format!(
                        "Stored the next public key on {label}. Its account moves to it when a rebuild has to rotate."
                    )
                    .into(),
                ));
            }
            Err(err) => error.set(Some(sentence(err))),
        },
        QrObject::AccountState(state) => match session.adopt_account_state(state) {
            Ok(adopted) => {
                import.set(String::new());
                note.set(Some(
                    match adopted.index {
                        Some(index) => format!(
                            "Took the state of the account of {}. It holds key number {index} of its recovery phrase, which signs for it from now on.",
                            adopted.label
                        ),
                        None => format!(
                            "Took the state of the account of {}: its nonce and the leaves it has used.",
                            adopted.label
                        ),
                    }
                    .into(),
                ));
            }
            Err(err) => error.set(Some(err)),
        },
        QrObject::KeyRotation(rotation) => {
            import.set(String::new());
            spawn(async move {
                if let Err(err) = crate::actions::wallets::send_rotation(session, rotation).await {
                    error.set(Some(err));
                }
            });
        }
        QrObject::WalletKeys(_) => error.set(Some(
            "These are the keys of wallets on another device. Add them on Wallets: Add wallet, then From another device."
                .into(),
        )),
    }
}

fn loaded_note(format: TxFormat, converted: Option<&Converted>) -> String {
    let name = if format == TxFormat::Legacy {
        "legacy"
    } else {
        "v0"
    };
    let Some(converted) = converted else {
        return format!("Loaded a {name} transaction. Only hot keys on this device sign it.");
    };
    let mut text = format!(
        "Loaded a {name} transaction and rewrote it as v1, with every account listed, so another device can read it. The blockhash and the price per compute unit are the ones it came with. {}",
        converted.limit_from.describe(converted.cu_limit)
    );
    for (old, new) in &converted.replaced {
        text.push_str(&format!(
            " {old} was never used on chain, so it was a one-time key: {new}, made on this device, takes its place and signs with this device's keys."
        ));
    }
    if converted.voided > 0 {
        text.push_str(&format!(
            " {} signature(s) it held no longer apply.",
            converted.voided
        ));
    }
    text
}

/// Decode the wire on the Sign screen with its stored lookup tables.
pub fn decode_on_sign(wire: &[u8], tables: &LookupTables) -> Result<DecodedTx, DecodeError> {
    decode_transaction(wire, tables)
}

pub fn attach_context(session: Session, context: TxContext) {
    let form = session.forms.sign;
    let mut error = form.error;
    let mut note = form.note;
    let mut stored = form.context;
    let mut import = form.import;
    let tables = form.tables.peek().clone();
    let matches = form
        .wire
        .peek()
        .as_ref()
        .and_then(|wire| decode_on_sign(wire, &tables).ok())
        .is_some_and(|tx| wallet_core::context_is_for(&context, &tx));
    if !matches {
        error.set(Some(
            "This context note is for a different transaction.".into(),
        ));
        return;
    }
    let pins = context.pins.len();
    let authorizations = context.authorizations.len();
    // A note's authorizations overlay what a rebuild already knew: same
    // account keeps its key from the rebuild, and its scheme, which neither
    // the rebuild nor a wallet here lets the note change.
    let mut known = form.vector.peek().clone();
    let wallets = session.wallets.peek().clone();
    let recorded = |account: &Pubkey| {
        wallets
            .summaries
            .iter()
            .filter(|wallet| wallet.address == Some(*account))
            .find_map(|wallet| wallet.contract.as_ref()?.vector_settings())
            .map(|settings| settings.scheme)
    };
    if let Err(err) = overlay_note(&mut known, &context.authorizations, recorded) {
        error.set(Some(err.to_string()));
        return;
    }
    form.set_vector_context(known);
    stored.set(Some(context));
    import.set(String::new());
    let auth = match authorizations {
        0 => String::new(),
        1 => " 1 Vector authorization.".into(),
        n => format!(" {n} Vector authorizations."),
    };
    note.set(Some(
        format!(
            "Context note attached. {pins} program pins.{auth} Signing still covers the transaction only."
        )
        .into(),
    ));
}

/// Ask the node what `tx` would do. The answer is dropped if another
/// transaction is on screen by the time it arrives. A failure of a
/// transaction a key made here signs in place of a site's one-time key says
/// that reshaping it is not supported.
///
/// The probe runs at the highest compute limit so a rebuild's earlier
/// estimate cannot cut it short, then both limits are written back from what
/// the node used. Authorization signatures stay on the wire. A signature over
/// the transaction itself is left alone: fitting would void it. Legacy and v0
/// are simulated as they are.
pub fn start_simulation(session: Session, tx: DecodedTx) {
    let form = session.forms.sign;
    let mut state = form.simulation;
    let mut error = form.error;
    let mut wire_signal = form.wire;
    let replaced = form.made_here_in(&tx);
    let mut marked = form.simulated_for;
    let digest = sha256_bytes(&tx.message);
    marked.set(Some(digest));

    if !matches!(session.plan_adaptation(&tx), Ok(None)) {
        state.set(SimState::Waiting(
            "Simulation runs once the transaction is rebuilt for the wallet it asks to sign."
                .into(),
        ));
        return;
    }
    let known = form.vector.peek().clone();
    let ctx = AuthContext { vector: &known };
    if authority_slots_in(&tx, &ctx)
        .iter()
        .any(|slot| !slot.signed)
    {
        state.set(SimState::Waiting(
            "Simulation runs once the authorization is signed.".into(),
        ));
        return;
    }
    state.set(SimState::Running);
    let rpc = session.rpc();
    // Only v1 states its limits in a header that fitting can rewrite. Legacy
    // and v0 are signed as they came, and a v0 wire needs the tables the
    // decoded transaction already holds, so they simulate as they are.
    let current_wire = wire_signal
        .peek()
        .clone()
        .filter(|_| tx.format == TxFormat::V1);
    spawn(async move {
        let mut answer_for = digest;
        let result = match (rpc, current_wire) {
            (Ok(rpc), Some(wire)) => {
                match wallet_flows::adapt::simulate_fitting(rpc.as_ref(), &wire).await {
                    Ok((fitted, simulation)) => {
                        if fitted != wire {
                            if let Ok(fitted_tx) = decode_v1_transaction(&fitted) {
                                answer_for = sha256_bytes(&fitted_tx.message);
                                marked.set(Some(answer_for));
                            }
                            wire_signal.set(Some(fitted));
                        }
                        Ok(simulation)
                    }
                    Err(err) => Err(err.to_string()),
                }
            }
            (Ok(rpc), None) => wallet_flows::simulate::simulate(rpc.as_ref(), &tx)
                .await
                .map_err(|err| err.to_string()),
            (Err(err), _) => Err(err.to_string()),
        };
        if *marked.peek() != Some(answer_for) {
            return;
        }
        if let (Ok(simulation), false) = (&result, replaced.is_empty()) {
            if let Some(failure) = &simulation.error {
                error.set(Some(reshape_unsupported(&replaced, failure)));
            }
        }
        state.set(match result {
            Ok(simulation) => SimState::Done(simulation),
            Err(err) => SimState::Failed(err),
        });
    });
}

/// What became of a transaction this device finished signing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandBack {
    /// No site is waiting for it.
    Nothing,
    /// It went back to the site, with the rest of its batch when it had one.
    Returned { count: usize },
    /// It waits with its batch for the others, and transaction `next` of it
    /// is on screen now.
    Kept {
        signed: usize,
        of: usize,
        next: usize,
    },
}

impl HandBack {
    /// What to tell the user, after what signing said.
    pub fn note(self) -> Option<String> {
        match self {
            Self::Nothing => None,
            Self::Returned { count: 1 } => Some("Returned it to the browser.".into()),
            Self::Returned { count } => Some(format!(
                "Returned all {count} transactions to the browser."
            )),
            Self::Kept { signed, of, next } => Some(format!(
                "{signed} of {of} signed. The site gets them back together once every one is, so this is transaction {} now.",
                next + 1
            )),
        }
    }
}

/// Give a signed transaction back to the site that asked for it, when its
/// request is still waiting. One of a batch is kept until every transaction
/// of the batch is signed, and the screen moves on to the next one; the last
/// sends them all.
pub fn return_to_browser(session: &Session, wire: &[u8]) -> HandBack {
    let form = session.forms.sign;
    let Some(TxSource::Browser { id, origin }) = form.source.peek().clone() else {
        return HandBack::Nothing;
    };
    if session.bridge.peek().get(&id).is_none() {
        return HandBack::Nothing;
    }
    let code = bridge_protocol::request_code(&id);
    let cluster = session.cluster();
    let batch = form.batch.peek().clone().filter(|batch| batch.id == id);
    let Some(mut batch) = batch else {
        let flow = form.flow.peek().unwrap_or_else(next_flow_id);
        TxLog {
            flow,
            wire,
            cluster: Some(&cluster),
        }
        .log_returned(&origin, &code);
        if !session.bridge_answer(&id, BridgeAnswer::Signed(vec![wire.to_vec()])) {
            return HandBack::Nothing;
        }
        let mut marked = form.returned;
        marked.set(true);
        return HandBack::Returned { count: 1 };
    };

    let current = batch.current;
    if let Some(item) = batch.items.get_mut(current) {
        item.wire = wire.to_vec();
        item.vector = form.vector.peek().clone();
        item.expiry = *form.clock.expiry.peek();
        item.signed = true;
    }
    if let Some(next) = batch.next_unsigned() {
        let (signed, of) = (batch.signed(), batch.len());
        batch.current = next;
        form.load_batch(batch);
        return HandBack::Kept { signed, of, next };
    }
    for item in &batch.items {
        TxLog {
            flow: item.flow,
            wire: &item.wire,
            cluster: Some(&cluster),
        }
        .log_returned(&origin, &code);
    }
    let wires: Vec<Vec<u8>> = batch.items.iter().map(|item| item.wire.clone()).collect();
    let count = wires.len();
    let mut kept = form.batch;
    kept.set(Some(batch));
    if !session.bridge_answer(&id, BridgeAnswer::Signed(wires)) {
        return HandBack::Nothing;
    }
    let mut marked = form.returned;
    marked.set(true);
    HandBack::Returned { count }
}

/// Take the batch transaction on screen as it is, with nothing for this
/// device to sign in it, and go on to the next.
pub fn keep_batch_item(session: Session) {
    let form = session.forms.sign;
    let Some(wire) = form.wire.peek().clone() else {
        return;
    };
    let handed = return_to_browser(&session, &wire);
    let mut error = form.error;
    let mut note = form.note;
    match handed.note() {
        Some(text) => note.set(Some(text.into())),
        None => error.set(Some("The site stopped waiting for this request.".into())),
    }
}

/// Paste an EIP-191 `personal_sign` result for `account` into the Advance.
pub fn apply_personal_sign(session: Session, account: Pubkey, pasted: String) {
    let form = session.forms.sign;
    let mut error = form.error;
    let mut note = form.note;
    let mut wire = form.wire;
    let Some(current) = form.wire.peek().clone() else {
        return;
    };
    let known = form.vector.peek().clone();
    let recorded = session.vector_recorded_key(account);
    let ctx = AuthContext { vector: &known };
    match insert_personal_sign(&current, &account, &pasted, &ctx, recorded.as_deref()) {
        Ok(authorized) => {
            let flow = form.flow.peek().unwrap_or_else(next_flow_id);
            let cluster = session.cluster();
            TxLog {
                flow,
                wire: &authorized.wire,
                cluster: Some(&cluster),
            }
            .log_signed("pasted");
            wire.set(Some(authorized.wire));
            note.set(Some(
                Phrase::default()
                    .t("Added the personal_sign authorization for ")
                    .a(account)
                    .t("."),
            ));
            error.set(None);
        }
        Err(err) => error.set(Some(sentence(err))),
    }
}

/// Whether a Sign press first moves the transaction to a fresh blockhash
/// with [`refresh_blockhash`]. A relayed transaction is moved inside
/// `wallet_flows::sign` and is not one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockhashOnSign {
    /// Signed on the blockhash it has.
    Keep,
    /// Moved when the node answers, signed as it is when it does not.
    Fresh,
    /// The blockhash has expired: a press that cannot move it signs nothing.
    Required,
}

/// Sign what is loaded with this device's keys, once the vault is unlocked.
pub fn sign_loaded(session: Session, busy: Signal<bool>, blockhash: BlockhashOnSign) {
    if session.is_initialized() && !session.is_unlocked() {
        session.when_unlocked("Unlock the vault to sign with its keys.", move || {
            sign_unlocked(session, busy, blockhash)
        });
        return;
    }
    sign_unlocked(session, busy, blockhash);
}

fn sign_unlocked(mut session: Session, mut busy: Signal<bool>, blockhash: BlockhashOnSign) {
    let form = session.forms.sign;
    let mut error = form.error;
    let mut note = form.note;
    let mut stored = form.wire;
    let Some(loaded) = form.wire.peek().clone() else {
        return;
    };
    error.set(None);
    busy.set(true);
    // Without a network, the transaction is signed on the blockhash it has.
    let rpc = session.rpc().ok();
    let told = form.vector.peek().clone();
    let tables = form.tables.peek().clone();
    spawn(async move {
        // The move is kept only with the signatures made on it, so a
        // simulation started in between cannot write over them.
        let mut moved = None;
        let wire = match blockhash {
            BlockhashOnSign::Keep => loaded,
            _ => match refresh_blockhash(session, &loaded).await {
                Ok(refreshed) => {
                    let cleared = decode_on_sign(&loaded, &tables)
                        .map(|tx| tx.signers.iter().filter(|slot| slot.valid).count())
                        .unwrap_or(0);
                    moved = Some((refreshed.replaced, cleared));
                    refreshed.wire
                }
                Err(err) if blockhash == BlockhashOnSign::Required => {
                    error.set(Some(err));
                    busy.set(false);
                    return;
                }
                Err(_) => loaded,
            },
        };
        // Online, what a hash-based key signs is recorded against the
        // accounts as the chain has them now, not as a note said.
        let (known, known_is_current) = match (&rpc, decode_v1_transaction(&wire)) {
            (Some(rpc), Ok(tx)) => {
                match wallet_flows::vector::read_context(rpc.as_ref(), &tx, &told).await {
                    Ok(read) => (read, true),
                    Err(_) => (told, false),
                }
            }
            _ => (told, false),
        };
        let signers = session.local_signers();
        let signers: Vec<&dyn WalletSigner> = signers
            .iter()
            .map(|signer| signer as &dyn WalletSigner)
            .collect();
        let vector_keys = match session.vector_keys_for_sign(&wire, &signers, &known) {
            Ok(keys) => keys,
            Err(err) => {
                error.set(Some(sentence(err)));
                busy.set(false);
                return;
            }
        };
        let mut cache = session.xmss_retries.peek().clone();
        let mut vector = VectorSigning {
            keys: &vector_keys,
            known: &known,
            cache: Some(&mut cache),
            known_is_current,
        };
        let node = rpc.as_deref();
        match wallet_flows::sign::sign_with(node, &wire, &signers, &mut vector).await {
            Ok(mut done) => {
                session.xmss_retries.set(cache);
                done.refreshed |= moved.is_some();
                let flow = form.flow.peek().unwrap_or_else(next_flow_id);
                let cluster = session.cluster();
                let by = if done.authorizations > 0 && done.signatures == 0 {
                    "Vector key"
                } else {
                    "hot key"
                };
                TxLog {
                    flow,
                    wire: &done.wire,
                    cluster: Some(&cluster),
                }
                .log_signed(by);
                let text = done.note();
                if let Some(height) = done.last_valid_height {
                    form.set_exact_blockhash_height(Some(height));
                }
                let mut phrase = Phrase::text(text);
                if let Some((replaced, cleared)) = moved {
                    if cleared > 0 {
                        phrase = phrase.t(format!(
                            " {cleared} signature(s) made on the old blockhash were cleared and have to be made again."
                        ));
                    }
                    for (old, new) in replaced {
                        phrase = phrase
                            .t(" ")
                            .a(old)
                            .t(" was never used on chain, so it was a one-time key: ")
                            .a(new)
                            .t(", made on this device, takes its place and signs with this device's keys, and the transaction still simulates. The account is created at the new address.");
                    }
                }
                let finished = done.authorized && !done.transaction_next;
                stored.set(Some(done.wire.clone()));
                // The site gets it back once this device has nothing left
                // to sign: not between the authorization and the transaction.
                let handed = if finished {
                    return_to_browser(&session, &done.wire)
                } else {
                    HandBack::Nothing
                };
                match handed {
                    // The next transaction of the batch is on screen, with
                    // its own note. What signing said is about the last one.
                    HandBack::Kept { .. } => {
                        let mut next = Phrase::text(handed.note().unwrap_or_default());
                        if let Some(own) = note.peek().clone() {
                            next = next.t(" ");
                            next.0.extend(own.0);
                        }
                        note.set(Some(next));
                    }
                    _ => {
                        if let Some(text) = handed.note() {
                            phrase = if phrase.plain().is_empty() {
                                Phrase::text(text)
                            } else {
                                phrase.t(format!(" {text}"))
                            };
                        }
                        note.set(Some(phrase));
                    }
                }
            }
            Err(err) => error.set(Some(sentence(err))),
        }
        busy.set(false);
    });
}

pub fn submit_loaded(session: Session, mut busy: Signal<bool>) {
    let form = session.forms.sign;
    let mut error = form.error;
    let mut last = form.last_signature;
    let Some(wire) = form.wire.peek().clone() else {
        return;
    };
    let flow = form.flow.peek().unwrap_or_else(next_flow_id);
    let cluster = session.cluster();
    let last_valid_height = form
        .clock
        .expiry
        .peek()
        .as_ref()
        .map(|expiry| expiry.last_valid_height);
    error.set(None);
    busy.set(true);
    let rpc = session.rpc();
    let submitter = session.submitter();
    spawn(async move {
        let submitted = match rpc {
            Ok(rpc) => {
                submitter
                    .submit(rpc, &wire, flow, "Sign", &cluster, last_valid_height)
                    .await
            }
            Err(err) => Err(err),
        };
        match submitted {
            // The toast follows the transaction from here.
            Ok(signature) => {
                last.set(Some(signature.to_string()));
                form.clear();
            }
            Err(err) => {
                error.set(Some(sentence(err)));
            }
        }
        busy.set(false);
    });
}
