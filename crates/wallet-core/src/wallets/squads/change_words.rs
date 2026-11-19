//! A change to the multisig in words, what the multisig looks like after it,
//! and what a careful member would want pointed out.
//!
//! The actions run in order on the multisig as it is when the change is
//! executed. Executing a change needs it not stale, and every change to the
//! members, threshold or time lock makes the open ones stale, so the
//! multisig as read now is the one it would run on: the outcome computed
//! here is exact, except for the clock (a spending limit's expiry).
//!
//! Both programs check the result before they keep it (`invariant`): at
//! least one member that may propose, one that may vote and one that may
//! execute, a threshold from 1 to the number of voters, no member twice, a
//! time lock of at most three months. A change whose result fails that can
//! never be executed, so it is said as such ([`ChangeRefusal`]) and not
//! warned about as if it could happen.

use super::*;
use crate::units::format_sol;

/// The longest time lock both programs keep: three months of 30 days.
pub const MAX_TIME_LOCK: u32 = 3 * 30 * 24 * 60 * 60;

/// A part of a sentence: words, or an address the screen names as it names
/// addresses everywhere (a wallet's label, an address book name, else the
/// address itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Key(Pubkey),
}

/// A sentence with the addresses in it kept apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Said(pub Vec<Piece>);

impl Said {
    fn new(text: impl Into<String>) -> Self {
        Self(vec![Piece::Text(text.into())])
    }

    fn t(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        match self.0.last_mut() {
            Some(Piece::Text(last)) => last.push_str(&text),
            _ => self.0.push(Piece::Text(text)),
        }
        self
    }

    fn k(mut self, key: Pubkey) -> Self {
        self.0.push(Piece::Key(key));
        self
    }

    /// `keys` joined with commas and a last "and".
    fn ks(mut self, keys: &[Pubkey]) -> Self {
        for (at, key) in keys.iter().enumerate() {
            if at > 0 {
                self = self.t(if at + 1 == keys.len() { " and " } else { ", " });
            }
            self = self.k(*key);
        }
        self
    }

    /// The sentence as text, each address called what `name` calls it.
    pub fn text(&self, name: &dyn Fn(&Pubkey) -> String) -> String {
        self.0
            .iter()
            .map(|piece| match piece {
                Piece::Text(text) => text.clone(),
                Piece::Key(key) => name(key),
            })
            .collect()
    }

    /// The addresses it names, in order.
    pub fn keys(&self) -> Vec<Pubkey> {
        self.0
            .iter()
            .filter_map(|piece| match piece {
                Piece::Key(key) => Some(*key),
                Piece::Text(_) => None,
            })
            .collect()
    }
}

/// An amount of `mint`: SOL for the default address, else the token's
/// smallest units, since a token's decimals are not in the change.
fn amount_words(said: Said, mint: &Pubkey, amount: u64) -> Said {
    if *mint == Pubkey::default() {
        said.t(format_sol(amount))
    } else {
        said.t(format!("{amount} of the smallest units of token "))
            .k(*mint)
    }
}

/// What a member with `permissions` may do, after "may".
fn may(permissions: u8) -> String {
    if permissions & PERMISSION_ALL == 0 {
        "do nothing".into()
    } else {
        permission_words(permissions)
    }
}

