//! The methods of the intents prototype (`docs/intents.md`, "Airlock's
//! prototype: the bridge messages"). None of them waits for the person
//! before it answers: a `resolve` joins the queue and is answered at once
//! with its resolution, and what the person decides reaches the site as a
//! `resolutionChanged` push. Only a `resolveMessage` signed with a key waits,
//! as `signMessage` does.

use std::collections::BTreeMap;
use std::str::FromStr;

use app_core::bridge::{BridgeRequest, ResolveCall, SiteCall, SiteSign, SiteTransaction, SiteWire};
use app_core::resolutions::{ResolutionRefused, Resolutions};
use app_core::PublicCore;
use base64::Engine;
use bridge_host::relay::Answer;
use bridge_protocol::reply;
use serde_json::Value;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use wallet_core::intent::{
    carried_transactions, check_notes, memo_format, memo_text, memo_transaction, plan_resolution,
    valid_id, Delivery, Execution, MessageMode, NoteError, Notes, Order, Payer, ProfileError,
    Requirement, Resolution, ResolveInput, SigningProfile, Timing, Unsupported,
};
use wallet_core::{decode_transaction, lookup_tables_needed, parse_wire, LookupTables};

use super::{
    asked_of_this_device, read_message_request, wait_for_user, Waiting, BLOCKED, DECISION_TIMEOUT,
    MAX_TRANSACTIONS, NOT_CONNECTED, NOT_GIVEN_TO_SITE,
};

/// The methods, as the extension names them (`INTENT_METHODS` in
/// `extension/src/protocol.ts`).
pub(super) const INTENT_METHODS: [&str; 6] = [
    "getSigningProfile",
    "resolve",
    "getResolution",
    "provideSignatures",
    "cancelResolution",
    "resolveMessage",
];

/// The chains a request may name.
const CHAINS: [&str; 3] = ["solana:mainnet", "solana:devnet", "solana:testnet"];

pub(super) fn is_intent_method(method: &str) -> bool {
    INTENT_METHODS.contains(&method)
}

/// Whether `method` can wait for the person: a `resolveMessage` signed with
/// a key. Every other intent method is answered at once.
#[cfg(test)]
pub(super) fn may_wait(method: &str) -> bool {
    method == "resolveMessage"
}

/// Why a request is refused, as its error body says it.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    code: &'static str,
    message: String,
    requirement: Option<&'static str>,
}

impl Refusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            requirement: None,
        }
    }

    fn reply(&self, id: &str) -> Vec<u8> {
        reply::error_with_requirement(id, self.code, &self.message, self.requirement)
    }
}

impl From<Unsupported> for Refusal {
    fn from(unsupported: Unsupported) -> Self {
        Self {
            code: unsupported.code(),
            message: unsupported.message,
            requirement: Some(unsupported.requirement.name()),
        }
    }
}

impl From<NoteError> for Refusal {
    fn from(err: NoteError) -> Self {
        Self::new(err.code(), err.to_string())
    }
}

impl From<app_core::resolutions::ResolutionCommandError> for Refusal {
    fn from(err: app_core::resolutions::ResolutionCommandError) -> Self {
        Self::new(err.code(), err.to_string())
    }
}

fn rejected(message: &str) -> Refusal {
    Refusal::new("rejected", message)
}

fn bad_request(message: impl Into<String>) -> Refusal {
    Refusal::new("bad_request", message)
}

fn unsupported(requirement: Requirement, message: impl Into<String>) -> Refusal {
    Unsupported::new(requirement, message).into()
}

