//! Adding the wallets that are more than a key, and setting them up.

use crate::state::forms::{SendAsset, TxSource};
use crate::state::session::{Session, WalletSummary};
use solana_pubkey::Pubkey;
use wallet_core::squads::{Squad, SquadsProgram};
use wallet_core::{WalletClass, WalletId, WalletRecord};
use wallet_flows::squads::Lookup;

/// Wallets that can be a multisig member from this device.
pub fn member_candidates(wallets: &[WalletSummary]) -> Vec<WalletSummary> {
    wallets
        .iter()
        .filter(|wallet| {
            matches!(wallet.class, WalletClass::Hot | WalletClass::Offline)
                || wallet.contract.as_ref().is_some_and(|kind| {
                    kind.signer_authority().is_some() || kind.vector_settings().is_some()
                })
        })
        .cloned()
        .collect()
}

/// Wallets that can approve together with the acting member `acting` of a
/// multisig: what can be a member, without the acting one. When `members`
/// was read from the chain, only the wallets among them.
pub fn together_candidates(
    wallets: &[WalletSummary],
    acting: Option<WalletId>,
    members: &[Pubkey],
) -> Vec<WalletSummary> {
    member_candidates(wallets)
        .into_iter()
        .filter(|wallet| Some(wallet.id) != acting)
        .filter(|wallet| {
            members.is_empty()
                || wallet
                    .address
                    .is_some_and(|address| members.contains(&address))
        })
        .collect()
}

/// Something the row of a multisig has to say about the wallets chosen to
/// approve together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TogetherNote {
    /// Programmatic signers are chosen and none has an executor nonce account.
    NoExecutorNonce,
    /// An offline wallet is chosen and the row names no durable nonce.
    OfflineWithoutNonce,
    /// An offline wallet and a programmatic signer are both chosen.
    OfflineBesideProgrammatic,
}

/// What a choice of wallets that approve together comes to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TogetherChoice {
    /// The acting member and the wallets chosen with it. Zero when nobody
    /// was chosen with it.
    pub members: usize,
    /// An offline wallet is among them, so the row asks for a durable nonce.
    pub wants_nonce: bool,
    pub notes: Vec<TogetherNote>,
}

/// What the row says about `approves_with` voting together with `acting`.
pub fn together_choice(
    wallets: &[WalletSummary],
    acting: Option<WalletId>,
    approves_with: &[WalletId],
    approval_nonce: Option<Pubkey>,
) -> TogetherChoice {
    let find = |id: &WalletId| wallets.iter().find(|wallet| wallet.id == *id);
    let others: Vec<&WalletSummary> = approves_with.iter().filter_map(find).collect();
    let Some(acting) = acting
        .as_ref()
        .and_then(find)
        .filter(|_| !others.is_empty())
    else {
        return TogetherChoice::default();
    };
    let chosen: Vec<&WalletSummary> = std::iter::once(acting).chain(others).collect();
    let offline = chosen
        .iter()
        .any(|wallet| wallet.class == WalletClass::Offline);
    let signers: Vec<bool> = chosen
        .iter()
        .filter_map(|wallet| {
            let kind = wallet.contract.as_ref()?;
            kind.signer_authority()?;
            Some(kind.executor_nonce().is_some())
        })
        .collect();
    let mut notes = Vec::new();
    if !signers.is_empty() && !signers.contains(&true) {
        notes.push(TogetherNote::NoExecutorNonce);
    }
    if offline && approval_nonce.is_none() {
        notes.push(TogetherNote::OfflineWithoutNonce);
    }
    if offline && !signers.is_empty() {
        notes.push(TogetherNote::OfflineBesideProgrammatic);
    }
    TogetherChoice {
        members: chosen.len(),
        wants_nonce: offline,
        notes,
    }
}

/// The first of this device's wallets that is one of `members`.
fn local_member(wallets: &[WalletSummary], members: &[Pubkey]) -> Option<WalletId> {
    member_candidates(wallets)
        .into_iter()
        .find(|wallet| {
            wallet
                .address
                .is_some_and(|address| members.contains(&address))
        })
        .map(|wallet| wallet.id)
}

/// A Squads wallet record for `squad`, acted on by the first local member.
fn squads_record(
    session: Session,
    label: String,
    squad: Squad,
    members: Vec<Pubkey>,
    threshold: u16,
    fee_payer: Option<WalletId>,
) -> WalletRecord {
    let member = local_member(&session.wallet_summaries(), &members);
    let mut wallet = WalletRecord::squads(label, squad, members, threshold);
    wallet.kind.set_member_wallet(member);
    wallet.kind.set_fee_payer(fee_payer);
    wallet
}