/// A Unix time as a day, in UTC: "2026-12-31".
fn day_words(seconds: i64) -> String {
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let days = seconds.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Each action of a change to the multisig `state` describes, in order, as
/// a person would say it. Thresholds and time locks say what they change
/// from, as the actions before them left it, when `state` is known.
pub fn change_lines(state: Option<&SquadState>, actions: &[ConfigAction]) -> Vec<Said> {
    let mut threshold = state.map(|state| state.threshold);
    let mut time_lock = state.map(|state| state.time_lock);
    actions
        .iter()
        .map(|action| match action {
            ConfigAction::AddMember(member) => Said::new("Add ")
                .k(member.key)
                .t(format!(
                    " as a member that may {}",
                    may(member.permissions)
                )),
            ConfigAction::RemoveMember(key) => Said::new("Remove ").k(*key).t(" from the members"),
            ConfigAction::ChangeThreshold(new) => {
                let said = match threshold {
                    None => Said::new(format!("Set the threshold to {new}")),
                    Some(before) if before == *new => {
                        Said::new(format!("Set the threshold to {new}, as it is already"))
                    }
                    Some(before) => {
                        Said::new(format!("Change the threshold from {before} to {new}"))
                    }
                };
                threshold = Some(*new);
                said
            }
            ConfigAction::SetTimeLock(new) => {
                let said = match (time_lock, *new) {
                    (None, 0) => Said::new("Remove the time lock, if there is one"),
                    (None, after) => Said::new(format!(
                        "Set the time lock to {}",
                        time_lock_words(after)
                    )),
                    (Some(before), after) if before == after && after == 0 => {
                        Said::new("Keep the multisig without a time lock")
                    }
                    (Some(before), after) if before == after => Said::new(format!(
                        "Set the time lock to {}, as it is already",
                        time_lock_words(after)
                    )),
                    (Some(0), after) => Said::new(format!(
                        "Set a time lock of {}: an approved proposal waits that long before it can be executed",
                        time_lock_words(after)
                    )),
                    (Some(before), 0) => Said::new(format!(
                        "Remove the time lock of {}",
                        time_lock_words(before)
                    )),
                    (Some(before), after) => Said::new(format!(
                        "Change the time lock from {} to {}",
                        time_lock_words(before),
                        time_lock_words(after)
                    )),
                };
                time_lock = Some(*new);
                said
            }
            ConfigAction::AddSpendingLimit(limit) => {
                let mut said = Said::new("Let ").ks(&limit.members).t(" move up to ");
                said = amount_words(said, &limit.mint, limit.amount);
                said = said.t(match limit.period {
                    Period::OneTime => " in all",
                    Period::Day => " a day",
                    Period::Week => " a week",
                    Period::Month => " a month (30 days)",
                });
                said = said.t(format!(" out of vault {} without a vote, ", limit.vault_index));
                said = if limit.destinations.is_empty() {
                    said.t("to any address")
                } else {
                    said.t("only to ").ks(&limit.destinations)
                };
                match limit.expiration {
                    Some(until) if until != i64::MAX => {
                        said.t(format!(", until {} UTC", day_words(until)))
                    }
                    _ => said,
                }
            }
            ConfigAction::RemoveSpendingLimit(limit) => {
                Said::new("Remove spending limit ").k(*limit)
            }
            ConfigAction::SetRentCollector(Some(key)) => Said::new(
                "Let the rent of executed, rejected and cancelled transactions' accounts be collected to ",
            )
            .k(*key),
            ConfigAction::SetRentCollector(None) => Said::new(
                "Stop the rent of finished transactions' accounts being collected: they can no longer be closed",
            ),
            ConfigAction::SetArchivalAuthority(Some(key)) => {
                Said::new("Set the archival authority to ").k(*key)
            }
            ConfigAction::SetArchivalAuthority(None) => Said::new("Clear the archival authority"),
            ConfigAction::Policy {
                verb,
                policy,
                fields,
            } => {
                let mut said = match (verb, policy) {
                    (PolicyVerb::Create, _) | (_, None) => Said::new("Create a policy"),
                    (PolicyVerb::Update, Some(policy)) => Said::new("Update policy ").k(*policy),
                    (PolicyVerb::Remove, Some(policy)) => Said::new("Remove policy ").k(*policy),
                };
                if !fields.is_empty() {
                    said = said.t(": ").t(fields
                        .iter()
                        .map(|(name, value)| format!("{name} {value}"))
                        .collect::<Vec<_>>()
                        .join(", "));
                }
                said
            }
        })
        .collect()
}

/// What the multisig would be after a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeOutcome {
    /// Sorted by key, as both programs keep them.
    pub members: Vec<SquadsMember>,
    pub threshold: u16,
    pub time_lock: u32,
    pub rent_collector: Option<Pubkey>,
    /// It changes the members, threshold or time lock, so every proposal
    /// made before it is executed becomes stale.
    pub stales: bool,
}

impl ChangeOutcome {
    /// How many members may vote.
    pub fn voters(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.permissions & PERMISSION_VOTE != 0)
            .count()
    }

    fn permissions_of(&self, key: &Pubkey) -> Option<u8> {
        self.members
            .iter()
            .find(|member| member.key == *key)
            .map(|member| member.permissions)
    }
}

/// Why the program would refuse to execute a change, whoever executes it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChangeRefusal {
    #[error("It removes {0}, which is not a member.")]
    NotAMember(Pubkey),
    #[error("It adds {0}, which is a member already.")]
    AlreadyAMember(Pubkey),
    #[error("It gives {0} a permission that does not exist.")]
    UnknownPermission(Pubkey),
    #[error("It would leave no member that may propose.")]
    NoProposers,
    #[error("It would leave no member that may execute.")]
    NoExecutors,
    #[error("It would leave no member that may vote.")]
    NoVoters,
    #[error("It would set the threshold to 0.")]
    ZeroThreshold,
    #[error("It would need {threshold} approvals with only {voters} members that may vote.")]
    ThresholdOverVoters { threshold: u16, voters: usize },
    #[error("It sets a time lock of {}, more than the three months the program allows.", time_lock_words(*.0))]
    TimeLockTooLong(u32),
    #[error("It adds a spending limit of nothing.")]
    EmptySpendingLimit,
    #[error("It adds a spending limit nobody may use.")]
    SpendingLimitForNobody,
    #[error("It adds a spending limit that names the same member twice.")]
    SpendingLimitTwice,
    #[error("It adds a spending limit that has expired.")]
    SpendingLimitExpired,
    #[error("It sets an archival authority, which the program does not do yet.")]
    ArchivalNotImplemented,
    #[error("It holds no action.")]
    NoActions,
}

