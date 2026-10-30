//! The way a transaction goes on the Sign screen, as steps: the ones this
//! transaction needs, in order, which are behind it and which one is next.
//!
//! The steps follow the order signing has to happen in. An authorization is
//! signed before the transaction, because the transaction signature covers
//! it. A simulation waits for the same things the Sign screen waits for:
//! a rebuild, and every authorization. Signatures from other devices come
//! after this device's own, since it shares the transaction once signed.

use super::{SignerKey, SignerLine, SignerRole, SignerState, SigningStatus};
use crate::wallets::relay::AuthorityId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignStepKind {
    /// Rebuild around a keyless wallet: a programmatic signer's
    /// authorization, or a proposal of a Squads multisig.
    Rebuild,
    /// Sign authorizations with keys on this device.
    SignAuthorization,
    /// Add authorizations signed on other devices.
    CollectAuthorizations,
    /// Ask the node what the transaction would do. Runs by itself.
    Simulate,
    /// Sign the transaction with keys on this device.
    SignTransaction,
    /// Add signatures made on other devices.
    CollectSignatures,
    Submit,
    /// Give the signed transaction back to the site that asked for it.
    ReturnToSite,
    /// This device is offline: share the transaction with one that is not.
    ShareToSubmit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignStepState {
    Done,
    /// The step the user is on.
    Current,
    /// The step the user is on, held up by an expired blockhash.
    Blocked,
    /// A simulation under way.
    Running,
    /// Behind, with a problem: the simulation failed. Signing goes on.
    Failed,
    /// Behind, unanswered: the node could not simulate.
    Unanswered,
    /// Passed over: there is no node to simulate with.
    Skipped,
    Upcoming,
}

/// Where the simulation stands, as far as the steps go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimulationStage {
    /// Not run yet: it waits for an earlier step.
    Waiting,
    Running,
    Succeeded,
    /// It ran, and the transaction would fail.
    Failed,
    /// The node did not answer.
    Unanswered,
    Offline,
}

/// What the last step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignFinish {
    Submit,
    ReturnToSite,
    ShareToSubmit,
}

impl SignFinish {
    fn kind(self) -> SignStepKind {
        match self {
            Self::Submit => SignStepKind::Submit,
            Self::ReturnToSite => SignStepKind::ReturnToSite,
            Self::ShareToSubmit => SignStepKind::ShareToSubmit,
        }
    }
}

/// What the steps need besides who has signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignStepInputs {
    /// The transaction asks a keyless wallet to sign, and is not rebuilt yet.
    pub needs_rebuild: bool,
    /// This screen rebuilt the transaction it is on.
    pub rebuilt: bool,
    pub simulation: SimulationStage,
    pub finish: SignFinish,
    /// The last step is behind: the transaction went back to the site.
    pub finished: bool,
    /// The blockhash has expired.
    pub expired: bool,
    /// Signing moves the transaction to a fresh blockhash by itself, so an
    /// expired one does not hold signing back.
    pub refreshes_on_sign: bool,
    /// Signatures the site makes once it has the transaction back, such as
    /// a new mint's. They are not collected here.
    pub left_for_site: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignStep {
    pub kind: SignStepKind,
    pub state: SignStepState,
    /// Signatures in, of those the step is about.
    pub progress: Option<(usize, usize)>,
}

/// A step before its state is set: whether it is behind, and its count.
struct Planned {
    kind: SignStepKind,
    done: bool,
    progress: Option<(usize, usize)>,
}

impl Planned {
    fn plain(kind: SignStepKind, done: bool) -> Self {
        Self {
            kind,
            done,
            progress: None,
        }
    }

    /// A step about signatures, behind once `signed` reaches `total`.
    fn counted(kind: SignStepKind, (signed, total): (usize, usize)) -> Self {
        Self {
            kind,
            done: signed >= total,
            progress: Some((signed, total)),
        }
    }
}

