//! The Sign screen's sign step: sign everything this device holds a key for,
//! after moving a relayed transaction to a blockhash that is still alive.

use crate::vector::read_context;
use solana_pubkey::Pubkey;
use wallet_core::relay::{authority_slots_in, with_blockhash, AuthContext, VectorContext};
use wallet_core::vector::VectorKey;
use wallet_core::vector::XmssRetryCache;
use wallet_core::{
    decode_v1_transaction, parse_wire, sign_locally_in_with, signing_status_in, DecodedTx,
    LocalKeys, SignError, SignerKey, SignerRole, SignerState, SolanaRpc, TxFormat, WalletSigner,
};

/// What signing did to the transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub wire: Vec<u8>,
    pub authorizations: usize,
    pub signatures: usize,
    /// Earlier transaction signatures that an authorization made void.
    pub dropped: usize,
    /// The transaction was moved to a fresh blockhash before it was signed.
    pub refreshed: bool,
    /// When `refreshed`, the height at which the new blockhash expires.
    pub last_valid_height: Option<u64>,
    /// No authorization inside the transaction is still waiting for a
    /// signature, so whoever asked for it can have it back.
    pub authorized: bool,
}

impl Signed {
    /// What was done, in sentences for the screen.
    pub fn note(&self) -> String {
        let mut parts = Vec::new();
        match self.authorizations {
            0 => {}
            1 => parts.push("Signed the authorization.".to_string()),
            count => parts.push(format!("Signed the authorization as {count} authorities.")),
        }
        if self.signatures > 0 {
            parts.push(if self.refreshed {
                "Moved the transaction to a fresh blockhash and signed it.".to_string()
            } else {
                "Signed the transaction.".to_string()
            });
        }
        if self.dropped > 0 {
            parts.push(format!(
                "{} signature(s) made over the transaction before the authorization were removed.",
                self.dropped
            ));
        }
        parts.join(" ")
    }
}

/// The Vector accounts a device signs for, and what it was told of them.
#[derive(Default)]
pub struct VectorSigning<'a> {
    /// The keys this device holds.
    pub keys: &'a [VectorKey],
    /// What a context note or a rebuild said of the accounts. Online, the
    /// chain is read over it.
    pub known: &'a [VectorContext],
    /// XMSS retry salts; a second press over the same digest reuses a leaf.
    pub cache: Option<&'a mut XmssRetryCache>,
    /// `known` was just read from the chain, so it is not read again.
    pub known_is_current: bool,
}

/// Whether `tx` should be moved to a fresh blockhash before a key in `local`
/// signs it. That is a relayed transaction nobody has signed yet, which this
/// device is about to sign as a transaction, and which lives on a blockhash.
///
/// A relayed transaction may have waited a long time for its authorization.
/// The authorization does not cover the blockhash, so it survives the move.
pub fn wants_fresh_blockhash(tx: &DecodedTx, local: &[Pubkey]) -> bool {
    let local = LocalKeys {
        keys: local,
        vector: &[],
    };
    fresh_blockhash_for(tx, &local, &AuthContext::default())
}

fn fresh_blockhash_for(tx: &DecodedTx, local: &LocalKeys, ctx: &AuthContext) -> bool {
    let status = signing_status_in(tx, local, ctx);
    let signs_transaction = !status.waiting_for_authority
        && status.lines.iter().any(|line| {
            !matches!(line.role, SignerRole::Authority { .. }) && line.state == SignerState::Local
        });
    let relayed = !authority_slots_in(tx, ctx).is_empty();
    let untouched = !tx.signers.iter().any(|slot| slot.valid);
    relayed && signs_transaction && untouched && !tx.uses_durable_nonce
}

/// Sign everything in `wire` that `signers` can sign: authorizations first,
/// then the transaction itself. `rpc` is the node when the app is online. A
/// node that cannot answer leaves the transaction on the blockhash it has.
pub async fn sign(
    rpc: Option<&dyn SolanaRpc>,
    wire: &[u8],
    signers: &[&dyn WalletSigner],
) -> Result<Signed, SignError> {
    let mut empty = VectorSigning::default();
    sign_with(rpc, wire, signers, &mut empty).await
}