impl ChangeRefusal {
    /// The whole sentence the screen shows.
    pub fn said(&self) -> Said {
        let head = "The program refuses to execute this change: ";
        match self {
            Self::NotAMember(key) => Said::new(head)
                .t("it removes ")
                .k(*key)
                .t(", which is not a member."),
            Self::AlreadyAMember(key) => Said::new(head)
                .t("it adds ")
                .k(*key)
                .t(", which is a member already."),
            Self::UnknownPermission(key) => Said::new(head)
                .t("it gives ")
                .k(*key)
                .t(" a permission that does not exist."),
            other => {
                let text = other.to_string();
                let mut chars = text.chars();
                let lower = chars
                    .next()
                    .map(|first| first.to_lowercase().chain(chars).collect::<String>())
                    .unwrap_or_default();
                Said::new(format!("{head}{lower}"))
            }
        }
        .t(" It can never be executed, so it is not offered for approval.")
    }
}

/// Run `actions` on `state` as the program would, at time `now` (Unix
/// seconds). The multisig after it, or why the program refuses it.
pub fn change_outcome(
    program: SquadsProgram,
    state: &SquadState,
    actions: &[ConfigAction],
    now: i64,
) -> Result<ChangeOutcome, ChangeRefusal> {
    if actions.is_empty() {
        return Err(ChangeRefusal::NoActions);
    }
    // The smart account program checks every spending limit's expiry
    // before it runs anything.
    if program == SquadsProgram::SmartAccount {
        for action in actions {
            if let ConfigAction::AddSpendingLimit(SpendingLimit {
                expiration: Some(until),
                ..
            }) = action
            {
                if *until <= now {
                    return Err(ChangeRefusal::SpendingLimitExpired);
                }
            }
        }
    }
    let mut members = state.members.clone();
    members.sort_by_key(|member| member.key);
    let mut out = ChangeOutcome {
        members,
        threshold: state.threshold,
        time_lock: state.time_lock,
        rent_collector: state.rent_collector,
        stales: false,
    };
    for action in actions {
        match action {
            ConfigAction::AddMember(member) => {
                // Pushed and sorted; a twin fails the check at the end.
                out.members.push(*member);
                out.members.sort_by_key(|member| member.key);
                out.stales = true;
            }
            ConfigAction::RemoveMember(key) => {
                let at = out
                    .members
                    .iter()
                    .position(|member| member.key == *key)
                    .ok_or(ChangeRefusal::NotAMember(*key))?;
                out.members.remove(at);
                out.stales = true;
            }
            ConfigAction::ChangeThreshold(threshold) => {
                out.threshold = *threshold;
                out.stales = true;
            }
            ConfigAction::SetTimeLock(seconds) => {
                out.time_lock = *seconds;
                out.stales = true;
            }
            ConfigAction::AddSpendingLimit(limit) => {
                if limit.amount == 0 {
                    return Err(ChangeRefusal::EmptySpendingLimit);
                }
                if limit.members.is_empty() {
                    return Err(ChangeRefusal::SpendingLimitForNobody);
                }
                let mut sorted = limit.members.clone();
                sorted.sort();
                if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
                    return Err(ChangeRefusal::SpendingLimitTwice);
                }
            }
            ConfigAction::RemoveSpendingLimit(_) | ConfigAction::Policy { .. } => {}
            ConfigAction::SetRentCollector(collector) => out.rent_collector = *collector,
            ConfigAction::SetArchivalAuthority(_) => {
                return Err(ChangeRefusal::ArchivalNotImplemented)
            }
        }
    }
    // The programs' `invariant`, in its order.
    if let Some(pair) = out
        .members
        .windows(2)
        .find(|pair| pair[0].key == pair[1].key)
    {
        return Err(ChangeRefusal::AlreadyAMember(pair[0].key));
    }
    if let Some(odd) = out
        .members
        .iter()
        .find(|member| member.permissions & !PERMISSION_ALL != 0)
    {
        return Err(ChangeRefusal::UnknownPermission(odd.key));
    }
    let count = |permission: u8| {
        out.members
            .iter()
            .filter(|member| member.permissions & permission != 0)
            .count()
    };
    if count(PERMISSION_INITIATE) == 0 {
        return Err(ChangeRefusal::NoProposers);
    }
    if count(PERMISSION_EXECUTE) == 0 {
        return Err(ChangeRefusal::NoExecutors);
    }
    let voters = count(PERMISSION_VOTE);
    if voters == 0 {
        return Err(ChangeRefusal::NoVoters);
    }
    if out.threshold == 0 {
        return Err(ChangeRefusal::ZeroThreshold);
    }
    if usize::from(out.threshold) > voters {
        return Err(ChangeRefusal::ThresholdOverVoters {
            threshold: out.threshold,
            voters,
        });
    }
    if out.time_lock > MAX_TIME_LOCK {
        return Err(ChangeRefusal::TimeLockTooLong(out.time_lock));
    }
    Ok(out)
}