/// Add a multisig that already exists. Members and threshold come from the
/// chain when it can be reached. The new wallet becomes the current one.
pub async fn add_existing_squad(
    session: Session,
    label: String,
    squad: Squad,
    fee_payer: Option<WalletId>,
) -> Result<(), String> {
    let rpc = session.rpc().ok();
    let found = wallet_flows::squads::look_up(rpc.as_deref(), &squad).await;
    let (members, threshold) = match found {
        Lookup::Found(state) => (
            state.members.iter().map(|member| member.key).collect(),
            state.threshold,
        ),
        Lookup::NotAMultisig => {
            return Err("That address is not a multisig of the chosen program on this cluster. If you meant the vault address, enter the multisig account instead.".into());
        }
        Lookup::Unreachable => (Vec::new(), 0),
    };
    let wallet = squads_record(session, label, squad, members, threshold, fee_payer);
    let id = wallet.id;
    session
        .add_contract_wallet(wallet)
        .map_err(|err| err.to_string())?;
    let _ = session.select_wallet(id);
    super::network::start_refresh(session);
    Ok(())
}

/// Create a multisig: store its wallet, and open the transaction that
/// creates it in Sign. `creator` is the hot wallet that pays.
pub async fn create_squad(
    session: Session,
    program: SquadsProgram,
    label: String,
    members: Vec<Pubkey>,
    threshold: u16,
    creator: (WalletId, Pubkey),
) -> Result<(), String> {
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    let made = wallet_flows::squads::create(rpc.as_ref(), program, &members, threshold, creator.1)
        .await
        .map_err(|err| err.to_string())?;
    let wallet = squads_record(
        session,
        label,
        made.squad,
        members,
        threshold,
        Some(creator.0),
    );
    session
        .add_contract_wallet(wallet)
        .map_err(|err| err.to_string())?;
    session.open_in_sign(
        made.wire,
        TxSource::Wallets,
        "This creates the multisig. Sign and send it, then the wallet is ready.",
    );
    Ok(())
}

/// Create the nonce account of the programmatic signer `id`: remember its
/// address on the wallet, and open the transaction that creates it in Sign.
pub async fn create_nonce(session: Session, id: WalletId) -> Result<(), String> {
    let route = session.route_of(id).map_err(|err| err.to_string())?;
    if route.fee_payer.is_none() {
        return Err("Pick a hot wallet to pay fees first.".into());
    }
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    let made = wallet_flows::programmatic::create_nonce(rpc.as_ref(), &route)
        .await
        .map_err(|err| err.to_string())?;
    session
        .update_wallet(id, |kind| kind.set_executor_nonce(Some(made.account)))
        .map_err(|err| err.to_string())?;
    session.open_in_sign(
        made.wire,
        TxSource::Wallets,
        "This creates the nonce account that orders the signer's actions.",
    );
    Ok(())
}

/// Create the durable nonce account of the offline wallet `id`, with its key
/// as the authority: remember its address on the wallet, make it the
/// default path when none is chosen yet, and open the transaction that
/// creates it in Sign. Deprecated once the programmatic signer or Vector is
/// on mainnet.
pub async fn create_durable_nonce(session: Session, id: WalletId) -> Result<(), String> {
    let route = session.route_of(id).map_err(|err| err.to_string())?;
    if route.fee_payer.is_none() {
        return Err("Pick a hot wallet to pay fees first.".into());
    }
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    let made = wallet_flows::offline_nonce::create(rpc.as_ref(), &route)
        .await
        .map_err(|err| err.to_string())?;
    session
        .update_wallet(id, |kind| {
            kind.set_durable_nonce(Some(made.account));
            kind.default_path_unless_chosen(wallet_core::OfflinePath::DurableNonce);
        })
        .map_err(|err| err.to_string())?;
    session.open_in_sign(
        made.wire,
        TxSource::Wallets,
        "This creates the durable nonce account of the offline key. Once it lands, what this key signs for itself stays valid until it is signed.",
    );
    Ok(())
}