/// The steps `status` still needs or has been through, given `inputs`.
pub fn sign_steps(status: &SigningStatus, inputs: &SignStepInputs) -> Vec<SignStep> {
    let mut planned = Vec::new();
    if inputs.needs_rebuild || inputs.rebuilt {
        planned.push(Planned::plain(SignStepKind::Rebuild, !inputs.needs_rebuild));
    }
    if inputs.needs_rebuild {
        // Who signs is known only once it is rebuilt.
        planned.push(Planned::plain(SignStepKind::Simulate, false));
        planned.push(Planned::plain(SignStepKind::SignTransaction, false));
    } else {
        let authority = |line: &&SignerLine| matches!(line.role, SignerRole::Authority { .. });
        let outer = |line: &&SignerLine| !matches!(line.role, SignerRole::Authority { .. });
        let here = |line: &&SignerLine| line.held_here;
        let away = |line: &&SignerLine| !line.held_here;
        let lines = &status.lines;

        let sign_auth = tally(lines.iter().filter(authority).filter(here));
        let collect_auth = tally(lines.iter().filter(authority).filter(away));
        // A Vector key here signs after every other authorization, so those
        // are collected first.
        let vector_here = lines
            .iter()
            .filter(authority)
            .filter(here)
            .any(|line| matches!(line.key, SignerKey::Authority(AuthorityId::Vector(_))));
        let mut auth_steps = [
            sign_auth.map(|count| Planned::counted(SignStepKind::SignAuthorization, count)),
            collect_auth.map(|count| Planned::counted(SignStepKind::CollectAuthorizations, count)),
        ];
        if vector_here && collect_auth.is_some() {
            auth_steps.reverse();
        }
        planned.extend(auth_steps.into_iter().flatten());
        planned.push(Planned::plain(SignStepKind::Simulate, false));

        if let Some(count) = tally(lines.iter().filter(outer).filter(here)) {
            planned.push(Planned::counted(SignStepKind::SignTransaction, count));
        }
        // The site's own signatures come after it has the transaction back.
        let collected = tally(lines.iter().filter(outer).filter(away))
            .map(|(signed, total)| (signed, total.saturating_sub(inputs.left_for_site)))
            .filter(|&(_, total)| total > 0);
        if let Some(count) = collected {
            planned.push(Planned::counted(SignStepKind::CollectSignatures, count));
        }
    }
    planned.push(Planned::plain(inputs.finish.kind(), inputs.finished));

    let mut current_found = false;
    planned
        .into_iter()
        .map(
            |Planned {
                 kind,
                 done,
                 progress,
             }| {
                let state = if kind == SignStepKind::Simulate {
                    match inputs.simulation {
                        SimulationStage::Waiting => SignStepState::Upcoming,
                        SimulationStage::Running => SignStepState::Running,
                        SimulationStage::Succeeded => SignStepState::Done,
                        SimulationStage::Failed => SignStepState::Failed,
                        SimulationStage::Unanswered => SignStepState::Unanswered,
                        SimulationStage::Offline => SignStepState::Skipped,
                    }
                } else if done {
                    SignStepState::Done
                } else if current_found {
                    SignStepState::Upcoming
                } else {
                    current_found = true;
                    if held_up_by_expiry(kind, inputs) {
                        SignStepState::Blocked
                    } else {
                        SignStepState::Current
                    }
                };
                SignStep {
                    kind,
                    state,
                    progress,
                }
            },
        )
        .collect()
}

/// Nothing to follow in these steps: a simulation that runs by itself, keys on
/// this device signing, and at most the submit or return that Sign leads to.
/// Which steps there are does not change as they pass, so a screen that hides
/// them does not jump. Sharing is kept: an offline device has to act on it.
pub fn only_simulate_and_sign(steps: &[SignStep]) -> bool {
    steps
        .iter()
        .any(|step| step.kind == SignStepKind::SignTransaction)
        && steps.iter().all(|step| {
            matches!(
                step.kind,
                SignStepKind::Simulate
                    | SignStepKind::SignTransaction
                    | SignStepKind::Submit
                    | SignStepKind::ReturnToSite
            )
        })
}

