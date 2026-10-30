//! Which sites may see which accounts, and which sites are turned away. Kept
//! in the workspace in plain text, like the address book: it names no secret,
//! and a site must reconnect while the vault is locked.
//!
//! A grant shows a site public keys and nothing else. Every signature is still
//! approved on its own.

use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::ids::pubkey_string;

/// Sites the user chose to remember, and sites they blocked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connections {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<SiteGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<String>,
}

/// The accounts one origin may see. An origin is compared whole, so another
/// subdomain, port or scheme is another site.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteGrant {
    pub origin: String,
    #[serde(with = "pubkeys")]
    pub accounts: Vec<Pubkey>,
    /// The site disconnected. The grant stays, but a silent connect gets
    /// nothing until the site asks to connect again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ended: bool,
}

impl SiteGrant {
    pub fn new(origin: impl Into<String>, accounts: Vec<Pubkey>) -> Self {
        Self {
            origin: origin.into(),
            accounts,
            ended: false,
        }
    }
}

impl Connections {
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty() && self.blocked.is_empty()
    }

    pub fn grant(&self, origin: &str) -> Option<&SiteGrant> {
        self.sites.iter().find(|site| site.origin == origin)
    }

    pub fn is_blocked(&self, origin: &str) -> bool {
        self.blocked.iter().any(|known| known == origin)
    }

    /// Remember that `origin` may see `accounts`, in place of what it could.
    pub fn remember(&mut self, origin: &str, accounts: Vec<Pubkey>) {
        self.forget(origin);
        self.sites.push(SiteGrant::new(origin, accounts));
    }

    pub fn forget(&mut self, origin: &str) {
        self.sites.retain(|site| site.origin != origin);
    }

    /// Turn `origin` away from now on. Its grant goes with it.
    pub fn block(&mut self, origin: &str) {
        self.forget(origin);
        if !self.is_blocked(origin) {
            self.blocked.push(origin.to_owned());
        }
    }

    /// Mark whether `origin`'s connection has ended. Answers whether that
    /// changed anything.
    pub fn set_ended(&mut self, origin: &str, ended: bool) -> bool {
        match self.sites.iter_mut().find(|site| site.origin == origin) {
            Some(site) if site.ended != ended => {
                site.ended = ended;
                true
            }
            _ => false,
        }
    }
}

/// What a site that was granted `granted` gets without asking the user: the
/// accounts still on this device with `current` first, when `current` is one of
/// them. `None` when the user has to decide.
pub fn reconnect_accounts(
    granted: &[Pubkey],
    current: Option<Pubkey>,
    shareable: &[Pubkey],
) -> Option<Vec<Pubkey>> {
    let current = current.filter(|key| granted.contains(key) && shareable.contains(key))?;
    let held = granted.iter().filter(|key| shareable.contains(key));
    Some(current_first(held.copied().collect(), Some(current)))
}

/// `accounts` as a site gets them: `current` first when it is among them, the
/// rest in the order given. Sites act as the first account they are given.
pub fn current_first(mut accounts: Vec<Pubkey>, current: Option<Pubkey>) -> Vec<Pubkey> {
    if let Some(at) = current.and_then(|key| accounts.iter().position(|known| *known == key)) {
        accounts[..=at].rotate_right(1);
    }
    accounts
}

/// What the connect modal opens with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectChoice {
    /// Every account that can be shared, the current wallet first.
    pub order: Vec<Pubkey>,
    /// The accounts ticked when it opens.
    pub ticked: Vec<Pubkey>,
    /// The current wallet, when it can be shared.
    pub current: Option<Pubkey>,
    /// The site has a grant, and the current wallet is not in it: revealing it
    /// cannot be undone, so it takes a tick of the user's own.
    pub current_is_new: bool,
}

/// The modal's starting point. On a first request the current wallet is ticked.
/// After that the accounts already granted are, and nothing new.
pub fn connect_choice(
    shareable: &[Pubkey],
    current: Option<Pubkey>,
    granted: Option<&[Pubkey]>,
) -> ConnectChoice {
    let current = current.filter(|key| shareable.contains(key));
    let order = current_first(shareable.to_vec(), current);
    let ticked = match granted {
        Some(granted) => granted
            .iter()
            .filter(|key| shareable.contains(key))
            .copied()
            .collect(),
        None => match (current, shareable) {
            (Some(key), _) => vec![key],
            (None, [only]) => vec![*only],
            (None, _) => Vec::new(),
        },
    };
    let current_is_new = granted.is_some() && current.is_some_and(|key| !ticked.contains(&key));
    ConnectChoice {
        order,
        ticked,
        current,
        current_is_new,
    }
}

