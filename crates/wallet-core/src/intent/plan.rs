//! What the wallet does with a request's notes, before anything is shown
//! ("What the wallet does with them" in `docs/intents.md`).

use super::error::{Requirement, Unsupported};
use super::notes::{AccountNote, Delivery, Notes, Order, SignsBy};
use super::profile::{NewAccountKey, Payer, SigningProfile};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;

/// What [`plan_resolution`] decided.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// `new` accounts that get a key made on this device, by the address the
    /// app gave them, in the order of the notes.
    pub new_keys: Vec<Pubkey>,
    /// `new` accounts the carrier program signs for, at an address it
    /// derives: a Squads multisig's ephemeral signers. By the address the
    /// app gave them, in the order of the notes.
    pub carried: Vec<Pubkey>,
    /// The `feePayer` replaced, and the route's payer that takes its place.
    pub fee_payer: Option<(Pubkey, Pubkey)>,
    /// Addresses whose signatures the app owes: `new` accounts kept, a fee
    /// payer of the app's kept, and `signer` notes. Empty unless the
    /// delivery is `cosign` or `return`.
    pub app_signs: Vec<Pubkey>,
}

impl Plan {
    /// The addresses replaced, old to new: each of [`Self::new_keys`] with the
    /// key made for it, `keys` in the same order, each of [`Self::carried`]
    /// with the address its carrier signs for, `carried` in the same order,
    /// and the fee payer. Pass the result to
    /// [`replacements`](super::replacements), which adds the derived
    /// accounts.
    ///
    /// # Panics
    ///
    /// When `keys` is not as long as [`Self::new_keys`], or `carried` as
    /// [`Self::carried`]: a mistake of the caller.
    pub fn replaced(&self, keys: &[Pubkey], carried: &[Pubkey]) -> BTreeMap<Pubkey, Pubkey> {
        assert_eq!(
            keys.len(),
            self.new_keys.len(),
            "one key for each new account"
        );
        assert_eq!(
            carried.len(),
            self.carried.len(),
            "one address for each carried account"
        );
        self.new_keys
            .iter()
            .copied()
            .zip(keys.iter().copied())
            .chain(self.carried.iter().copied().zip(carried.iter().copied()))
            .chain(self.fee_payer)
            .collect()
    }

    /// The app signs after the wallet.
    pub fn needs_app(&self) -> bool {
        !self.app_signs.is_empty()
    }
}

