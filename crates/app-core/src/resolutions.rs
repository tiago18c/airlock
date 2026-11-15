//! What became of the requests sites asked Airlock to resolve
//! (`docs/intents.md`). Each resolution belongs to the origin that made it,
//! and another origin never finds it. Saved deferred to `resolutions.json`,
//! the latest [`MAX_RESOLUTIONS`].
//!
//! A resolution can hold the wires the wallet signed and has not sent yet
//! (`held`). Those stay in memory: the file never has them, so a resolution
//! that needed them when the app stopped cannot go on after it starts again
//! ([`Resolutions::after_restart`]).

use bridge_protocol::reply::SendError;
use serde::{Deserialize, Serialize};
use solana_signature::Signature;
use wallet_core::intent::{Resolution, ResolutionError, ResolutionFailure, SendResult, State};
use wallet_vault::store::{check_saveable, load_json, LoadProblem, VaultStore};
use wallet_vault::VaultError;

/// How a send's report of a transaction that never went out begins
/// (`wallet_flows::send::NOT_SENT`, which this crate does not depend on).
const NOT_SENT_START: &str = "Not sent, because an earlier transaction failed";

/// How many resolutions are kept. One past it drops the oldest that has
/// ended, or the oldest of all when none has.
pub const MAX_RESOLUTIONS: usize = 200;

/// A resolution, with the site it belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptResolution {
    pub origin: String,
    pub resolution: Resolution,
    /// Its wires were taken to be sent, so nobody sends them again. Kept in
    /// memory only: after a restart nothing is being sent.
    #[serde(skip)]
    pub claimed: bool,
}

impl KeptResolution {
    /// It is `sending`, its wires are here, and nobody has taken them yet:
    /// the window sends it (`PublicCore::resolution_begin_send`).
    pub fn ready_to_send(&self) -> bool {
        !self.claimed
            && self
                .resolution
                .wires_to_send()
                .is_some_and(|wires| !wires.is_empty())
    }
}

/// Every resolution kept, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolutions {
    items: Vec<KeptResolution>,
}

/// A site used an id it used before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DuplicateId;

/// Why a resolution was not made, and its request not queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolutionRefused {
    /// The site used this id before (`duplicate_id`).
    DuplicateId,
    /// Its site, or every site together, has as many requests waiting as may
    /// wait (`too_many_requests`).
    Full,
    /// A request with the same bridge id is waiting (`bad_request`).
    SameRequest,
}

/// Why a command on a resolution changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolutionCommandError {
    /// The origin has no resolution with this id.
    NotFound,
    /// The resolution does not take this change.
    Refused(ResolutionError),
}

impl ResolutionCommandError {
    /// The error code on the wire: `not_found`, `bad_state`,
    /// `bad_signature` or `internal`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Refused(err) => err.code(),
        }
    }
}

impl std::fmt::Display for ResolutionCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("No resolution of this site has this id."),
            Self::Refused(err) => write!(f, "{err}"),
        }
    }
}

/// What a send told the site, as a resolution takes it: a signature is a
/// transaction that landed (a resolution's sends wait until each is
/// confirmed), `send_failed` one that failed, or that an earlier failure
/// kept from going out, and `send_unknown` one that may still land.
pub fn send_results(results: Vec<Result<[u8; 64], SendError>>) -> Vec<SendResult> {
    results
        .into_iter()
        .map(|result| match result {
            Ok(signature) => SendResult::Landed {
                signature: Signature::from(signature),
            },
            // `wallet_flows::send::NOT_SENT`, as a sentence: it never went
            // out, so it has no signature to look for.
            Err(SendError::Failed(reason)) if reason.starts_with(NOT_SENT_START) => {
                SendResult::NotSent { reason }
            }
            Err(SendError::Failed(reason)) => SendResult::Failed {
                signature: None,
                reason,
            },
            Err(SendError::Unknown(_)) => SendResult::Unknown { signature: None },
        })
        .collect()
}

impl Resolutions {
    pub fn items(&self) -> &[KeptResolution] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The resolution `id` of `origin`. Another origin's is not found.
    pub fn get(&self, origin: &str, id: &str) -> Option<&KeptResolution> {
        self.items
            .iter()
            .find(|kept| kept.origin == origin && kept.resolution.id == id)
    }

    pub(crate) fn get_mut(&mut self, origin: &str, id: &str) -> Option<&mut KeptResolution> {
        self.items
            .iter_mut()
            .find(|kept| kept.origin == origin && kept.resolution.id == id)
    }

    /// Those the window has to send: see [`KeptResolution::ready_to_send`].
    pub fn ready_to_send(&self) -> impl Iterator<Item = &KeptResolution> {
        self.items.iter().filter(|kept| kept.ready_to_send())
    }

    /// Keep `resolution` for `origin`. Refused when the site used its id
    /// before, whatever became of that one.
    pub(crate) fn insert(
        &mut self,
        origin: &str,
        resolution: Resolution,
    ) -> Result<(), DuplicateId> {
        if self.get(origin, &resolution.id).is_some() {
            return Err(DuplicateId);
        }
        self.items.push(KeptResolution {
            origin: origin.to_owned(),
            resolution,
            claimed: false,
        });
        while self.items.len() > MAX_RESOLUTIONS {
            // Never the one just kept.
            let older = &self.items[..self.items.len() - 1];
            let at = older
                .iter()
                .position(|kept| kept.resolution.state.is_final())
                .unwrap_or(0);
            self.items.remove(at);
        }
        Ok(())
    }

