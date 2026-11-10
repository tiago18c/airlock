//! What a site gets, decided from what the app holds about it and the
//! accounts it could be shown. The core reads these under its locks and acts
//! on the answer; nothing here locks, saves or publishes.

use bridge_protocol::reply::SiteAccount;
use solana_pubkey::Pubkey;
use wallet_core::{connect_choice, reconnect_accounts, ShareableAccount, SiteGrant};

use crate::bridge::{SiteChoice, SiteSign};

/// What the app holds about one site.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Site {
    /// The user blocked it.
    pub blocked: bool,
    /// What it was given, remembered or for this run of the app.
    pub grant: Option<SiteGrant>,
    /// What it was given is saved.
    pub remembered: bool,
}

/// What a `connect` gets before anyone is asked.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Connect {
    Blocked,
    /// Nothing the site was given covers this: only the user can decide.
    Undecided,
    /// The site reconnects with `accounts`, the current wallet first.
    /// `ended`: it had disconnected, and is back.
    Reconnect {
        accounts: Vec<Pubkey>,
        ended: bool,
    },
}

/// What a `connect` from `site` gets. Only accounts the user already showed
/// the site can come back, and only with the current wallet among them.
pub(crate) fn connect(
    site: &Site,
    silent: bool,
    shareable: &[ShareableAccount],
    current: Option<Pubkey>,
) -> Connect {
    if site.blocked {
        return Connect::Blocked;
    }
    let Some(grant) = &site.grant else {
        return Connect::Undecided;
    };
    // A site that disconnected stays out until it asks again.
    if grant.ended && silent {
        return Connect::Undecided;
    }
    match reconnect_accounts(&grant.accounts, current, &keys(shareable)) {
        Some(accounts) => Connect::Reconnect {
            accounts,
            ended: grant.ended,
        },
        None => Connect::Undecided,
    }
}

/// Whether `site` may ask `accounts` for a signature. The user still
/// approves the signature itself.
pub(crate) fn sign(site: &Site, accounts: &[Pubkey]) -> SiteSign {
    if site.blocked {
        return SiteSign::Blocked;
    }
    match &site.grant {
        Some(grant) if !grant.ended && accounts.iter().all(|key| grant.accounts.contains(key)) => {
            SiteSign::Allowed
        }
        _ => SiteSign::NotConnected,
    }
}

/// What the connect modal opens with for `site`.
pub(crate) fn choice(
    site: &Site,
    shareable: &[ShareableAccount],
    current: Option<Pubkey>,
) -> SiteChoice {
    SiteChoice {
        choice: connect_choice(
            &keys(shareable),
            current,
            site.grant.as_ref().map(|grant| grant.accounts.as_slice()),
        ),
        remembered: site.remembered,
    }
}

/// What a site that holds `held` and was given `granted` holds once the
/// wallets changed.
pub(crate) fn synced(
    granted: &[Pubkey],
    held: &[Pubkey],
    shareable: &[ShareableAccount],
    current: Option<Pubkey>,
) -> Vec<Pubkey> {
    let keys = keys(shareable);
    // A wallet the site was not given changes nothing, but one no
    // longer on this device cannot stay with it.
    reconnect_accounts(granted, current, &keys).unwrap_or_else(|| {
        held.iter()
            .filter(|key| keys.contains(key))
            .copied()
            .collect()
    })
}

/// What a site sees once `account` signed in to it: that account first, then
/// what it was given before.
pub(crate) fn signed_in(account: Pubkey, grant: Option<&SiteGrant>) -> Vec<Pubkey> {
    // The account that signed in comes first: it is who the site now sees.
    let mut keys = vec![account];
    if let Some(grant) = grant {
        keys.extend(grant.accounts.iter().filter(|key| **key != account));
    }
    keys
}

/// `keys` as a site is sent them, each with the label it has here.
pub(crate) fn labelled(keys: Vec<Pubkey>, accounts: &[ShareableAccount]) -> Vec<SiteAccount> {
    keys.into_iter()
        .map(|key| {
            let known = accounts.iter().find(|account| account.pubkey == key);
            SiteAccount {
                pubkey: key.to_bytes(),
                label: known
                    .map(|account| account.label.clone())
                    .unwrap_or_default(),
                signs_messages: known.is_some_and(|account| !account.contract),
            }
        })
        .collect()
}