/// Decide what the wallet does with `notes`, already checked, for an
/// account of `profile`. `transactions` is how many the request carries.
///
/// The account acted as carries no note (check 6). When it is also a
/// transaction's fee payer nothing here replaces it: the app's usual
/// preparation has another key pay for a wallet without one, and leaves
/// every instruction as written.
/// `route_payer` is the payer of the account's signing route, `None` when
/// nothing on this device can pay.
///
/// 1. A `new` account that is `replaceable` gets a key made here when the
///    profile lists `wallet-key`, or an address its carrier program signs
///    for when it lists `carrier`. One that is not needs the app's
///    signature.
/// 2. A `feePayer` that is `replaceable` is replaced by `route_payer` when
///    that is another address, unless it is the app's, the delivery is
///    `cosign` or `return` and the profile lists `app` as a payer: then it
///    stays and the app signs for it. One that is not `replaceable` stays,
///    and needs the app's signature when the app holds its key.
/// 3. A `signer` note needs the app's signature.
///
/// A request that needs the app's signature takes `cosign` or `return` and
/// a profile that allows it. Otherwise it is [`Unsupported`], naming what
/// asked for the signature.
pub fn plan_resolution(
    notes: &Notes,
    profile: &SigningProfile,
    delivery: Delivery,
    order: Order,
    transactions: usize,
    route_payer: Option<Pubkey>,
) -> Result<Plan, Unsupported> {
    if !profile.delivery.contains(&delivery) {
        return Err(Unsupported::new(
            Requirement::Delivery,
            format!(
                "{} cannot take delivery \"{}\".",
                profile.account,
                delivery_name(delivery)
            ),
        ));
    }
    if transactions > usize::from(profile.max_transactions) {
        return Err(Unsupported::new(
            Requirement::Order,
            format!(
                "{} takes at most {} transaction(s) in one request: each authorization it signs is ordered by a nonce that the one before it moves. Send them one request at a time.",
                profile.account, profile.max_transactions
            ),
        ));
    }
    if order == Order::Atomic && transactions > 1 {
        return Err(Unsupported::new(
            Requirement::Order,
            "Running several transactions as one is not supported. Send one transaction, or another order.",
        ));
    }
    let app_may_sign = |requirement: Requirement, what: String| {
        if delivery.app_signs() {
            Ok(())
        } else {
            Err(Unsupported::new(
                requirement,
                format!("{what}, so the app signs after the wallet. That takes delivery \"cosign\" or \"return\"."),
            ))
        }
    };

    let mut plan = Plan::default();
    for (address, note) in notes.accounts.iter() {
        match note {
            AccountNote::New { replaceable } => {
                let lists = |key: NewAccountKey| profile.new_accounts.contains(&key);
                if *replaceable && lists(NewAccountKey::WalletKey) {
                    plan.new_keys.push(*address);
                    continue;
                }
                if *replaceable && lists(NewAccountKey::Carrier) {
                    plan.carried.push(*address);
                    continue;
                }
                if *replaceable && !lists(NewAccountKey::AppKey) {
                    return Err(Unsupported::new(
                        Requirement::NewAccounts,
                        format!("{} cannot sign for the new account {address}. Send it from an account that can.", profile.account),
                    ));
                }
                if !profile.new_accounts.contains(&NewAccountKey::AppKey) {
                    return Err(Unsupported::new(
                        Requirement::NewAccounts,
                        format!("{} cannot keep the app's key for the new account {address}. Mark it replaceable.", profile.account),
                    ));
                }
                app_may_sign(
                    Requirement::NewAccounts,
                    format!("The new account {address} keeps the app's key"),
                )?;
                plan.app_signs.push(*address);
            }
            AccountNote::FeePayer { replaceable, signs } => {
                let apps = *signs == Some(SignsBy::App);
                let app_pays =
                    apps && delivery.app_signs() && profile.fee_payer.contains(&Payer::App);
                if *replaceable && !app_pays {
                    match route_payer {
                        Some(payer) if payer == *address => {}
                        Some(payer) => plan.fee_payer = Some((*address, payer)),
                        None => {
                            return Err(Unsupported::new(
                                Requirement::FeePayer,
                                format!(
                                    "Nothing on this device can pay fees for {}.",
                                    profile.account
                                ),
                            ))
                        }
                    }
                    continue;
                }
                if apps {
                    if !profile.fee_payer.contains(&Payer::App) {
                        return Err(Unsupported::new(
                            Requirement::FeePayer,
                            format!("{} cannot have its fees paid by the app's {address}. Mark the fee payer replaceable.", profile.account),
                        ));
                    }
                    app_may_sign(
                        Requirement::FeePayer,
                        format!("The app's {address} pays the fees"),
                    )?;
                    plan.app_signs.push(*address);
                }
            }
            AccountNote::Signer { .. } => {
                app_may_sign(
                    Requirement::Signer,
                    format!("{address} is the app's signer"),
                )?;
                plan.app_signs.push(*address);
            }
            AccountNote::Derived(_) => {}
        }
    }
    Ok(plan)
}

pub(crate) fn delivery_name(delivery: Delivery) -> &'static str {
    match delivery {
        Delivery::Send => "send",
        Delivery::Cosign => "cosign",
        Delivery::Return => "return",
    }
}

/// For each of `carried`, the one transaction of `wires` it signs, in the
/// same order. A carrier signs for a new account at an address derived from
/// the one proposal that stores it (a Squads multisig's ephemeral signer),
/// so an account that signs several transactions, or none, cannot be
/// carried: [`Unsupported`] (`newAccounts`). A transaction that cannot be
/// read is taken to sign for nothing.
pub fn carried_transactions(
    wires: &[Vec<u8>],
    carried: &[Pubkey],
) -> Result<Vec<usize>, Unsupported> {
    let signers: Vec<Vec<Pubkey>> = wires
        .iter()
        .map(|wire| {
            crate::tx::decode::parse_wire(wire)
                .map(|view| {
                    let count = view.num_required_signatures.min(view.static_keys.len());
                    view.static_keys[..count].to_vec()
                })
                .unwrap_or_default()
        })
        .collect();
    carried
        .iter()
        .map(|address| {
            let mut signs = signers
                .iter()
                .enumerate()
                .filter(|(_, keys)| keys.contains(address))
                .map(|(index, _)| index);
            match (signs.next(), signs.next()) {
                (Some(index), None) => Ok(index),
                (None, _) => Err(Unsupported::new(
                    Requirement::NewAccounts,
                    format!("The new account {address} signs no transaction of the request, so the multisig cannot sign for it."),
                )),
                (Some(_), Some(_)) => Err(Unsupported::new(
                    Requirement::NewAccounts,
                    format!("The new account {address} signs more than one transaction of the request. The multisig signs for a new account in one proposal only, so it has to sign one transaction."),
                )),
            }
        })
        .collect()
}