mod pubkeys {
    use super::*;
    use serde::{Deserializer, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(transparent)]
    struct Key(#[serde(with = "pubkey_string")] Pubkey);

    pub fn serialize<S: Serializer>(keys: &[Pubkey], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(keys.iter().map(|key| Key(*key)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Pubkey>, D::Error> {
        let keys = Vec::<Key>::deserialize(deserializer)?;
        Ok(keys.into_iter().map(|key| key.0).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn reconnect_puts_the_current_wallet_first() {
        let granted = [key(1), key(2), key(3)];
        let held = [key(1), key(2), key(3), key(4)];
        assert_eq!(
            reconnect_accounts(&granted, Some(key(3)), &held),
            Some(vec![key(3), key(1), key(2)])
        );
    }

    #[test]
    fn reconnect_needs_the_current_wallet_in_the_grant() {
        let held = [key(1), key(2)];
        assert_eq!(reconnect_accounts(&[key(1)], Some(key(2)), &held), None);
        assert_eq!(reconnect_accounts(&[key(1)], None, &held), None);
    }

    #[test]
    fn reconnect_leaves_out_a_wallet_no_longer_here() {
        assert_eq!(
            reconnect_accounts(&[key(1), key(2)], Some(key(1)), &[key(1)]),
            Some(vec![key(1)])
        );
        // The current wallet was granted, but is no longer on this device.
        assert_eq!(reconnect_accounts(&[key(2)], Some(key(2)), &[key(1)]), None);
    }

    #[test]
    fn the_first_selected_leads_when_the_current_wallet_is_not_selected() {
        assert_eq!(
            current_first(vec![key(2), key(1)], Some(key(9))),
            vec![key(2), key(1)]
        );
        assert_eq!(
            current_first(vec![key(2), key(1), key(3)], Some(key(3))),
            vec![key(3), key(2), key(1)]
        );
    }

    #[test]
    fn a_first_request_ticks_the_current_wallet() {
        let choice = connect_choice(&[key(1), key(2)], Some(key(2)), None);
        assert_eq!(choice.order, vec![key(2), key(1)]);
        assert_eq!(choice.ticked, vec![key(2)]);
        assert!(!choice.current_is_new);
    }

    #[test]
    fn a_first_request_ticks_the_only_account_when_the_current_cannot_be_shared() {
        assert_eq!(
            connect_choice(&[key(1)], Some(key(9)), None).ticked,
            vec![key(1)]
        );
        assert!(connect_choice(&[key(1), key(2)], Some(key(9)), None)
            .ticked
            .is_empty());
    }

    #[test]
    fn a_later_request_ticks_only_what_was_granted() {
        let granted = [key(1), key(3)];
        let choice = connect_choice(&[key(1), key(2), key(3)], Some(key(2)), Some(&granted));
        assert_eq!(choice.order, vec![key(2), key(1), key(3)]);
        assert_eq!(choice.ticked, vec![key(1), key(3)]);
        assert!(choice.current_is_new);
    }

    #[test]
    fn blocking_drops_the_grant_and_is_not_repeated() {
        let mut connections = Connections::default();
        connections.remember("https://a.test", vec![key(1)]);
        connections.block("https://a.test");
        connections.block("https://a.test");
        assert!(connections.grant("https://a.test").is_none());
        assert_eq!(connections.blocked, vec!["https://a.test".to_string()]);
    }

    #[test]
    fn remembering_again_replaces_the_list_and_resumes() {
        let mut connections = Connections::default();
        connections.remember("https://a.test", vec![key(1)]);
        assert!(connections.set_ended("https://a.test", true));
        assert!(!connections.set_ended("https://a.test", true));
        connections.remember("https://a.test", vec![key(2)]);
        let grant = connections.grant("https://a.test").unwrap();
        assert_eq!(grant.accounts, vec![key(2)]);
        assert!(!grant.ended);
    }

    #[test]
    fn origins_are_compared_whole() {
        let mut connections = Connections::default();
        connections.remember("https://a.test", vec![key(1)]);
        connections.block("https://b.test");
        assert!(connections.grant("https://app.a.test").is_none());
        assert!(connections.grant("http://a.test").is_none());
        assert!(!connections.is_blocked("https://app.b.test"));
    }

    #[test]
    fn a_grant_is_saved_with_readable_addresses() {
        let mut connections = Connections::default();
        connections.remember("https://a.test", vec![key(1)]);
        let json = serde_json::to_value(&connections).unwrap();
        assert_eq!(json["sites"][0]["accounts"][0], key(1).to_string());
        assert!(json.get("blocked").is_none());
        let back: Connections = serde_json::from_value(json).unwrap();
        assert_eq!(back, connections);
    }
}