/// Open the transaction that initializes the Vector wallet `id`, paid by its
/// fee payer. The account can already hold lamports; initializing keeps them.
pub async fn initialize_vector(session: Session, id: WalletId) -> Result<(), String> {
    use wallet_core::WalletKind;

    let route = session.route_of(id).map_err(|err| err.to_string())?;
    let Some((_, payer)) = route.fee_payer else {
        return Err("Pick a hot wallet to pay fees first.".into());
    };
    let wallet = session
        .wallet_summaries()
        .into_iter()
        .find(|wallet| wallet.id == id)
        .ok_or_else(|| "That wallet is not in the vault.".to_string())?;
    let settings = wallet
        .contract
        .as_ref()
        .and_then(WalletKind::vector_settings)
        .ok_or_else(|| "That wallet is not a Vector wallet.".to_string())?;
    let key = settings.key.to_vec();
    let scheme = settings.scheme;
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    if wallet_flows::vector::read_account(rpc.as_ref(), &route.address)
        .await
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Err("This Vector wallet is already initialized on this cluster.".into());
    }
    let made = wallet_flows::vector::initialize(rpc.as_ref(), payer, scheme, &key)
        .await
        .map_err(|err| err.to_string())?;
    session.open_in_sign(made.wire, TxSource::Wallets, made.note);
    Ok(())
}

/// Open the transaction that moves a Vector wallet's account to the key
/// `rotation` names, as the device that holds the key signed it, paid by
/// the wallet's fee payer.
pub async fn send_rotation(
    session: Session,
    rotation: wallet_core::protocol::KeyRotation,
) -> Result<(), String> {
    use wallet_core::WalletKind;

    let account = Pubkey::new_from_array(rotation.account);
    let wallet = session
        .wallet_summaries()
        .into_iter()
        .find(|wallet| wallet.address == Some(account))
        .ok_or_else(|| {
            format!("This rotation is for {account}, which is not a wallet on this device.")
        })?;
    let settings = wallet
        .contract
        .as_ref()
        .and_then(WalletKind::vector_settings)
        .ok_or_else(|| format!("{} is not a Vector wallet.", wallet.label))?;
    let (scheme, key) = (settings.scheme, settings.key.to_vec());
    let route = session.route_of(wallet.id).map_err(|err| err.to_string())?;
    let Some((_, payer)) = route.fee_payer else {
        return Err(format!(
            "Pick a hot wallet to pay fees for {} first.",
            wallet.label
        ));
    };
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    let made = wallet_flows::vector::rotate(rpc.as_ref(), payer, scheme, &key, &rotation)
        .await
        .map_err(|err| format!("This rotation cannot be sent: {err}"))?;
    session.open_in_sign(made.wire, TxSource::Wallets, made.note);
    Ok(())
}

/// Open a transfer of `amount` of `asset`, in its smallest units, from
/// wallet `id` to `to` in Sign, on a fresh blockhash. A token recipient
/// without an account for it gets one created first.
pub async fn send(
    session: Session,
    id: WalletId,
    to: Pubkey,
    asset: SendAsset,
    amount: u64,
) -> Result<(), String> {
    let route = session.route_of(id).map_err(|err| err.to_string())?;
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    let (prepared, height) = match asset {
        SendAsset::Sol => wallet_flows::transfer::prepare(rpc.as_ref(), &route, to, amount).await,
        SendAsset::Token(mint) => {
            wallet_flows::token_transfer::prepare(rpc.as_ref(), &route, mint, to, amount).await
        }
    }
    .map_err(|err| err.to_string())?;
    session.open_in_sign_with_lifetime(prepared.wire, TxSource::Wallets, prepared.note, height);
    Ok(())
}