/// What becomes of intent request `method` from `origin`.
pub(super) fn answer(
    core: &PublicCore,
    method: &str,
    id: String,
    origin: String,
    params: &Value,
    waiting: &Waiting,
) -> Answer {
    let now = |result: Result<Vec<u8>, Refusal>| {
        Answer::Now(result.unwrap_or_else(|refusal| {
            tracing::debug!(
                target: "airlock::bridge",
                code = %super::request_code(&id),
                method = %method,
                origin = %origin,
                error = refusal.code,
                reason = %refusal.message,
                "request refused"
            );
            refusal.reply(&id)
        }))
    };
    match method {
        "getSigningProfile" => now(get_signing_profile(core, &id, &origin, params)),
        "resolve" => now(resolve(core, &id, &origin, params)),
        "getResolution" => now(resolution_by_id(core, &id, &origin, params, |core, id| {
            core.resolution(&origin, id)
                .ok_or_else(|| app_core::resolutions::ResolutionCommandError::NotFound.into())
        })),
        "provideSignatures" => now(provide_signatures(core, &id, &origin, params)),
        "cancelResolution" => now(resolution_by_id(core, &id, &origin, params, |core, id| {
            core.resolution_cancel(&origin, id).map_err(Refusal::from)
        })),
        "resolveMessage" => resolve_message(core, id.clone(), origin.clone(), params, waiting)
            .unwrap_or_else(|refusal| now(Err(refusal))),
        _ => now(Err(Refusal::new(
            "not_implemented",
            "Airlock does not know this request.",
        ))),
    }
}

/// The account a request names, when it is connected to `origin`.
fn connected_account(core: &PublicCore, origin: &str, params: &Value) -> Result<Pubkey, Refusal> {
    let account: Pubkey = params
        .get("account")
        .and_then(Value::as_str)
        .and_then(|account| account.parse().ok())
        .ok_or_else(|| rejected(NOT_CONNECTED))?;
    match core.site_sign(origin, &[account]) {
        SiteSign::Blocked => Err(rejected(BLOCKED)),
        SiteSign::NotConnected => Err(rejected(NOT_CONNECTED)),
        SiteSign::Allowed => Ok(account),
    }
}

/// Refused when the site is blocked: a request that names no account.
fn not_blocked(core: &PublicCore, origin: &str) -> Result<(), Refusal> {
    if core.site_sign(origin, &[]) == SiteSign::Blocked {
        Err(rejected(BLOCKED))
    } else {
        Ok(())
    }
}

/// How `account` signs, and the payer of its route.
fn profile_of(
    core: &PublicCore,
    account: Pubkey,
) -> Result<(SigningProfile, Option<Pubkey>), Refusal> {
    core.resolution_profile(account).map_err(|err| match err {
        ProfileError::NotAWallet(_) => rejected("This account is not a wallet of this device."),
        ProfileError::Route(why) => Refusal::new(
            "unsupported",
            format!("Airlock cannot tell how this account signs: {why}"),
        ),
    })
}

fn get_signing_profile(
    core: &PublicCore,
    id: &str,
    origin: &str,
    params: &Value,
) -> Result<Vec<u8>, Refusal> {
    let account = connected_account(core, origin, params)?;
    let (profile, _) = profile_of(core, account)?;
    let profile =
        serde_json::to_value(profile).map_err(|err| Refusal::new("internal", err.to_string()))?;
    Ok(reply::signing_profile(id, profile))
}

