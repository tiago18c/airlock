//! Prepare a browser sign request for display and action.

use super::*;
use app_ui_kit::state::forms::BlockhashClock;

/// The transaction a sign request will be answered with.
#[derive(Clone, PartialEq)]
pub(in crate::ui) enum Prepared {
    Working,
    Ready {
        wire: Vec<u8>,
        format: wallet_core::TxFormat,
        tables: wallet_core::LookupTables,
        /// Set when the site's transaction was rebuilt for a keyless wallet.
        /// Boxed, with `converted`, so this variant is not far larger than
        /// the others.
        adapted: Option<Box<Adapted>>,
        /// Set when a legacy or v0 wire was rewritten as v1.
        converted: Option<Box<wallet_core::Converted>>,
        /// When this app chose the blockhash, the height it expires at.
        last_valid_height: Option<u64>,
        flow: u64,
    },
    Failed(String),
}

type ReadyWire = (
    Vec<u8>,
    Option<Adapted>,
    Option<u64>,
    wallet_core::TxFormat,
    wallet_core::LookupTables,
    Option<wallet_core::Converted>,
    u64,
);

type SignWire = (
    Vec<u8>,
    wallet_core::LookupTables,
    Vec<wallet_core::relay::VectorContext>,
    u64,
);

/// Values derived from a bridge sign prompt, for the request modal.
pub(in crate::ui) struct PreparedRequest {
    pub account: Option<Pubkey>,
    pub original: Option<Vec<u8>>,
    pub attempt: Signal<u32>,
    pub paid_here: Signal<bool>,
    pub fetching: Signal<bool>,
    pub state: Prepared,
    pub adapted: Option<Adapted>,
    pub converted: Option<wallet_core::Converted>,
    pub offered: Option<PaidHere>,
    pub shown: Option<wallet_core::DecodedTx>,
    pub explained: Option<wallet_core::ExplainedTx>,
    pub locked: bool,
    pub can_review: bool,
    pub can_sign: bool,
    pub verdict: Option<wallet_core::QuickSign>,
    pub step: Option<wallet_core::NextSignature>,
    pub lifetime: Option<wallet_core::TxLifetime>,
    pub modal_expired: bool,
    pub countdown_label: Signal<String>,
    pub clock: BlockhashClock,
    pub clock_expired: Signal<bool>,
    pub for_review: Option<ReadyWire>,
    pub for_sign: Option<SignWire>,
    pub replaced_payer: bool,
    /// Unsigned accounts the site's own transaction creates, such as a new
    /// mint. The site signs for them once it has the transaction back.
    pub site_signs: Vec<Pubkey>,
}