/// [`sign`], also signing for the Vector accounts whose keys `vector` holds.
///
/// Online, each account the transaction's `Advance` lists is read first, so
/// signing uses its nonce as it is now. A signature already in place that no
/// longer verifies, because its account acted since, stops the press before
/// anything else is signed: the transaction has to be rebuilt.
pub async fn sign_with(
    rpc: Option<&dyn SolanaRpc>,
    wire: &[u8],
    signers: &[&dyn WalletSigner],
    vector: &mut VectorSigning<'_>,
) -> Result<Signed, SignError> {
    let mut wire = wire.to_vec();
    let mut refreshed = false;
    let mut last_valid_height = None;
    let decoded = decode_v1_transaction(&wire).ok();
    let mut context = vector.known.to_vec();
    if let (Some(rpc), Some(tx), false) = (rpc, &decoded, vector.known_is_current) {
        // A node that cannot answer leaves what the device was told.
        if let Ok(read) = read_context(rpc, tx, vector.known).await {
            context = read;
        }
    }
    let ctx = AuthContext { vector: &context };
    let held: Vec<Pubkey> = vector.keys.iter().map(|key| key.account).collect();
    let keys: Vec<Pubkey> = signers.iter().map(|signer| signer.pubkey()).collect();
    let local = LocalKeys {
        keys: &keys,
        vector: &held,
    };
    if let Some(tx) = &decoded {
        let stale = signing_status_in(tx, &local, &ctx)
            .lines
            .iter()
            .find(|line| line.state == SignerState::Invalid)
            .and_then(|line| match line.key {
                SignerKey::Authority(authority) => Some(authority.shown_as()),
                SignerKey::Key(_) => None,
            });
        if let Some(account) = stale {
            return Err(SignError::Message(format!(
                "The signature for {account} no longer verifies: it has acted since this was signed, or the transaction changed. Rebuild it."
            )));
        }
    }
    if let (Some(rpc), Some(tx)) = (rpc, &decoded) {
        if fresh_blockhash_for(tx, &local, &ctx) {
            if let Ok((hash, height)) = rpc.get_latest_blockhash_with_height().await {
                if let Ok(next) = with_blockhash(&wire, hash) {
                    refreshed = next != wire;
                    if refreshed {
                        last_valid_height = Some(height);
                    }
                    wire = next;
                }
            }
        }
    }
    let cache = vector.cache.take();
    let done = sign_locally_in_with(&wire, signers, vector.keys, &ctx, cache)?;
    // Only v1 transactions this app built carry authorizations.
    let authorized = match parse_wire(&done.wire) {
        Ok(view) if view.format != TxFormat::V1 => true,
        _ => decode_v1_transaction(&done.wire)
            .map(|tx| authority_slots_in(&tx, &ctx).iter().all(|slot| slot.signed))
            .unwrap_or(false),
    };
    Ok(Signed {
        wire: done.wire,
        authorizations: done.authorizations,
        signatures: done.signatures,
        dropped: done.dropped,
        refreshed,
        last_valid_height,
        authorized,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pk, transfer_instruction, FakeRpc};
    use solana_hash::Hash;
    use wallet_core::programmatic::{programmatic_signer_address, ExecutorNonce};
    use wallet_core::relay::authority_slots;
    use wallet_core::relay::wrap_for_programmatic;
    use wallet_core::signing_status;
    use wallet_core::{compile_v1, KeypairSigner};

    /// The blockhash the fake node answers with.
    const FRESH: Hash = Hash::new_from_array([9; 32]);
    const STALE: Hash = Hash::new_from_array([1; 32]);

    fn blockhash_of(wire: &[u8]) -> Hash {
        decode_v1_transaction(wire).unwrap().lifetime
    }

    fn plain(payer: &KeypairSigner) -> Vec<u8> {
        compile_v1(
            payer.pubkey(),
            vec![transfer_instruction(payer.pubkey(), pk(2))],
            None,
            Some(STALE),
        )
        .unwrap()
        .wire
    }

    /// A transfer out of `authority`'s programmatic signer, relayed by `hot`.
    fn relayed(hot: &KeypairSigner, authority: &KeypairSigner) -> Vec<u8> {
        relayed_for(hot, &[authority])
    }

    /// A transfer out of each authority's programmatic signer, in one
    /// authorization on the first one's nonce, relayed by `hot`.
    fn relayed_for(hot: &KeypairSigner, authorities: &[&KeypairSigner]) -> Vec<u8> {
        let keys: Vec<Pubkey> = authorities.iter().map(|key| key.pubkey()).collect();
        let addresses: Vec<Pubkey> = keys.iter().map(programmatic_signer_address).collect();
        let built = compile_v1(
            hot.pubkey(),
            addresses
                .iter()
                .map(|address| transfer_instruction(*address, pk(2)))
                .collect(),
            None,
            Some(STALE),
        )
        .unwrap();
        wrap_for_programmatic(
            &decode_v1_transaction(&built.wire).unwrap(),
            &keys,
            ExecutorNonce {
                account: pk(9),
                authority: addresses[0],
                value: Hash::new_from_array([3; 32]),
            },
            hot.pubkey(),
            STALE,
        )
        .unwrap()
    }

    fn run(rpc: Option<&FakeRpc>, wire: &[u8], signers: &[&KeypairSigner]) -> Signed {
        try_run(rpc, wire, signers).unwrap()
    }

    fn try_run(
        rpc: Option<&FakeRpc>,
        wire: &[u8],
        signers: &[&KeypairSigner],
    ) -> Result<Signed, SignError> {
        let refs: Vec<&dyn WalletSigner> = signers
            .iter()
            .map(|signer| *signer as &dyn WalletSigner)
            .collect();
        pollster::block_on(sign(rpc.map(|rpc| rpc as &dyn SolanaRpc), wire, &refs))
    }

    #[test]
    fn an_ordinary_transaction_is_signed_on_the_blockhash_it_came_with() {
        let payer = KeypairSigner::new_random();
        let rpc = FakeRpc::default();
        let done = run(Some(&rpc), &plain(&payer), &[&payer]);

        assert_eq!((done.authorizations, done.signatures), (0, 1));
        assert!(!done.refreshed && done.authorized);
        assert_eq!(blockhash_of(&done.wire), STALE);
        assert_eq!(rpc.asked(), 0, "the node is not asked for anything");
        assert_eq!(done.note(), "Signed the transaction.");
    }

    /// A legacy transfer from `payer`, laid out by hand: signature count,
    /// an empty slot, then the message.
    fn legacy(payer: &KeypairSigner) -> Vec<u8> {
        let mut wire = vec![1u8];
        wire.extend_from_slice(&[0u8; 64]);
        wire.extend_from_slice(&[1, 0, 1, 3]);
        for key in [payer.pubkey(), pk(2), wallet_core::SYSTEM_PROGRAM_ID] {
            wire.extend_from_slice(key.as_ref());
        }
        wire.extend_from_slice(STALE.as_ref());
        wire.extend_from_slice(&[1, 2, 2, 0, 1, 12]);
        wire.extend_from_slice(&2u32.to_le_bytes());
        wire.extend_from_slice(&5u64.to_le_bytes());
        wire
    }

    #[test]
    fn a_legacy_transaction_is_signed_in_its_own_format_and_can_go_back() {
        let payer = KeypairSigner::new_random();
        let wire = legacy(&payer);
        assert_eq!(parse_wire(&wire).unwrap().format, TxFormat::Legacy);
        let rpc = FakeRpc::default();
        let done = run(Some(&rpc), &wire, &[&payer]);

        assert_eq!(done.signatures, 1);
        assert_eq!(parse_wire(&done.wire).unwrap().format, TxFormat::Legacy);
        assert!(!done.refreshed);
        assert!(
            done.authorized,
            "nothing is left to authorize, so the site gets it back"
        );
    }

    #[test]
    fn a_relayed_transaction_the_payer_signs_is_moved_to_a_fresh_blockhash_first() {
        let (hot, authority) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        let wire = relayed(&hot, &authority);
        // The authorization arrives first, from the device that holds that key.
        let authorized = run(None, &wire, &[&authority]);
        assert_eq!((authorized.authorizations, authorized.signatures), (1, 0));
        assert!(authorized.authorized && !authorized.refreshed);
        assert_eq!(authorized.note(), "Signed the authorization.");

        let rpc = FakeRpc::default();
        let done = run(Some(&rpc), &authorized.wire, &[&hot]);
        assert_eq!((done.authorizations, done.signatures), (0, 1));
        assert!(done.refreshed);
        assert_eq!(blockhash_of(&done.wire), FRESH);
        assert_eq!(rpc.asked(), 1);
        assert_eq!(
            done.note(),
            "Moved the transaction to a fresh blockhash and signed it."
        );
        // The authorization was not made over the blockhash, so it still stands.
        let tx = decode_v1_transaction(&done.wire).unwrap();
        assert!(authority_slots(&tx).iter().all(|slot| slot.signed));
        assert!(signing_status(&tx, &[]).complete());
    }

    #[test]
    fn one_device_holding_both_keys_signs_both_and_moves_the_blockhash() {
        let (hot, authority) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        let rpc = FakeRpc::default();
        let done = run(Some(&rpc), &relayed(&hot, &authority), &[&hot, &authority]);
        assert_eq!((done.authorizations, done.signatures), (1, 1));
        assert!(done.authorized);
        assert_eq!(blockhash_of(&done.wire), FRESH);
    }

    #[test]
    fn one_device_holding_two_authorities_and_the_payer_signs_everything_in_one_press() {
        let hot = KeypairSigner::new_random();
        let (first, second) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        let rpc = FakeRpc::default();
        let done = run(
            Some(&rpc),
            &relayed_for(&hot, &[&first, &second]),
            &[&hot, &first, &second],
        );
        assert_eq!((done.authorizations, done.signatures), (2, 1));
        assert!(done.authorized && done.refreshed);
        assert_eq!(blockhash_of(&done.wire), FRESH);
        assert_eq!(
            done.note(),
            "Signed the authorization as 2 authorities. Moved the transaction to a fresh blockhash and signed it."
        );
        let tx = decode_v1_transaction(&done.wire).unwrap();
        assert!(signing_status(&tx, &[]).complete());
    }

    #[test]
    fn the_payer_does_not_move_the_blockhash_while_one_of_two_authorities_is_missing() {
        let hot = KeypairSigner::new_random();
        let (first, second) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        let wire = relayed_for(&hot, &[&first, &second]);
        let rpc = FakeRpc::default();

        // The first authority is here with the payer, the second is elsewhere.
        let partly = run(Some(&rpc), &wire, &[&hot, &first]);
        assert_eq!((partly.authorizations, partly.signatures), (1, 0));
        assert!(!partly.authorized && !partly.refreshed);
        assert_eq!(blockhash_of(&partly.wire), STALE);
        assert_eq!(rpc.asked(), 0);

        // The second arrives. Now the payer moves it and signs.
        let authorized = run(None, &partly.wire, &[&second]);
        let done = run(Some(&rpc), &authorized.wire, &[&hot]);
        assert_eq!((done.authorizations, done.signatures), (0, 1));
        assert!(done.refreshed && done.authorized);
        assert_eq!(blockhash_of(&done.wire), FRESH);
    }

    #[test]
    fn the_blockhash_stays_when_the_device_only_authorizes_or_must_wait() {
        let (hot, authority) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        let wire = relayed(&hot, &authority);
        let rpc = FakeRpc::default();

        // Only the authority's key is here: nothing signs the transaction.
        let done = run(Some(&rpc), &wire, &[&authority]);
        assert_eq!(blockhash_of(&done.wire), STALE);

        // Only the payer's key is here, and the authorization has not arrived.
        let tx = decode_v1_transaction(&wire).unwrap();
        assert!(!wants_fresh_blockhash(&tx, &[hot.pubkey()]));
        assert!(try_run(Some(&rpc), &wire, &[&hot]).is_err());
        assert_eq!(rpc.asked(), 0);
    }

    #[test]
    fn offline_or_with_a_node_that_does_not_answer_the_transaction_is_signed_as_it_is() {
        let (hot, authority) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        let wire = run(None, &relayed(&hot, &authority), &[&authority]).wire;

        let offline = run(None, &wire, &[&hot]);
        assert_eq!((offline.signatures, offline.refreshed), (1, false));
        assert_eq!(blockhash_of(&offline.wire), STALE);
        assert_eq!(offline.note(), "Signed the transaction.");

        let silent = FakeRpc {
            offline: true,
            ..FakeRpc::default()
        };
        let done = run(Some(&silent), &wire, &[&hot]);
        assert_eq!((done.signatures, done.refreshed), (1, false));
        assert_eq!(blockhash_of(&done.wire), STALE);
    }

    #[test]
    fn a_transaction_somebody_already_signed_keeps_its_blockhash() {
        let (hot, authority) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        let wire = run(None, &relayed(&hot, &authority), &[&authority]).wire;
        let signed = run(None, &wire, &[&hot]).wire;
        let tx = decode_v1_transaction(&signed).unwrap();
        assert!(!wants_fresh_blockhash(&tx, &[hot.pubkey()]));
    }

    #[test]
    fn a_device_with_no_key_for_the_transaction_is_told_so() {
        let payer = KeypairSigner::new_random();
        let stranger = KeypairSigner::new_random();
        let err = try_run(None, &plain(&payer), &[&stranger]).unwrap_err();
        assert!(matches!(err, SignError::NoLocalSigner));
    }
}