fn keys(shareable: &[ShareableAccount]) -> Vec<Pubkey> {
    shareable.iter().map(|account| account.pubkey).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://example.org";

    fn key(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn keys_of(bytes: &[u8]) -> Vec<Pubkey> {
        bytes.iter().copied().map(key).collect()
    }

    fn account(byte: u8) -> ShareableAccount {
        ShareableAccount {
            pubkey: key(byte),
            label: format!("Wallet {byte}"),
            contract: false,
            summary: String::new(),
            on_device: None,
        }
    }

    /// Wallets 1, 2 and 3 on this device, 1 the current one.
    fn wallets() -> (Vec<ShareableAccount>, Option<Pubkey>) {
        (vec![account(1), account(2), account(3)], Some(key(1)))
    }

    fn granted(accounts: &[u8]) -> Site {
        Site {
            grant: Some(SiteGrant::new(ORIGIN, keys_of(accounts))),
            ..Site::default()
        }
    }

    fn disconnected(accounts: &[u8]) -> Site {
        let mut site = granted(accounts);
        site.grant.as_mut().unwrap().ended = true;
        site
    }

    fn blocked(mut site: Site) -> Site {
        site.blocked = true;
        site
    }

    #[test]
    fn a_blocked_site_is_turned_away_whatever_it_was_given() {
        let (shareable, current) = wallets();
        for site in [blocked(Site::default()), blocked(granted(&[1]))] {
            for silent in [true, false] {
                assert_eq!(
                    connect(&site, silent, &shareable, current),
                    Connect::Blocked
                );
            }
            assert_eq!(sign(&site, &[key(1)]), SiteSign::Blocked);
            assert_eq!(sign(&site, &[]), SiteSign::Blocked);
        }
    }

    #[test]
    fn a_site_never_given_anything_is_for_the_user_to_decide() {
        let (shareable, current) = wallets();
        let site = Site::default();
        for silent in [true, false] {
            assert_eq!(
                connect(&site, silent, &shareable, current),
                Connect::Undecided
            );
        }
        assert_eq!(sign(&site, &[key(1)]), SiteSign::NotConnected);
    }

    #[test]
    fn a_silent_connect_with_a_grant_holding_the_current_wallet_reconnects_it_first() {
        let (shareable, current) = wallets();
        assert_eq!(
            connect(&granted(&[2, 1]), true, &shareable, current),
            Connect::Reconnect {
                accounts: vec![key(1), key(2)],
                ended: false,
            }
        );
    }

    #[test]
    fn a_connect_with_a_grant_not_holding_the_current_wallet_asks_the_user() {
        let (shareable, current) = wallets();
        for silent in [true, false] {
            assert_eq!(
                connect(&granted(&[2, 3]), silent, &shareable, current),
                Connect::Undecided
            );
        }
        // Nor with no current wallet at all.
        assert_eq!(
            connect(&granted(&[1]), true, &shareable, None),
            Connect::Undecided
        );
    }

    #[test]
    fn a_grant_reconnects_only_accounts_still_on_this_device() {
        let (_, current) = wallets();
        let shareable = vec![account(1)];
        assert_eq!(
            connect(&granted(&[1, 9]), true, &shareable, current),
            Connect::Reconnect {
                accounts: vec![key(1)],
                ended: false,
            }
        );
    }

    #[test]
    fn a_site_that_disconnected_is_back_only_when_it_asks() {
        let (shareable, current) = wallets();
        let site = disconnected(&[1]);
        assert_eq!(
            connect(&site, true, &shareable, current),
            Connect::Undecided
        );
        assert_eq!(
            connect(&site, false, &shareable, current),
            Connect::Reconnect {
                accounts: vec![key(1)],
                ended: true,
            }
        );
        assert_eq!(sign(&site, &[key(1)]), SiteSign::NotConnected);
    }

    #[test]
    fn a_site_may_ask_only_accounts_it_was_given_for_a_signature() {
        let site = granted(&[1, 2]);
        assert_eq!(sign(&site, &[key(1)]), SiteSign::Allowed);
        assert_eq!(sign(&site, &[key(2), key(1)]), SiteSign::Allowed);
        assert_eq!(sign(&site, &[]), SiteSign::Allowed);
        assert_eq!(sign(&site, &[key(3)]), SiteSign::NotConnected);
        assert_eq!(sign(&site, &[key(1), key(3)]), SiteSign::NotConnected);
    }

    #[test]
    fn the_connect_modal_ticks_what_the_site_was_given_and_says_if_remembered() {
        let (shareable, current) = wallets();
        let first = choice(&Site::default(), &shareable, current);
        assert_eq!(first.choice.ticked, vec![key(1)]);
        assert!(!first.remembered);

        let mut site = granted(&[2]);
        site.remembered = true;
        let again = choice(&site, &shareable, current);
        assert_eq!(again.choice.ticked, vec![key(2)]);
        assert!(again.choice.current_is_new);
        assert!(again.remembered);
    }

    #[test]
    fn after_the_wallets_change_a_site_follows_the_current_wallet_it_was_given() {
        let (shareable, _) = wallets();
        // The current wallet is now 2, and the site was given it.
        assert_eq!(
            synced(
                &keys_of(&[1, 2]),
                &keys_of(&[1, 2]),
                &shareable,
                Some(key(2))
            ),
            vec![key(2), key(1)]
        );
        // The current wallet is 3, which the site was not given: it keeps
        // what it holds, less what is no longer on this device.
        assert_eq!(
            synced(
                &keys_of(&[1, 9]),
                &keys_of(&[1, 9]),
                &shareable,
                Some(key(3))
            ),
            vec![key(1)]
        );
    }

    #[test]
    fn signing_in_puts_the_account_first_ahead_of_what_was_given() {
        let grant = SiteGrant::new(ORIGIN, vec![key(1), key(2)]);
        assert_eq!(signed_in(key(2), Some(&grant)), vec![key(2), key(1)]);
        assert_eq!(signed_in(key(3), None), vec![key(3)]);
    }

    #[test]
    fn a_site_is_sent_each_account_with_its_label_here() {
        let contract = ShareableAccount {
            contract: true,
            ..account(2)
        };
        let sent = labelled(vec![key(2), key(1), key(9)], &[account(1), contract]);
        let labels: Vec<&str> = sent.iter().map(|one| one.label.as_str()).collect();
        assert_eq!(labels, ["Wallet 2", "Wallet 1", ""]);
        let signs: Vec<bool> = sent.iter().map(|one| one.signs_messages).collect();
        assert_eq!(signs, [false, true, false]);
    }
}
