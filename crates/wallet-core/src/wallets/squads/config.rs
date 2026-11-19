//! A multisig's configuration as a wallet record keeps it, and what changed
//! between two readings of it.
//!
//! The record keeps what decides who acts and how: the members with what
//! each may do, the threshold, the time lock, the config authority and the
//! rent collector. The transaction numbers move with every proposal and are
//! read fresh when they are needed, so they are not kept.

use super::*;

/// What a wallet record keeps of a multisig's account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SquadConfig {
    pub members: Vec<SquadsMember>,
    pub threshold: u16,
    /// Seconds an approved proposal waits before it can be executed.
    pub time_lock: u32,
    /// See [`SquadState::config_authority`].
    pub config_authority: Option<Pubkey>,
    /// See [`SquadState::rent_collector`].
    pub rent_collector: Option<Pubkey>,
}

impl From<&SquadState> for SquadConfig {
    fn from(state: &SquadState) -> Self {
        Self {
            members: state.members.clone(),
            threshold: state.threshold,
            time_lock: state.time_lock,
            config_authority: state.config_authority,
            rent_collector: state.rent_collector,
        }
    }
}

impl From<&SquadsSetup> for SquadConfig {
    fn from(setup: &SquadsSetup) -> Self {
        Self {
            members: setup.members.clone(),
            threshold: setup.threshold,
            time_lock: setup.time_lock,
            config_authority: setup.config_authority,
            rent_collector: setup.rent_collector,
        }
    }
}

/// What a new reading of a multisig changed in what its record kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SquadChanges {
    /// Nothing was read before: the members were unknown.
    pub first_read: bool,
    pub added: Vec<Pubkey>,
    pub removed: Vec<Pubkey>,
    /// Members whose permissions changed. A record from before permissions
    /// were kept has none to compare, and lists nobody here.
    pub permissions: Vec<Pubkey>,
    /// Before and after.
    pub threshold: Option<(u16, u16)>,
    pub time_lock: Option<(u32, u32)>,
    pub config_authority: Option<(Option<Pubkey>, Option<Pubkey>)>,
    pub rent_collector: Option<(Option<Pubkey>, Option<Pubkey>)>,
}

impl SquadChanges {
    /// Nothing anyone would notice changed.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// What changed, in sentences for the person, for a multisig of
    /// `program`, with each address called what `name` calls it. Empty when
    /// nothing did.
    pub fn sentences(
        &self,
        program: SquadsProgram,
        now: &SquadConfig,
        name: &dyn Fn(&Pubkey) -> String,
    ) -> Vec<String> {
        let authority = authority_name(program);
        if self.first_read {
            return vec![format!(
                "Read from the chain: {} of {} members.",
                now.threshold,
                now.members.len()
            )];
        }
        let mut out = Vec::new();
        if let Some((before, after)) = self.threshold {
            out.push(format!("The threshold went from {before} to {after}."));
        }
        let keys = |keys: &[Pubkey]| keys.iter().map(name).collect::<Vec<_>>().join(", ");
        match self.added.len() {
            0 => {}
            1 => out.push(format!("{} became a member.", keys(&self.added))),
            _ => out.push(format!("{} became members.", keys(&self.added))),
        }
        match self.removed.len() {
            0 => {}
            1 => out.push(format!("{} is no longer a member.", keys(&self.removed))),
            _ => out.push(format!("{} are no longer members.", keys(&self.removed))),
        }
        if !self.permissions.is_empty() {
            out.push(format!("What {} may do changed.", keys(&self.permissions)));
        }
        if let Some((before, after)) = self.time_lock {
            out.push(format!(
                "The time lock went from {} to {}.",
                time_lock_words(before),
                time_lock_words(after)
            ));
        }
        match self.config_authority {
            Some((_, Some(key))) => out.push(format!(
                "Its {authority} is now {}, which changes its members and threshold without a vote.",
                name(&key)
            )),
            Some((_, None)) => out.push(format!(
                "It no longer has a {authority}: its members vote on changes to it."
            )),
            None => {}
        }
        match self.rent_collector {
            Some((_, Some(key))) => out.push(format!(
                "Rent of closed transactions now goes to {}.",
                name(&key)
            )),
            Some((_, None)) => {
                out.push("Its transactions' accounts can no longer be closed.".into())
            }
            None => {}
        }
        out
    }
}

/// What the program calls the key that changes a multisig by itself.
pub fn authority_name(program: SquadsProgram) -> &'static str {
    match program {
        SquadsProgram::V4 => "config authority",
        SquadsProgram::SmartAccount => "settings authority",
    }
}

/// A time lock in words: "none", "90 seconds", "2 hours".
pub fn time_lock_words(seconds: u32) -> String {
    let unit = |count: u32, one: &str| {
        if count == 1 {
            format!("1 {one}")
        } else {
            format!("{count} {one}s")
        }
    };
    match seconds {
        0 => "none".into(),
        s if s % 86_400 == 0 => unit(s / 86_400, "day"),
        s if s % 3_600 == 0 => unit(s / 3_600, "hour"),
        s if s % 60 == 0 => unit(s / 60, "minute"),
        s => unit(s, "second"),
    }
}

/// What a member with `permissions` may do, in words: "propose, vote and
/// execute", "vote", "nothing".
pub fn permission_words(permissions: u8) -> String {
    let can: Vec<&str> = [
        (PERMISSION_INITIATE, "propose"),
        (PERMISSION_VOTE, "vote"),
        (PERMISSION_EXECUTE, "execute"),
    ]
    .into_iter()
    .filter(|(bit, _)| permissions & bit != 0)
    .map(|(_, word)| word)
    .collect();
    match can.as_slice() {
        [] => "nothing".into(),
        [one] => (*one).into(),
        [first, second] => format!("{first} and {second}"),
        _ => format!("{}, {} and {}", can[0], can[1], can[2]),
    }
}