/// The steps of a batch a site sent: one for each transaction, in the order
/// the site sent them, then one for returning them all. A transaction is
/// behind once this device signed it; the first that is not is the one to
/// work on. The return waits for every transaction, since the site gets them
/// all at once or not at all.
pub fn batch_steps(signed: &[bool], returned: bool) -> Vec<SignStepState> {
    let mut current_found = false;
    let mut next = |done: bool| {
        if done {
            SignStepState::Done
        } else if current_found {
            SignStepState::Upcoming
        } else {
            current_found = true;
            SignStepState::Current
        }
    };
    let mut states: Vec<SignStepState> = signed.iter().map(|&done| next(done)).collect();
    states.push(next(returned));
    states
}

/// Lines signed and lines in all, or None when there are none.
fn tally<'a>(lines: impl Iterator<Item = &'a SignerLine>) -> Option<(usize, usize)> {
    let (signed, total) = lines.fold((0, 0), |(signed, total), line| {
        (
            signed + usize::from(line.state == SignerState::Signed),
            total + 1,
        )
    });
    (total > 0).then_some((signed, total))
}

/// An expired blockhash holds back everything the transaction signature
/// covers. An authorization has no blockhash, and a rebuild makes a new one.
fn held_up_by_expiry(kind: SignStepKind, inputs: &SignStepInputs) -> bool {
    inputs.expired
        && match kind {
            SignStepKind::SignTransaction => !inputs.refreshes_on_sign,
            SignStepKind::CollectSignatures
            | SignStepKind::Submit
            | SignStepKind::ReturnToSite
            | SignStepKind::ShareToSubmit => true,
            SignStepKind::Rebuild
            | SignStepKind::SignAuthorization
            | SignStepKind::CollectAuthorizations
            | SignStepKind::Simulate => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::pk;
    use solana_pubkey::Pubkey;

    fn line(role: SignerRole, key: SignerKey, state: SignerState, held_here: bool) -> SignerLine {
        SignerLine {
            key,
            signing_as: None,
            role,
            state,
            held_here,
        }
    }

    fn payer(state: SignerState, held_here: bool) -> SignerLine {
        line(
            SignerRole::FeePayer,
            SignerKey::Key(pk(1)),
            state,
            held_here,
        )
    }

    fn signer(n: u8, state: SignerState, held_here: bool) -> SignerLine {
        line(SignerRole::Signer, SignerKey::Key(pk(n)), state, held_here)
    }

    fn authority(id: AuthorityId, state: SignerState, held_here: bool) -> SignerLine {
        let address = Pubkey::new_unique();
        line(
            SignerRole::Authority { address },
            SignerKey::Authority(id),
            state,
            held_here,
        )
    }

    fn status(lines: Vec<SignerLine>) -> SigningStatus {
        SigningStatus {
            lines,
            signable: 0,
            waiting_for_authority: false,
            unplaced_authorizations: 0,
            unverified_authorizations: 0,
        }
    }

    fn inputs(simulation: SimulationStage) -> SignStepInputs {
        SignStepInputs {
            needs_rebuild: false,
            rebuilt: false,
            simulation,
            finish: SignFinish::Submit,
            finished: false,
            expired: false,
            refreshes_on_sign: false,
            left_for_site: 0,
        }
    }

    fn shape(steps: &[SignStep]) -> Vec<(SignStepKind, SignStepState)> {
        steps.iter().map(|step| (step.kind, step.state)).collect()
    }

    use SignStepKind::*;
    use SignStepState::{Blocked, Current, Done, Running, Skipped, Upcoming};
    use SignerState::{Local, Missing, Signed};

    #[test]
    fn a_transaction_this_device_alone_signs_simulates_signs_and_submits() {
        let unsigned = status(vec![payer(Local, true)]);
        assert_eq!(
            shape(&sign_steps(&unsigned, &inputs(SimulationStage::Running))),
            vec![
                (Simulate, Running),
                (SignTransaction, Current),
                (Submit, Upcoming)
            ]
        );
        let signed = status(vec![payer(Signed, true)]);
        assert_eq!(
            shape(&sign_steps(&signed, &inputs(SimulationStage::Succeeded))),
            vec![(Simulate, Done), (SignTransaction, Done), (Submit, Current)]
        );
    }

    #[test]
    fn signatures_from_other_devices_are_collected_after_this_devices() {
        let lines = vec![
            payer(Signed, true),
            signer(2, Missing, false),
            signer(3, Signed, false),
        ];
        let steps = sign_steps(&status(lines), &inputs(SimulationStage::Succeeded));
        assert_eq!(
            shape(&steps),
            vec![
                (Simulate, Done),
                (SignTransaction, Done),
                (CollectSignatures, Current),
                (Submit, Upcoming)
            ]
        );
        assert_eq!(steps[2].progress, Some((1, 2)));
    }

    #[test]
    fn a_simulation_waits_for_every_authorization() {
        let lines = vec![
            authority(AuthorityId::Ed25519(pk(7)), Signed, true),
            authority(AuthorityId::Ed25519(pk(8)), Missing, false),
            payer(Local, true),
        ];
        let steps = sign_steps(&status(lines), &inputs(SimulationStage::Waiting));
        assert_eq!(
            shape(&steps),
            vec![
                (SignAuthorization, Done),
                (CollectAuthorizations, Current),
                (Simulate, Upcoming),
                (SignTransaction, Upcoming),
                (Submit, Upcoming)
            ]
        );
        assert_eq!(steps[1].progress, Some((0, 1)));
    }

    #[test]
    fn a_vector_key_here_waits_for_the_other_authorizations() {
        let lines = vec![
            authority(AuthorityId::Ed25519(pk(7)), Missing, false),
            authority(AuthorityId::Vector(pk(9)), Local, true),
            payer(Local, true),
        ];
        let steps = sign_steps(&status(lines), &inputs(SimulationStage::Waiting));
        assert_eq!(
            shape(&steps)[..2],
            [
                (CollectAuthorizations, Current),
                (SignAuthorization, Upcoming)
            ]
        );
    }

    #[test]
    fn a_rebuild_comes_first_and_stays_behind_once_done() {
        let mut asked = inputs(SimulationStage::Waiting);
        asked.needs_rebuild = true;
        let vault = status(vec![payer(Local, true), signer(2, Missing, false)]);
        assert_eq!(
            shape(&sign_steps(&vault, &asked)),
            vec![
                (Rebuild, Current),
                (Simulate, Upcoming),
                (SignTransaction, Upcoming),
                (Submit, Upcoming)
            ]
        );
        let mut rebuilt = inputs(SimulationStage::Succeeded);
        rebuilt.rebuilt = true;
        let proposal = status(vec![payer(Local, true)]);
        assert_eq!(shape(&sign_steps(&proposal, &rebuilt))[0], (Rebuild, Done));
    }

    #[test]
    fn an_expired_blockhash_holds_back_what_the_transaction_signature_covers() {
        let lines = vec![payer(Local, true)];
        let mut expired = inputs(SimulationStage::Succeeded);
        expired.expired = true;
        assert_eq!(
            shape(&sign_steps(&status(lines.clone()), &expired))[1],
            (SignTransaction, Blocked)
        );
        expired.refreshes_on_sign = true;
        assert_eq!(
            shape(&sign_steps(&status(lines), &expired))[1],
            (SignTransaction, Current)
        );
        let authorizing = vec![
            authority(AuthorityId::Ed25519(pk(7)), Local, true),
            payer(Local, true),
        ];
        assert_eq!(
            shape(&sign_steps(&status(authorizing), &expired))[0],
            (SignAuthorization, Current)
        );
    }

    #[test]
    fn a_site_signs_for_what_it_creates_after_it_has_the_transaction_back() {
        let lines = vec![payer(Signed, true), signer(2, Missing, false)];
        let mut site = inputs(SimulationStage::Succeeded);
        site.finish = SignFinish::ReturnToSite;
        site.left_for_site = 1;
        assert_eq!(
            shape(&sign_steps(&status(lines.clone()), &site)),
            vec![
                (Simulate, Done),
                (SignTransaction, Done),
                (ReturnToSite, Current)
            ]
        );
        site.finished = true;
        assert_eq!(
            shape(&sign_steps(&status(lines), &site))[2],
            (ReturnToSite, Done)
        );
    }

    #[test]
    fn offline_the_simulation_is_skipped_and_another_device_submits() {
        let mut offline = inputs(SimulationStage::Offline);
        offline.finish = SignFinish::ShareToSubmit;
        assert_eq!(
            shape(&sign_steps(&status(vec![payer(Local, true)]), &offline)),
            vec![
                (Simulate, Skipped),
                (SignTransaction, Current),
                (ShareToSubmit, Upcoming)
            ]
        );
    }

    #[test]
    fn only_simulate_and_sign_holds_for_a_hot_key_alone_and_nothing_more() {
        let hot = status(vec![payer(Local, true)]);
        let mut plain = inputs(SimulationStage::Running);
        assert!(only_simulate_and_sign(&sign_steps(&hot, &plain)));
        plain.finish = SignFinish::ReturnToSite;
        let mut own = sign_steps(&hot, &plain);
        assert!(only_simulate_and_sign(&own));
        // A transaction of a batch, whose return is the batch's.
        own.pop();
        assert!(only_simulate_and_sign(&own));

        let mut offline = inputs(SimulationStage::Offline);
        offline.finish = SignFinish::ShareToSubmit;
        assert!(!only_simulate_and_sign(&sign_steps(&hot, &offline)));

        let mut rebuilt = inputs(SimulationStage::Succeeded);
        rebuilt.rebuilt = true;
        assert!(!only_simulate_and_sign(&sign_steps(&hot, &rebuilt)));

        let shared = status(vec![payer(Local, true), signer(2, Missing, false)]);
        let collecting = sign_steps(&shared, &inputs(SimulationStage::Succeeded));
        assert!(!only_simulate_and_sign(&collecting));

        let authorizing = status(vec![
            authority(AuthorityId::Ed25519(pk(7)), Local, true),
            payer(Local, true),
        ]);
        let waiting = sign_steps(&authorizing, &inputs(SimulationStage::Waiting));
        assert!(!only_simulate_and_sign(&waiting));

        // Nothing for this device to sign.
        let elsewhere = status(vec![payer(Signed, false)]);
        let submit_only = sign_steps(&elsewhere, &inputs(SimulationStage::Succeeded));
        assert!(!only_simulate_and_sign(&submit_only));
    }

    #[test]
    fn a_batch_returns_only_after_every_transaction() {
        use SignStepState::{Current, Done, Upcoming};
        assert_eq!(
            batch_steps(&[false, false, false], false),
            vec![Current, Upcoming, Upcoming, Upcoming]
        );
        // Signed out of order: the first left is next, the return still waits.
        assert_eq!(
            batch_steps(&[true, false, true], false),
            vec![Done, Current, Done, Upcoming]
        );
        assert_eq!(batch_steps(&[true, true], false), vec![Done, Done, Current]);
        assert_eq!(batch_steps(&[true, true], true), vec![Done, Done, Done]);
    }

    #[test]
    fn a_failed_simulation_does_not_hold_signing_back() {
        let steps = sign_steps(
            &status(vec![payer(Local, true)]),
            &inputs(SimulationStage::Failed),
        );
        assert_eq!(
            shape(&steps)[..2],
            [
                (Simulate, SignStepState::Failed),
                (SignTransaction, Current)
            ]
        );
    }
}