pub(in crate::ui) fn use_prepared_request(
    session: Session,
    current: BridgePrompt,
) -> PreparedRequest {
    let original = crate::bridge::transaction_wire(&current.params);
    let account = crate::bridge::requested_account(&current.params);

    let mut prepared = use_signal(|| Prepared::Working);
    let attempt = use_signal(|| 0u32);
    // Counts preparations, so a slow older one cannot overwrite a newer one.
    let mut turn = use_signal(|| 0u64);
    // A locked vault knows no wallets. Prepare again once it is unlocked.
    let unlocked = use_memo(move || session.is_unlocked());
    let mut offer = use_signal(|| None::<PaidHere>);
    let mut paid_here = use_signal(|| false);

    // One log trail for the request, however often it is prepared again.
    let flow = use_hook(app_core::diagnostics::next_flow_id);
    let mut received_logged = use_hook(|| CopyValue::new(false));

    let source = original.clone();
    let prepare_origin = current.origin.clone();
    let prepare_id = current.id.clone();
    use_effect(move || {
        let _ = (attempt(), unlocked());
        let Some(wire) = source.clone() else {
            return;
        };
        let mine = *turn.peek() + 1;
        turn.set(mine);
        prepared.set(Prepared::Working);
        offer.set(None);
        paid_here.set(false);
        let origin = prepare_origin.clone();
        let request_id = prepare_id.clone();
        // `prepare_request` logs `received` before anything can stop it.
        let log_received = !*received_logged.peek();
        received_logged.set(true);
        spawn(async move {
            let code = crate::bridge::request_code(&request_id);
            let result = prepare_request(
                session,
                wire.clone(),
                log_received.then(|| app_ui_kit::actions::sign::ReceivedMeta {
                    source: "browser",
                    origin: Some(&origin),
                    code: Some(&code),
                }),
                Some(flow),
                true,
            )
            .await;
            if *turn.peek() != mine {
                return;
            }
            let ready = result.is_ok();
            prepared.set(match result {
                Ok(prepared) => Prepared::Ready {
                    wire: prepared.wire,
                    format: prepared.format,
                    tables: prepared.tables,
                    adapted: prepared.adapted.map(Box::new),
                    converted: prepared.converted.map(Box::new),
                    last_valid_height: prepared.last_valid_height,
                    flow: prepared.flow,
                },
                Err(err) => Prepared::Failed(err),
            });
            // Payer swap only after a v1 (or converted) path; as-is legacy/v0
            // keeps the site's fee payer.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(Prepared::Ready {
                format: wallet_core::TxFormat::V1,
                wire: ready_wire,
                flow,
                ..
            }) = ready.then(|| prepared.peek().clone())
            {
                let found = offer_paid_here(session, ready_wire, flow).await;
                if *turn.peek() == mine {
                    offer.set(found);
                }
            }
        });
    });

    // IDLs this device lacks for what is about to be shown. A stored IDL
    // changes the catalog, and the summary below is read again.
    let mut fetched_for = use_signal(|| None::<[u8; 32]>);
    let mut fetching = use_signal(|| false);
    use_effect(move || {
        let Prepared::Ready { wire, tables, .. } = prepared() else {
            return;
        };
        let Ok(tx) = wallet_core::decode_transaction(&wire, &tables) else {
            return;
        };
        let digest = sha256_bytes(&tx.message);
        if *fetched_for.peek() == Some(digest) {
            return;
        }
        fetched_for.set(Some(digest));
        let missing = missing_catalog_programs(&*session.idls.peek(), &tx);
        if missing.is_empty() {
            return;
        }
        fetching.set(true);
        spawn(async move {
            let _ = fetch_missing_idls(&session, &missing).await;
            fetching.set(false);
        });
    });

    let state = prepared();
    let offered = offer();
    let taken = offered.clone().filter(|_| paid_here());
    let ready = match (&state, &taken) {
        (
            Prepared::Ready {
                last_valid_height,
                format,
                tables,
                converted,
                flow,
                ..
            },
            Some(paid),
        ) => Some((
            paid.wire.clone(),
            paid.adapted.clone(),
            *last_valid_height,
            *format,
            tables.clone(),
            converted.as_deref().cloned(),
            *flow,
        )),
        (
            Prepared::Ready {
                wire,
                adapted,
                last_valid_height,
                format,
                tables,
                converted,
                flow,
            },
            None,
        ) => Some((
            wire.clone(),
            adapted.as_deref().cloned(),
            *last_valid_height,
            *format,
            tables.clone(),
            converted.as_deref().cloned(),
            *flow,
        )),
        _ => None,
    };
    let adapted = ready
        .as_ref()
        .and_then(|(_, adapted, _, _, _, _, _)| adapted.clone());
    let converted = ready
        .as_ref()
        .and_then(|(_, _, _, _, _, converted, _)| converted.clone());
    let replaced_payer = taken.is_some();
    // Decoded once per wire, not on every render of the modal.
    let decode_key = ready
        .as_ref()
        .map(|(wire, _, _, _, tables, _, _)| (wire.clone(), tables.clone()));
    let tx = use_memo(use_reactive!(|(decode_key)| {
        decode_key
            .as_ref()
            .and_then(|(wire, tables)| wallet_core::decode_transaction(wire, tables).ok())
    }))();
    let shown = tx.clone();
    let vector_known = adapted
        .as_ref()
        .map(|adapted| adapted.context.clone())
        .unwrap_or_default();
    let schemes = vector_known.clone();
    let explained = use_memo(use_reactive!(|(tx, schemes)| {
        tx.as_ref().map(|tx| {
            let note = wallet_core::DisplayNote::default().with_vector_schemes(&schemes);
            wallet_core::explain_with_note(&*session.idls.read(), tx, &note)
        })
    }))();
    // Preparing may rebuild the request around a wallet's keys, so nothing
    // goes further until the vault is open.
    let locked = session.is_initialized() && !session.is_unlocked();
    let can_review = ready.is_some() && !locked;
    let vector_here = session.local_vector_accounts();
    let local_keys = session.local_signer_pubkeys();
    let local = wallet_core::LocalKeys {
        keys: &local_keys,
        vector: &vector_here,
    };
    let ctx = wallet_core::relay::AuthContext {
        vector: &vector_known,
    };
    let site_signs = shown
        .as_ref()
        .filter(|tx| tx.format != wallet_core::TxFormat::V1)
        .map(|tx| {
            let mut here = session.known_wallet_pubkeys();
            here.extend(local_keys.iter().copied());
            wallet_flows::request::left_for_site(tx, &here, &session.forms.sign.created_in(tx))
        })
        .unwrap_or_default();
    let verdict = match (&shown, &explained) {
        (Some(tx), Some(explained)) => Some(quick_sign_in(tx, &local, &ctx, explained)),
        _ => None,
    };
    let status = shown
        .as_ref()
        .map(|tx| wallet_core::signing_status_in(tx, &local, &ctx));
    let step = status.as_ref().map(wallet_core::next_step);
    let lifetime = shown.as_ref().map(wallet_core::tx_lifetime);
    // The modal's own countdown. A blockhash this app chose (a rebuild) has
    // an exact expiry. A site's is estimated from when the request arrived,
    // and Review hands that estimate on to the Sign screen.
    let clock = use_blockhash_clock_state();
    let prepared_height = ready
        .as_ref()
        .and_then(|(_, _, height, _, _, _, _)| *height);
    use_effect(use_reactive!(|(lifetime, prepared_height)| {
        let _ = lifetime;
        clock.reset(prepared_height.map(BlockhashExpiry::exact));
    }));
    let clock_expired = use_signal(|| false);
    let countdown_label = use_signal(|| String::from("…"));
    let modal_expired = clock_expired()
        && matches!(
            step,
            Some(wallet_core::NextSignature::Transaction | wallet_core::NextSignature::Nothing)
        );
    // A rebuild with a key made here simulates only once its authorization is
    // signed, which Review does before anything is sent.
    let unsimulated = adapted.is_some()
        && converted
            .as_ref()
            .is_some_and(|converted| !converted.replaced.is_empty());
    // An authorization signed here changes what runs, so Review signs it,
    // simulates the transaction again and fits its limits, then signs the
    // transaction on a later press.
    let authorizes_here = status
        .as_ref()
        .is_some_and(|status| status.authorizations_here() > 0);
    let can_sign = can_review
        && !unsimulated
        && !authorizes_here
        && verdict.is_some_and(|verdict| verdict.allowed_leaving(site_signs.len()))
        && !fetching()
        && !modal_expired;

    let for_review = ready.clone();
    let for_sign = ready
        .as_ref()
        .map(|(wire, adapted, _, _, tables, _, flow)| {
            (
                wire.clone(),
                tables.clone(),
                adapted
                    .as_ref()
                    .map(|adapted| adapted.context.clone())
                    .unwrap_or_default(),
                *flow,
            )
        });

    PreparedRequest {
        account,
        original,
        attempt,
        paid_here,
        fetching,
        state,
        adapted,
        converted,
        offered,
        shown,
        explained,
        locked,
        can_review,
        can_sign,
        verdict,
        step,
        lifetime,
        modal_expired,
        countdown_label,
        clock,
        clock_expired,
        for_review,
        for_sign,
        replaced_payer,
        site_signs,
    }
}