/// Where `mint` sent to `to` would land, read from the chain.
pub async fn send_destination(
    session: Session,
    mint: Pubkey,
    to: Pubkey,
) -> Result<wallet_flows::token_transfer::Resolved, String> {
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    wallet_flows::token_transfer::resolve(rpc.as_ref(), mint, to)
        .await
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_core::WalletKind;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn wallet(n: u8, class: WalletClass, contract: Option<WalletKind>) -> WalletSummary {
        WalletSummary {
            id: WalletId::new(),
            label: format!("Wallet {n}"),
            tags: Vec::new(),
            class,
            summary: String::new(),
            address: Some(pk(n)),
            contract,
            fee_payer: None,
            derivation_path: None,
            durable_nonce: None,
            default_path: None,
        }
    }

    fn signer(n: u8, nonce: Option<Pubkey>) -> WalletSummary {
        let mut record = WalletRecord::programmatic("Signer", pk(n), None);
        record.kind.set_executor_nonce(nonce);
        let mut summary = wallet(n, WalletClass::SmartContract, Some(record.kind));
        summary.address = Some(pk(n));
        summary
    }

    fn ids(wallets: &[WalletSummary]) -> Vec<WalletId> {
        wallets.iter().map(|wallet| wallet.id).collect()
    }

    #[test]
    fn the_wallets_that_can_approve_together_are_members_other_than_the_acting_one() {
        let acting = wallet(1, WalletClass::Hot, None);
        let hot = wallet(2, WalletClass::Hot, None);
        let cold = wallet(3, WalletClass::Offline, None);
        let programmatic = signer(4, None);
        let ledger = wallet(5, WalletClass::Hardware, None);
        let squad = Squad {
            program: SquadsProgram::V4,
            address: pk(40),
        };
        let multisig = wallet(
            6,
            WalletClass::SmartContract,
            Some(WalletRecord::squads("Team", squad, Vec::new(), 2).kind),
        );
        let outsider = wallet(7, WalletClass::Hot, None);
        let all = vec![
            acting.clone(),
            hot.clone(),
            cold.clone(),
            programmatic.clone(),
            ledger,
            multisig,
            outsider.clone(),
        ];

        // Members were not read from the chain: every kind that can be one.
        let unread = together_candidates(&all, Some(acting.id), &[]);
        assert_eq!(
            ids(&unread),
            vec![hot.id, cold.id, programmatic.id, outsider.id]
        );
        // Once they were, only wallets among them.
        let members = [pk(1), pk(2), pk(3), pk(4)];
        let read = together_candidates(&all, Some(acting.id), &members);
        assert_eq!(ids(&read), vec![hot.id, cold.id, programmatic.id]);
        // Whoever acts is not offered to approve with itself.
        let other = together_candidates(&all, Some(hot.id), &members);
        assert_eq!(ids(&other), vec![acting.id, cold.id, programmatic.id]);
    }

    #[test]
    fn each_note_about_approving_together_appears_under_its_own_condition() {
        let acting = wallet(1, WalletClass::Hot, None);
        let hot = wallet(2, WalletClass::Hot, None);
        let cold = wallet(3, WalletClass::Offline, None);
        let bare = signer(4, None);
        let ordered = signer(5, Some(pk(50)));
        let all = vec![
            acting.clone(),
            hot.clone(),
            cold.clone(),
            bare.clone(),
            ordered.clone(),
        ];
        let choice = |acting: &WalletSummary, with: &[&WalletSummary], nonce| {
            let with: Vec<WalletId> = with.iter().map(|wallet| wallet.id).collect();
            together_choice(&all, Some(acting.id), &with, nonce)
        };

        // Nobody ticked: nothing to say, whatever the acting member is.
        assert_eq!(choice(&acting, &[], None), TogetherChoice::default());
        assert_eq!(choice(&cold, &[], None), TogetherChoice::default());
        assert_eq!(
            together_choice(&all, None, &[hot.id], None),
            TogetherChoice::default()
        );

        // Hot keys need nothing.
        assert_eq!(
            choice(&acting, &[&hot], None),
            TogetherChoice {
                members: 2,
                wants_nonce: false,
                notes: Vec::new(),
            }
        );

        // No programmatic signer among them has a nonce account.
        assert_eq!(
            choice(&acting, &[&bare], None).notes,
            vec![TogetherNote::NoExecutorNonce]
        );
        assert_eq!(choice(&bare, &[&ordered], None).notes, Vec::new());
        assert_eq!(choice(&acting, &[&ordered], None).notes, Vec::new());

        // An offline wallet, ticked or acting, asks for a durable nonce.
        for with_cold in [
            choice(&acting, &[&cold], None),
            choice(&cold, &[&hot], None),
        ] {
            assert!(with_cold.wants_nonce);
            assert_eq!(with_cold.notes, vec![TogetherNote::OfflineWithoutNonce]);
        }
        let named = choice(&acting, &[&cold], Some(pk(60)));
        assert!(named.wants_nonce);
        assert_eq!(named.notes, Vec::new());

        // An offline wallet beside a programmatic signer.
        let mixed = choice(&acting, &[&cold, &ordered], Some(pk(60)));
        assert_eq!(mixed.members, 3);
        assert_eq!(mixed.notes, vec![TogetherNote::OfflineBesideProgrammatic]);
        assert_eq!(
            choice(&acting, &[&cold, &bare], None).notes,
            vec![
                TogetherNote::NoExecutorNonce,
                TogetherNote::OfflineWithoutNonce,
                TogetherNote::OfflineBesideProgrammatic,
            ]
        );
    }
}