/// A `resolve`, refused in this order: the site is blocked or the account is
/// not connected, or the transactions ask another wallet of this device
/// (`rejected`); the request cannot be read, or holds no transaction
/// (`bad_request`) or too many (`not_implemented`); a note cannot be read or
/// fails a check that needs no node (`bad_notes`); the account cannot do
/// what it needs (`unsupported`); the site used its id before
/// (`duplicate_id`). Then it joins the queue, as the queue's limits allow.
fn resolve(core: &PublicCore, id: &str, origin: &str, params: &Value) -> Result<Vec<u8>, Refusal> {
    let account = connected_account(core, origin, params)?;
    let chain = params
        .get("chain")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let read: Vec<SiteTransaction> = params
        .get("transactions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| site_transaction(account, item, chain.clone()))
                .collect()
        })
        .unwrap_or_default();
    // The account a site names is only what it says. This is what its
    // transactions really ask of this device.
    let asked = asked_of_this_device(core, &read);
    if core.site_sign(origin, &asked) != SiteSign::Allowed {
        return Err(rejected(NOT_GIVEN_TO_SITE));
    }

    let (input, notes) = read_input(params)?;
    if input.id.as_deref().is_some_and(|id| !valid_id(id)) {
        return Err(bad_request("An id is 1 to 80 letters, digits and dashes."));
    }
    let count = input.transactions.len();
    if count == 0 {
        return Err(bad_request("No transaction was included."));
    }
    if count > MAX_TRANSACTIONS {
        return Err(Refusal::new(
            "not_implemented",
            format!("Airlock signs at most {MAX_TRANSACTIONS} transactions per request."),
        ));
    }
    let mut formats = Vec::with_capacity(count);
    for (index, wire) in input.transactions.iter().enumerate() {
        let view = parse_wire(wire).map_err(|_| {
            bad_request(format!(
                "Transaction {index} could not be read as a Solana transaction."
            ))
        })?;
        formats.push(view.format);
    }

    let notes = notes
        .map_err(|err| Refusal::new("bad_notes", format!("The notes could not be read: {err}")))?;
    let notes_checked = check_what_can_be_checked(&input.transactions, &notes, &account)?;

    let (profile, payer) = profile_of(core, account)?;
    if !CHAINS.contains(&input.chain.as_str()) {
        return Err(unsupported(
            Requirement::Chain,
            format!(
                "Airlock resolves requests on {}, not on {}.",
                CHAINS.join(", "),
                input.chain
            ),
        ));
    }
    let plan = plan_resolution(&notes, &profile, input.delivery, input.order, count, payer)?;
    // A multisig signs for a new account in the one proposal that stores
    // it. Which proposal that is needs a node, and is found when the
    // request is prepared.
    carried_transactions(&input.transactions, &plan.carried)?;

    let resolution_id = input.id.clone().unwrap_or_else(|| made_id(id));
    let transactions = input
        .transactions
        .iter()
        .zip(formats)
        .map(|(wire, format)| SiteTransaction {
            account,
            wire: SiteWire::Readable(format, wire.clone()),
            chain: Some(input.chain.clone()),
            commitment: None,
        })
        .collect();
    let mut resolution = Resolution::new(
        resolution_id.clone(),
        account,
        input.chain.clone(),
        profile.timing,
        input.delivery,
        count,
        now_secs(core),
    );
    resolution.order = input.order;
    let call = ResolveCall {
        resolution: resolution_id,
        account,
        chain: input.chain,
        transactions,
        notes,
        order: input.order,
        delivery: input.delivery,
        profile,
        plan,
        notes_checked,
        message: None,
    };
    let made = create(core, id, origin, call, resolution)?;
    Ok(reply::resolution(id, made.public()))
}

/// The request, and its notes apart, so a note that cannot be read is
/// `bad_notes` and the rest that cannot be read is `bad_request`.
fn read_input(params: &Value) -> Result<(ResolveInput, Result<Notes, serde_json::Error>), Refusal> {
    let mut rest = params
        .as_object()
        .cloned()
        .ok_or_else(|| bad_request("The request could not be read."))?;
    let mut notes = serde_json::Map::new();
    for key in ["accounts", "data"] {
        if let Some(value) = rest.remove(key) {
            notes.insert(key.to_owned(), value);
        }
    }
    let input: ResolveInput = serde_json::from_value(Value::Object(rest))
        .map_err(|err| bad_request(format!("The request could not be read: {err}")))?;
    Ok((input, serde_json::from_value(Value::Object(notes))))
}