    /// End what cannot go on once the app has started again, at `now`.
    /// Answers whether anything changed.
    ///
    /// - `awaiting-approval`: the request it waited on was in memory, and is
    ///   gone, so nobody can approve it. `failed`.
    /// - `awaiting-app`: the wires the app was to sign with were in memory.
    ///   `failed`.
    /// - `sending`: they may have gone out before the app stopped, so it is
    ///   `unknown`, never `failed`: sending again as something new could do
    ///   it twice.
    pub fn after_restart(&mut self, now: u64) -> bool {
        let mut changed = false;
        for kept in &mut self.items {
            let resolution = &mut kept.resolution;
            let ended = match resolution.state {
                State::AwaitingApproval => resolution.fail(restarted(RESTARTED_UNDECIDED), now),
                State::AwaitingApp => resolution.fail(restarted(RESTARTED_UNSIGNED), now),
                State::Sending => {
                    let results = resolution
                        .transactions
                        .iter()
                        .map(|made| SendResult::Unknown {
                            signature: made.signature,
                        })
                        .collect();
                    resolution.sent(results, now)
                }
                _ => continue,
            };
            changed |= ended.is_ok();
        }
        changed
    }

    /// What is written to the file: everything but the wires held.
    fn for_disk(&self) -> Self {
        Self {
            items: self
                .items
                .iter()
                .map(|kept| KeptResolution {
                    resolution: Resolution {
                        held: Vec::new(),
                        ..kept.resolution.clone()
                    },
                    ..kept.clone()
                })
                .collect(),
        }
    }
}

const RESTARTED_UNDECIDED: &str =
    "Airlock was restarted before anyone decided, and the request did not outlast it. Send it again.";
const RESTARTED_UNSIGNED: &str = "Airlock was restarted while it waited for the app's signatures, and the transactions it had signed were not kept. Send the request again.";

fn restarted(message: &str) -> ResolutionFailure {
    ResolutionFailure {
        code: "failed".into(),
        message: message.into(),
        requirement: None,
    }
}

/// On-disk wrapper, same shape as the notices.
pub struct ResolutionsStore<S: VaultStore> {
    store: S,
    data: Resolutions,
    problem: Option<LoadProblem>,
}

impl<S: VaultStore> ResolutionsStore<S> {
    pub fn open(store: S) -> Self {
        let (data, problem) = load_json(&store, "resolutions");
        Self {
            store,
            data,
            problem,
        }
    }

    pub fn load_problem(&self) -> Option<&LoadProblem> {
        self.problem.as_ref()
    }

    pub fn data(&self) -> &Resolutions {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Resolutions {
        &mut self.data
    }

    /// Write every resolution, without the wires held.
    pub fn save(&self) -> Result<(), VaultError> {
        check_saveable(&self.problem)?;
        let json = serde_json::to_vec_pretty(&self.data.for_disk())
            .map_err(|e| VaultError::Serde(e.to_string()))?;
        self.store.save(&json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_pubkey::Pubkey;
    use wallet_core::intent::{Delivery, Timing};

    fn resolution(id: &str) -> Resolution {
        Resolution::new(
            id,
            Pubkey::new_from_array([1; 32]),
            "solana:devnet",
            Timing::Sync,
            Delivery::Send,
            1,
            10,
        )
    }

    #[test]
    fn an_id_is_the_sites_own_and_used_once() {
        let mut kept = Resolutions::default();
        kept.insert("https://a.test", resolution("one")).unwrap();
        assert_eq!(
            kept.insert("https://a.test", resolution("one")),
            Err(DuplicateId)
        );
        // Another site may use the same id, and finds only its own.
        kept.insert("https://b.test", resolution("one")).unwrap();
        assert_eq!(kept.len(), 2);
        assert!(kept.get("https://c.test", "one").is_none());
        assert_eq!(
            kept.get("https://b.test", "one").unwrap().origin,
            "https://b.test"
        );
    }

    #[test]
    fn past_the_limit_the_oldest_that_ended_goes_first() {
        let mut kept = Resolutions::default();
        for n in 0..MAX_RESOLUTIONS {
            let mut one = resolution(&format!("r{n}"));
            if n == 5 {
                one.decline(11).unwrap();
            }
            kept.insert("https://a.test", one).unwrap();
        }
        kept.insert("https://a.test", resolution("new")).unwrap();
        assert_eq!(kept.len(), MAX_RESOLUTIONS);
        assert!(kept.get("https://a.test", "r5").is_none());
        assert!(kept.get("https://a.test", "r0").is_some());
        // With none ended, the oldest of all.
        kept.insert("https://a.test", resolution("newer")).unwrap();
        assert!(kept.get("https://a.test", "r0").is_none());
        assert!(kept.get("https://a.test", "newer").is_some());
    }
}