/// "After this: 3 of 4 approvals needed · 1 day time lock", when the change
/// moves the members, threshold or time lock. `None` when it does not.
pub fn outcome_line(state: &SquadState, outcome: &ChangeOutcome) -> Option<Said> {
    let mut before = state.members.clone();
    before.sort_by_key(|member| member.key);
    let same = before == outcome.members
        && state.threshold == outcome.threshold
        && state.time_lock == outcome.time_lock;
    if same {
        return None;
    }
    let mut said = Said::new(format!(
        "After this: {} of {} approvals needed",
        outcome.threshold,
        outcome.members.len()
    ));
    if outcome.time_lock > 0 {
        said = said.t(format!(
            " · {} time lock",
            time_lock_words(outcome.time_lock)
        ));
    } else if state.time_lock > 0 {
        said = said.t(" · no time lock");
    }
    Some(said)
}

/// What a careful member would want pointed out about a change from
/// `state` to `outcome`, when `acting` votes for it: the acting member
/// leaving or losing a permission, a threshold that drops to one or that
/// needs every voter, a member added with every permission.
pub fn change_warnings(
    state: &SquadState,
    actions: &[ConfigAction],
    outcome: &ChangeOutcome,
    acting: Option<&Pubkey>,
) -> Vec<Said> {
    let mut warnings = Vec::new();
    if let Some(acting) = acting {
        let before = state
            .members
            .iter()
            .find(|member| member.key == *acting)
            .map(|member| member.permissions);
        match (before, outcome.permissions_of(acting)) {
            (Some(_), None) => warnings.push(
                Said::new("It removes ").k(*acting).t(
                    " from the members: this device acts as that member, and could no longer propose, vote or execute for the multisig.",
                ),
            ),
            (Some(before), Some(after)) if before & !after != 0 => warnings.push(
                Said::new("After it, ").k(*acting).t(format!(
                    " may only {}, where it may {} now. This device acts as that member.",
                    may(after),
                    may(before)
                )),
            ),
            _ => {}
        }
    }
    let voters = outcome.voters();
    if outcome.threshold == 1 && state.threshold > 1 {
        warnings.push(Said::new(
            "After it, one approval is enough: any single member that may vote can pass a proposal, including one that moves the vault's funds or changes the members.",
        ));
    } else if voters > 1
        && usize::from(outcome.threshold) == voters
        && (state.threshold != outcome.threshold || state.members.len() != outcome.members.len())
    {
        warnings.push(Said::new(format!(
            "After it, all {voters} members that may vote have to approve every proposal: if one of them loses their key, nothing can pass any more, not even a change to the members."
        )));
    }
    for action in actions {
        if let ConfigAction::AddMember(member) = action {
            if member.permissions == PERMISSION_ALL {
                warnings.push(
                    Said::new("It adds ")
                        .k(member.key)
                        .t(" with every permission: it may propose, vote and execute."),
                );
            }
        }
    }
    warnings
}

/// What executing a change that makes earlier proposals stale does to them.
pub const STALES_OTHERS: &str = "Once it is executed, every other proposal made so far can no longer be voted on, and approved changes to the multisig among them can no longer be executed.";

/// A change's actions in a few words, for a closed row: "adds a member and
/// changes the threshold", or "3 changes to the multisig".
pub fn change_summary(actions: &[ConfigAction]) -> String {
    let word = |action: &ConfigAction| match action {
        ConfigAction::AddMember(_) => "adds a member",
        ConfigAction::RemoveMember(_) => "removes a member",
        ConfigAction::ChangeThreshold(_) => "changes the threshold",
        ConfigAction::SetTimeLock(_) => "sets the time lock",
        ConfigAction::AddSpendingLimit(_) => "adds a spending limit",
        ConfigAction::RemoveSpendingLimit(_) => "removes a spending limit",
        ConfigAction::SetRentCollector(_) => "sets the rent collector",
        ConfigAction::SetArchivalAuthority(_) => "sets the archival authority",
        ConfigAction::Policy { .. } => "changes a policy",
    };
    match actions {
        [] => "changes nothing".into(),
        [one] => word(one).into(),
        [first, second] => format!("{} and {}", word(first), word(second)),
        more => format!("{} changes to the multisig", more.len()),
    }
}