/// Check the notes as far as can be done without a node. A v0 transaction
/// that reads lookup tables cannot be read whole before they are, so then
/// only what needs no transaction is checked here, and the rest when the
/// request is prepared. Answers whether every check ran.
fn check_what_can_be_checked(
    wires: &[Vec<u8>],
    notes: &Notes,
    account: &Pubkey,
) -> Result<bool, Refusal> {
    let reads_tables = wires
        .iter()
        .any(|wire| lookup_tables_needed(wire).is_ok_and(|tables| !tables.is_empty()));
    if reads_tables {
        if notes.accounts.get(account).is_some() {
            return Err(NoteError::OnAccount { address: *account }.into());
        }
        return Ok(false);
    }
    let decoded = wires
        .iter()
        .enumerate()
        .map(|(index, wire)| {
            decode_transaction(wire, &LookupTables::new()).map_err(|err| {
                bad_request(format!("Transaction {index} could not be read: {err}."))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    check_notes(&decoded, notes, account)?;
    Ok(true)
}

/// Keep the resolution and queue its request.
fn create(
    core: &PublicCore,
    id: &str,
    origin: &str,
    call: ResolveCall,
    resolution: Resolution,
) -> Result<Resolution, Refusal> {
    let request = BridgeRequest {
        id: id.to_owned(),
        origin: origin.to_owned(),
        call: SiteCall::Resolve(Box::new(call)),
    };
    core.resolution_create(request, resolution)
        .map_err(|refused| match refused {
            ResolutionRefused::DuplicateId => Refusal::new(
                "duplicate_id",
                "This site used this id before. Give each request an id of its own.",
            ),
            ResolutionRefused::Full => Refusal::new(
                "too_many_requests",
                "Too many requests are waiting in Airlock.",
            ),
            ResolutionRefused::SameRequest => {
                bad_request("A request with this id is already waiting.")
            }
        })
}

/// An id for a request that came with none: the bridge's own id for it,
/// which the extension made and is unique, when it is one a site could have
/// written.
fn made_id(request_id: &str) -> String {
    if valid_id(request_id) {
        request_id.to_owned()
    } else {
        let digest: String = request_id
            .bytes()
            .filter(u8::is_ascii_alphanumeric)
            .take(64)
            .map(char::from)
            .collect();
        format!("r-{digest}")
    }
}

/// One transaction of a `resolve` as far as it can be read now, for the
/// check of what it asks this device.
fn site_transaction(account: Pubkey, item: &Value, chain: Option<String>) -> SiteTransaction {
    let wire = match item.as_str() {
        None => SiteWire::Missing,
        Some(text) => match base64::engine::general_purpose::STANDARD.decode(text) {
            Err(_) => SiteWire::NotBase64,
            Ok(bytes) => match parse_wire(&bytes).map(|view| view.format) {
                Ok(format) => SiteWire::Readable(format, bytes),
                Err(_) => SiteWire::Unreadable(bytes.len()),
            },
        },
    };
    SiteTransaction {
        account,
        wire,
        chain,
        commitment: None,
    }
}

/// `getResolution` and `cancelResolution`: `act` on the resolution the
/// request names.
fn resolution_by_id(
    core: &PublicCore,
    id: &str,
    origin: &str,
    params: &Value,
    act: impl FnOnce(&PublicCore, &str) -> Result<Resolution, Refusal>,
) -> Result<Vec<u8>, Refusal> {
    not_blocked(core, origin)?;
    let resolution_id = params
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request("Name the resolution by its id."))?;
    let resolution = act(core, resolution_id)?;
    Ok(reply::resolution(id, resolution.public()))
}

fn provide_signatures(
    core: &PublicCore,
    id: &str,
    origin: &str,
    params: &Value,
) -> Result<Vec<u8>, Refusal> {
    not_blocked(core, origin)?;
    let resolution_id = params
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request("Name the resolution by its id."))?;
    let items = params
        .get("signatures")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| bad_request("Give at least one signature."))?;
    let mut signatures = Vec::with_capacity(items.len());
    for (at, item) in items.iter().enumerate() {
        let unreadable = || {
            bad_request(format!(
                "signatures[{at}] is not a transaction index, a signer and a signature in base58."
            ))
        };
        let transaction = item
            .get("transaction")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(unreadable)?;
        let signer: Pubkey = item
            .get("signer")
            .and_then(Value::as_str)
            .and_then(|signer| signer.parse().ok())
            .ok_or_else(unreadable)?;
        let signature = item
            .get("signature")
            .and_then(Value::as_str)
            .and_then(|signature| Signature::from_str(signature).ok())
            .ok_or_else(unreadable)?;
        signatures.push((transaction, signer, <[u8; 64]>::from(signature)));
    }
    let resolution = core.resolution_provide_signatures(origin, resolution_id, signatures)?;
    Ok(reply::resolution(id, resolution.public()))
}

/// A `resolveMessage`: signed by a key, which waits for the person as
/// `signMessage` does, or as a memo, answered at once. With no `mode`,
/// `signature` when the profile lists it, otherwise `memo`.
fn resolve_message(
    core: &PublicCore,
    id: String,
    origin: String,
    params: &Value,
    waiting: &Waiting,
) -> Result<Answer, Refusal> {
    let account = connected_account(core, &origin, params)?;
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
        .ok_or_else(|| bad_request("The message was not valid base64."))?;
    let (profile, payer) = profile_of(core, account)?;
    let mode = match params.get("mode").and_then(Value::as_str) {
        None if profile.messages.contains(&MessageMode::Signature) => MessageMode::Signature,
        None => MessageMode::Memo,
        Some("signature") => MessageMode::Signature,
        Some("memo") => MessageMode::Memo,
        Some(_) => return Err(bad_request("A mode is \"signature\" or \"memo\".")),
    };
    if !profile.messages.contains(&mode) {
        let how = match mode {
            MessageMode::Signature => "with a key, since no key signs as it",
            MessageMode::Memo => "as a memo",
        };
        return Err(unsupported(
            Requirement::Messages,
            format!("{account} cannot sign a message {how}."),
        ));
    }
    match mode {
        MessageMode::Signature => {
            let call = read_message_request(core, &origin, params).map_err(|why| rejected(&why))?;
            let request = BridgeRequest { id, origin, call };
            Ok(as_resolved_message(wait_for_user(
                core,
                request,
                DECISION_TIMEOUT,
                waiting,
            )))
        }
        MessageMode::Memo => {
            let resolution = memo(
                core, &id, &origin, params, account, &message, profile, payer,
            )?;
            Ok(Answer::Now(reply::memo_resolution(
                &id,
                resolution.public(),
            )))
        }
    }
}

/// The message signed as a memo: its one transaction, paid by the account's
/// route, queued as a `resolve` delivered `send`.
#[allow(clippy::too_many_arguments)]
fn memo(
    core: &PublicCore,
    id: &str,
    origin: &str,
    params: &Value,
    account: Pubkey,
    message: &[u8],
    profile: SigningProfile,
    payer: Option<Pubkey>,
) -> Result<Resolution, Refusal> {
    let text =
        memo_text(message).map_err(|err| unsupported(Requirement::Messages, err.to_string()))?;
    let resolution_id = match params.get("id") {
        None | Some(Value::Null) => made_id(id),
        Some(Value::String(own)) if valid_id(own) => own.clone(),
        Some(_) => return Err(bad_request("An id is 1 to 80 letters, digits and dashes.")),
    };
    let chain = memo_chain(core, params)?;
    let plan = plan_resolution(
        &Notes::default(),
        &profile,
        Delivery::Send,
        Order::Serial,
        1,
        payer,
    )?;
    // A key on another device pays for itself, signing as it does: its
    // route names no payer on this device.
    let payer = payer.or_else(|| {
        profile
            .fee_payer
            .contains(&Payer::Account)
            .then_some(account)
    });
    let payer = payer.ok_or_else(|| {
        unsupported(
            Requirement::FeePayer,
            format!("Nothing on this device can pay fees for {account}."),
        )
    })?;
    // Legacy only where a key of this device signs at the top, so any node
    // shows the proof; v1 for every other account (`memo_format`). On no
    // real blockhash yet: the request moves onto a fresh one when it is
    // prepared.
    let key_here = profile.timing == Timing::Sync && profile.execution == Execution::TopLevel;
    let format = memo_format(key_here);
    let built = memo_transaction(account, text, payer, None, format)
        .map_err(|err| Refusal::new("internal", err.to_string()))?;
    let resolution = Resolution::for_memo(
        resolution_id.clone(),
        account,
        chain.clone(),
        profile.timing,
        text,
        now_secs(core),
    );
    let call = ResolveCall {
        resolution: resolution_id,
        account,
        chain: chain.clone(),
        transactions: vec![SiteTransaction {
            account,
            wire: SiteWire::Readable(format, built),
            chain: Some(chain),
            commitment: None,
        }],
        notes: Notes::default(),
        order: Order::Serial,
        delivery: Delivery::Send,
        profile,
        plan,
        notes_checked: true,
        message: Some(text.to_owned()),
    };
    create(core, id, origin, call, resolution)
}

/// The chain a memo is for: the one the site names, as in any `resolve`,
/// or with none named the network Airlock is on, when it can name it.
fn memo_chain(core: &PublicCore, params: &Value) -> Result<String, Refusal> {
    match params.get("chain") {
        None | Some(Value::Null) => core
            .slices()
            .network
            .get()
            .cluster
            .standard_chain()
            .map(str::to_owned)
            .ok_or_else(|| {
                unsupported(
                    Requirement::Chain,
                    "Airlock is on a network that is none of the public ones, so a memo that names no chain cannot say which it is on. Name the chain.",
                )
            }),
        Some(Value::String(chain)) if CHAINS.contains(&chain.as_str()) => Ok(chain.clone()),
        Some(Value::String(chain)) => Err(unsupported(
            Requirement::Chain,
            format!(
                "Airlock resolves requests on {}, not on {chain}.",
                CHAINS.join(", ")
            ),
        )),
        Some(_) => Err(bad_request("A chain is a string such as \"solana:devnet\".")),
    }
}

/// A `signMessage` reply as a `resolveMessage` gives it: `kind`, and the
/// signature in base58. A refusal is the same either way.
fn as_resolved_message(answer: Answer) -> Answer {
    match answer {
        Answer::Now(bytes) => Answer::Now(resolved_message(bytes)),
        Answer::Later(wait) => Answer::Later(Box::new(move || resolved_message(wait()))),
    }
}

fn resolved_message(bytes: Vec<u8>) -> Vec<u8> {
    let Ok(reply) = serde_json::from_slice::<Value>(&bytes) else {
        return bytes;
    };
    let id = reply.get("id").and_then(Value::as_str).unwrap_or_default();
    let read = |key: &str| {
        reply
            .get("result")
            .and_then(|result| result.get(key))
            .and_then(Value::as_str)
            .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
    };
    match (read("signedMessage"), read("signature")) {
        (Some(signed), Some(signature)) => match <[u8; 64]>::try_from(signature) {
            Ok(signature) => {
                reply::message_signature(id, &signed, &Signature::from(signature).to_string())
            }
            Err(_) => bytes,
        },
        _ => bytes,
    }
}

/// The clock the core keeps, in Unix seconds.
fn now_secs(core: &PublicCore) -> u64 {
    core.ports().clock.now_ms() / 1000
}

/// What a site may see of each resolution, by its origin and id.
pub(super) fn public_views(resolutions: &Resolutions) -> BTreeMap<(String, String), Value> {
    resolutions
        .items()
        .iter()
        .map(|kept| {
            (
                (kept.origin.clone(), kept.resolution.id.clone()),
                kept.resolution.public(),
            )
        })
        .collect()
}

/// The `resolutionChanged` pushes from `told` to `now`: one for each
/// resolution that is new or changed. One dropped to make room says
/// nothing.
pub(super) fn resolution_changes(
    told: &BTreeMap<(String, String), Value>,
    now: &BTreeMap<(String, String), Value>,
) -> Vec<Vec<u8>> {
    now.iter()
        .filter(|(key, view)| told.get(*key) != Some(*view))
        .map(|((origin, _), view)| reply::resolution_changed(origin, view.clone()))
        .collect()
}
